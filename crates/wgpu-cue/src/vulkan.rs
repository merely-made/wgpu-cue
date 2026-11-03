//! Producer-complete, move-owned Linux DMA-BUF frames on a Vulkan host queue.
use crate::*;
use webcue::grafting_frame::{FrameSync, OwnedNativeImage};

/// Linux Vulkan importer. A `None` synchronization value requires a completed
/// producer snapshot: flushing a foreign GL context alone is insufficient.
/// Descriptor import consumes the owned frame exactly once. Graft schedules
/// queue-family acquisition and retires its command resources asynchronously;
/// this importer never waits for GPU completion or pumps an event loop.
#[derive(Default)]
pub struct GraftVulkanImporter;
impl NativeImporter for GraftVulkanImporter {
    fn try_import(&mut self, cx: &RenderContext<'_>, frame: NativeFrame) -> Result<Imported> {
        if !matches!(frame.frame.sync(), FrameSync::None) {
            return Err(Error::Unsupported("this DMA-BUF binding requires a producer-complete snapshot; external semaphore synchronization is not implemented".into()));
        }
        let (metadata, image, _) = frame.frame.into_parts();
        let OwnedNativeImage::DmaBufImage(image) = image else {
            return Err(Error::Unsupported(
                "only owned DMA-BUF images are supported".into(),
            ));
        };
        if metadata.size.width == 0
            || metadata.size.height == 0
            || metadata.size != FrameSize::new(cx.viewport.width, cx.viewport.height)
            || metadata.generation == 0
            || metadata.generation != frame.stamp.allocation_epoch
        {
            return Err(Error::Invalid(
                "DMA-BUF metadata differs from the viewport or allocation epoch".into(),
            ));
        }
        let (format, alpha_fourcc, opaque_fourcc) = match metadata.format {
            PixelFormat::Rgba8Unorm => (
                wgpu::TextureFormat::Rgba8Unorm,
                u32::from_le_bytes(*b"AB24"),
                u32::from_le_bytes(*b"XB24"),
            ),
            PixelFormat::Rgba8UnormSrgb => (
                wgpu::TextureFormat::Rgba8UnormSrgb,
                u32::from_le_bytes(*b"AB24"),
                u32::from_le_bytes(*b"XB24"),
            ),
            PixelFormat::Bgra8Unorm => (
                wgpu::TextureFormat::Bgra8Unorm,
                u32::from_le_bytes(*b"AR24"),
                u32::from_le_bytes(*b"XR24"),
            ),
            PixelFormat::Bgra8UnormSrgb => (
                wgpu::TextureFormat::Bgra8UnormSrgb,
                u32::from_le_bytes(*b"AR24"),
                u32::from_le_bytes(*b"XR24"),
            ),
            _ => return Err(Error::Unsupported("DMA-BUF pixel format".into())),
        };
        if image.drm_format() != alpha_fourcc
            && !(image.drm_format() == opaque_fourcc && frame.alpha == Alpha::Opaque)
        {
            return Err(Error::Invalid(
                "DMA-BUF DRM format disagrees with the declared pixel format or alpha".into(),
            ));
        }
        if image.planes().len() != 1
            || image.planes()[0].stride < u64::from(metadata.size.width) * 4
        {
            return Err(Error::Invalid(
                "RGBA DMA-BUF requires one plane with a full row stride".into(),
            ));
        }
        if image.drm_modifier() == (1u64 << 56) - 1 {
            return Err(Error::Unsupported(
                "DMA-BUF requires an explicit DRM modifier".into(),
            ));
        }
        let (buffers, planes, drm_format, drm_modifier) = image.into_parts();
        let import = grafting::vulkan_dmabuf::VulkanDmaBufImport::new(
            dpi::PhysicalSize::new(metadata.size.width, metadata.size.height),
            format,
            drm_format,
            drm_modifier,
            buffers,
            planes
                .into_iter()
                .map(|p| grafting::vulkan_dmabuf::VulkanDmaBufPlane {
                    buffer_index: p.buffer_index as usize,
                    offset: p.offset,
                    stride: p.stride,
                })
                .collect(),
            grafting::vulkan_dmabuf::VulkanDmaBufQueueOwnership::Foreign,
        )
        .map_err(|e| Error::Invalid(e.to_string()))?;
        let host = grafting::HostWgpuContext::new(cx.device.clone(), cx.queue.clone());
        let texture = grafting::vulkan_dmabuf::import_dmabuf(import, &host)
            .map_err(|e| Error::Backend(e.to_string()))?;
        Ok(Imported::Ready(TextureFrame {
            texture,
            stamp: frame.stamp,
            device_epoch: cx.device_epoch,
            alpha: frame.alpha,
            encoding: frame.encoding,
        }))
    }
}

/// Final-use tracking for completed Graft Vulkan exports. The host transports
/// the owned descriptor and validates the producer's completion witness before
/// calling `try_import`. Seal after the last GPU submission, then drop every
/// texture/view reference. Return only `poll_releases` stamps to the producer.
/// Peer loss never releases storage for reuse.
pub struct GraftVulkanSurfaceImporter {
    scope: Scope,
    device_epoch: DeviceEpoch,
    last: Option<FrameStamp>,
    active: Vec<(FrameStamp, grafting::vulkan_surface::VulkanRelease)>,
}
impl GraftVulkanSurfaceImporter {
    pub fn new(scope: Scope, device_epoch: DeviceEpoch) -> Self {
        Self {
            scope,
            device_epoch,
            last: None,
            active: Vec::new(),
        }
    }
    pub fn seal_use(&mut self, stamp: FrameStamp) -> Result<()> {
        self.active
            .iter_mut()
            .find(|(active, _)| *active == stamp)
            .ok_or(Error::NotPending)?
            .1
            .seal()
            .map_err(|e| Error::Invalid(e.to_string()))
    }
    pub fn in_flight(&self) -> usize {
        self.active.len()
    }
    pub fn poll_releases(&mut self) -> Result<Vec<FrameStamp>> {
        let mut released = Vec::new();
        let mut i = 0;
        while i < self.active.len() {
            if self.active[i]
                .1
                .poll_released()
                .map_err(|e| Error::Backend(e.to_string()))?
            {
                released.push(self.active.remove(i).0);
            } else {
                i += 1;
            }
        }
        Ok(released)
    }
}
impl NativeImporter for GraftVulkanSurfaceImporter {
    fn try_import(&mut self, cx: &RenderContext<'_>, frame: NativeFrame) -> Result<Imported> {
        if frame.stamp.scope != self.scope || cx.device_epoch != self.device_epoch {
            return Err(Error::Stale);
        }
        if self.last.is_some_and(|last| {
            frame.stamp.allocation_epoch <= last.allocation_epoch
                || frame.stamp.content_revision <= last.content_revision
        }) {
            return Err(Error::Stale);
        }
        if self.active.len() >= 64 {
            return Err(Error::Backpressure);
        }
        if !matches!(frame.frame.sync(), FrameSync::None) {
            return Err(Error::Unsupported(
                "Vulkan surface requires observed producer completion".into(),
            ));
        }
        let (metadata, image, _) = frame.frame.into_parts();
        if metadata.generation == 0
            || metadata.generation != frame.stamp.allocation_epoch
            || metadata.size != FrameSize::new(cx.viewport.width, cx.viewport.height)
            || metadata.format != PixelFormat::Rgba8Unorm
        {
            return Err(Error::Invalid(
                "Vulkan surface extent/format/stamp mismatch".into(),
            ));
        }
        let OwnedNativeImage::DmaBufImage(image) = image else {
            return Err(Error::Unsupported("expected owned DMA-BUF image".into()));
        };
        let (mut buffers, planes, fourcc, modifier) = image.into_parts();
        if buffers.len() != 1
            || planes.len() != 1
            || planes[0].buffer_index != 0
            || fourcc != u32::from_le_bytes(*b"AB24")
            || modifier != 0
        {
            return Err(Error::Unsupported(
                "Vulkan surface requires single-plane linear RGBA8".into(),
            ));
        }
        let host = grafting::HostWgpuContext::new(cx.device.clone(), cx.queue.clone());
        let (texture, release) = grafting::vulkan_surface::import_surface(
            buffers.remove(0),
            metadata.size,
            planes[0].offset,
            planes[0].stride,
            &host,
        )
        .map_err(|e| Error::Backend(e.to_string()))?;
        self.last = Some(frame.stamp);
        self.active.push((frame.stamp, release));
        Ok(Imported::Ready(TextureFrame {
            texture,
            stamp: frame.stamp,
            device_epoch: cx.device_epoch,
            alpha: frame.alpha,
            encoding: frame.encoding,
        }))
    }
}
