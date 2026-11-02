// SPDX-License-Identifier: MPL-2.0
//! Retained custody enters Graft only after an immediately observable fence.
use crate::*;
use objc2_metal::{MTLPixelFormat, MTLSharedEvent, MTLTexture, MTLTextureType};
use webcue::grafting_frame::{FrameSync, OwnedNativeImage};
/// macOS Metal importer. No CPU wait or run-loop pumping is performed.
/// `FrameSync::None` requires an already ordered producer (for example the
/// same queue). Unsupported image/sync kinds are explicit errors.
#[derive(Default)]
pub struct GraftMetalImporter;
impl NativeImporter for GraftMetalImporter {
    fn try_import(&mut self, cx: &RenderContext<'_>, frame: NativeFrame) -> Result<Imported> {
        match frame.frame.sync() {
            FrameSync::None => {}
            FrameSync::MetalSharedEvent { event, value } => {
                if event.signaledValue() < *value {
                    return Ok(Imported::Pending(frame));
                }
            }
            _ => return Err(Error::Unsupported("non-Metal synchronization".into())),
        }
        let (metadata, image, _sync) = frame.frame.into_parts();
        let OwnedNativeImage::MetalTexture(texture) = image else {
            return Err(Error::Unsupported(
                "only retained Metal textures are implemented".into(),
            ));
        };
        let format = match metadata.format {
            PixelFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
            PixelFormat::Rgba8UnormSrgb => wgpu::TextureFormat::Rgba8UnormSrgb,
            PixelFormat::Bgra8Unorm => wgpu::TextureFormat::Bgra8Unorm,
            PixelFormat::Bgra8UnormSrgb => wgpu::TextureFormat::Bgra8UnormSrgb,
            _ => return Err(Error::Unsupported("native pixel format".into())),
        };
        let retained = texture.into_retained();
        let native_format = match format {
            wgpu::TextureFormat::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb => MTLPixelFormat::RGBA8Unorm_sRGB,
            wgpu::TextureFormat::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
            wgpu::TextureFormat::Bgra8UnormSrgb => MTLPixelFormat::BGRA8Unorm_sRGB,
            _ => unreachable!(),
        };
        if retained.width() != metadata.size.width as usize
            || retained.height() != metadata.size.height as usize
            || retained.pixelFormat() != native_format
            || retained.textureType() != MTLTextureType::Type2D
            || retained.sampleCount() != 1
            || retained.arrayLength() != 1
        {
            return Err(Error::Invalid(
                "Metal resource differs from declared extent, format or shape".into(),
            ));
        }
        let host = grafting::HostWgpuContext::new(cx.device.clone(), cx.queue.clone());
        let metadata = grafting::FrameMetadata {
            size: dpi::PhysicalSize::new(metadata.size.width, metadata.size.height),
            format,
            generation: metadata.generation,
            producer_sync: grafting::SyncMechanism::None,
        };
        let texture = grafting::import_metal_texture_ref(
            grafting::MetalTextureRef::new(metadata, retained),
            &host,
        )
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

/// IOSurface importer with explicit final-use tracking. The host must withhold
/// a frame until it has observed producer GPU completion. After its last GPU
/// submission, call `seal_use`, drop all views/textures (including Presenter
/// custody), then return each `poll_releases` stamp to the producer exactly once.
/// A lost peer does not release storage; abandon its pool and start a new scope.
pub struct GraftIoSurfaceImporter {
    scope: Scope,
    device_epoch: DeviceEpoch,
    last: Option<FrameStamp>,
    active: Vec<(FrameStamp, grafting::metal_surface::MetalRelease)>,
}
impl GraftIoSurfaceImporter {
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
            .find(|(s, _)| *s == stamp)
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
impl NativeImporter for GraftIoSurfaceImporter {
    fn try_import(&mut self, cx: &RenderContext<'_>, frame: NativeFrame) -> Result<Imported> {
        if frame.stamp.scope != self.scope || cx.device_epoch != self.device_epoch {
            return Err(Error::Stale);
        }
        if self.last.is_some_and(|s| {
            frame.stamp.allocation_epoch <= s.allocation_epoch
                || frame.stamp.content_revision <= s.content_revision
        }) {
            return Err(Error::Stale);
        }
        if self.active.len() >= 64 {
            return Err(Error::Backpressure);
        }
        match frame.frame.sync() {
            FrameSync::None => {} // host has already observed producer completion
            FrameSync::MetalSharedEvent { event, value } => {
                if event.signaledValue() < *value {
                    return Ok(Imported::Pending(frame));
                }
            }
            _ => return Err(Error::Unsupported("non-Metal synchronization".into())),
        }
        let (metadata, image, _) = frame.frame.into_parts();
        if metadata.generation != frame.stamp.allocation_epoch
            || metadata.size != FrameSize::new(cx.viewport.width, cx.viewport.height)
        {
            return Err(Error::Invalid("IOSurface stamp or extent mismatch".into()));
        }
        let OwnedNativeImage::IoSurface(surface) = image else {
            return Err(Error::Unsupported("expected owned IOSurface".into()));
        };
        let host = grafting::HostWgpuContext::new(cx.device.clone(), cx.queue.clone());
        let (texture, release) =
            grafting::metal_surface::import_surface(surface, metadata.size, metadata.format, &host)
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
