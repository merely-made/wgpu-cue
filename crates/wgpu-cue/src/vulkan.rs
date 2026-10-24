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
