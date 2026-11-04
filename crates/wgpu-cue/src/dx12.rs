// SPDX-License-Identifier: MPL-2.0
//! Receiving adapter for host-validated, same-adapter DX12 publications.
use crate::*;
use webcue::grafting_frame::{FrameSync, OwnedNativeImage};
/// The host transports the resource/fence pair and validates execution identity,
/// adapter LUID and lease before import. Seal after the final submitted use,
/// drop all texture/view aliases, and return only `poll_releases` stamps.
/// Device loss or producer exit never grants allocation reuse.
pub struct GraftDx12SurfaceImporter {
    scope: Scope,
    device_epoch: DeviceEpoch,
    last: Option<FrameStamp>,
    active: Vec<(
        FrameStamp,
        Option<wgpu::Texture>,
        grafting::dx12_surface::Dx12Release,
    )>,
}
impl GraftDx12SurfaceImporter {
    pub fn new(scope: Scope, device_epoch: DeviceEpoch) -> Self {
        Self {
            scope,
            device_epoch,
            last: None,
            active: Vec::new(),
        }
    }
    pub fn seal_use(&mut self, stamp: FrameStamp) -> Result<()> {
        let (_, texture, release) = self
            .active
            .iter_mut()
            .find(|(s, _, _)| *s == stamp)
            .ok_or(Error::NotPending)?;
        release
            .seal(texture.as_ref().ok_or(Error::NotPending)?)
            .map_err(|e| Error::Backend(e.to_string()))?;
        // Release our own alias only after recording the COMMON-state barrier.
        texture.take();
        Ok(())
    }
    pub fn in_flight(&self) -> usize {
        self.active.len()
    }
    pub fn poll_releases(&mut self) -> Result<Vec<FrameStamp>> {
        let mut result = Vec::new();
        let mut i = 0;
        while i < self.active.len() {
            if self.active[i]
                .2
                .poll_released()
                .map_err(|e| Error::Backend(e.to_string()))?
            {
                result.push(self.active.remove(i).0);
            } else {
                i += 1;
            }
        }
        Ok(result)
    }
}
impl NativeImporter for GraftDx12SurfaceImporter {
    fn try_import(&mut self, cx: &RenderContext<'_>, frame: NativeFrame) -> Result<Imported> {
        if frame.stamp.scope != self.scope
            || cx.device_epoch != self.device_epoch
            || self.last.is_some_and(|last| {
                frame.stamp.allocation_epoch <= last.allocation_epoch
                    || frame.stamp.content_revision <= last.content_revision
            })
        {
            return Err(Error::Stale);
        }
        if self.active.len() >= 64 {
            return Err(Error::Backpressure);
        }
        let (metadata, image, sync) = frame.frame.into_parts();
        if metadata.generation == 0
            || metadata.generation != frame.stamp.allocation_epoch
            || metadata.size != FrameSize::new(cx.viewport.width, cx.viewport.height)
            || metadata.format != PixelFormat::Rgba8Unorm
        {
            return Err(Error::Invalid(
                "DX12 surface extent/format/stamp mismatch".into(),
            ));
        }
        let OwnedNativeImage::Dx12SharedTexture(image) = image else {
            return Err(Error::Unsupported("owned DX12 resource required".into()));
        };
        let FrameSync::Dx12Fence { fence, value } = sync else {
            return Err(Error::Unsupported(
                "owned DX12 publication fence required".into(),
            ));
        };
        let host = grafting::HostWgpuContext::new(cx.device.clone(), cx.queue.clone());
        let (texture, release) = grafting::dx12_surface::import_surface(
            image.resource().as_handle(),
            fence.as_handle(),
            value,
            metadata.size,
            &host,
        )
        .map_err(|e| Error::Backend(e.to_string()))?;
        self.last = Some(frame.stamp);
        self.active
            .push((frame.stamp, Some(texture.clone()), release));
        Ok(Imported::Ready(TextureFrame {
            texture,
            stamp: frame.stamp,
            device_epoch: cx.device_epoch,
            alpha: frame.alpha,
            encoding: frame.encoding,
        }))
    }
}
