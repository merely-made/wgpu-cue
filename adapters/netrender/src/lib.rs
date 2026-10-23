// SPDX-License-Identifier: MPL-2.0
//! A private retained renderer on the embedding host's existing GPU device.
//! Quality counters belong to this renderer, never to the host's renderer or
//! another producer. Polling does not wait for GPU work or mapping callbacks.
pub use netrender::{Scene, WgpuHandles};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, Waker},
};
use webcue::{Alpha, Encoding, Error, FrameStamp, Result};
use wgpu_cue::{DeviceEpoch, RenderContext, Rendered, TextureFrame, wgpu};
type ErrorScope = Pin<Box<dyn Future<Output = Option<wgpu::Error>>>>;
struct Pending {
    frame: TextureFrame,
    viewport: webcue::Viewport,
    scopes: Vec<ErrorScope>,
}
pub struct SceneRasterizer {
    renderer: netrender::Renderer,
    pending: Option<Pending>,
    epoch: Option<DeviceEpoch>,
}
impl SceneRasterizer {
    pub fn new(handles: WgpuHandles) -> Result<Self> {
        let renderer = netrender::create_netrender_instance(
            handles,
            netrender::NetrenderOptions {
                enable_vello: true,
                tile_cache_size: Some(256),
                ..Default::default()
            },
        )
        .map_err(|e| Error::Backend(format!("private retained renderer: {e:?}")))?;
        Ok(Self {
            renderer,
            pending: None,
            epoch: None,
        })
    }
    pub fn suspend(&mut self) {
        self.pending = None;
    }
    /// Poll an already submitted target. None means that new scene preparation
    /// may proceed; Pending means no blocking wait and no fresh submission.
    pub fn poll(&mut self, cx: &RenderContext<'_>) -> Result<Option<Rendered>> {
        cx.viewport.validate()?;
        if cx.device != &self.renderer.wgpu_device.core.device
            || cx.queue != &self.renderer.wgpu_device.core.queue
        {
            return Err(Error::Invalid(
                "renderer belongs to a different host device/queue".into(),
            ));
        }
        let max = cx.device.limits().max_texture_dimension_2d;
        if cx.viewport.width > max || cx.viewport.height > max {
            return Err(Error::Invalid(
                "viewport exceeds host texture limits".into(),
            ));
        }
        if self.epoch.is_some_and(|epoch| epoch != cx.device_epoch) {
            self.pending = None;
        }
        self.epoch = Some(cx.device_epoch);
        cx.device
            .poll(wgpu::PollType::Poll)
            .map_err(|e| Error::Backend(e.to_string()))?;
        match self.renderer.poll_vello_curve_quality() {
            Err(e) => {
                self.pending = None;
                return Err(Error::Backend(format!("retained frame quality: {e}")));
            }
            Ok(None) => {
                self.pending = None;
                return Err(Error::Unsupported(
                    "Classic retained renderer is unavailable".into(),
                ));
            }
            Ok(Some((0, 0))) => {}
            Ok(Some(_)) => return Ok(Some(Rendered::Pending)),
        }
        if let Some(pending) = &mut self.pending {
            let mut task = Context::from_waker(Waker::noop());
            let mut waiting = false;
            let mut error = None;
            pending
                .scopes
                .retain_mut(|scope| match scope.as_mut().poll(&mut task) {
                    Poll::Pending => {
                        waiting = true;
                        true
                    }
                    Poll::Ready(Some(e)) => {
                        error = Some(e);
                        false
                    }
                    Poll::Ready(None) => false,
                });
            if let Some(error) = error {
                self.pending = None;
                return Err(Error::Backend(error.to_string()));
            }
            if waiting {
                return Ok(Some(Rendered::Pending));
            }
            let submitted = self.pending.take().unwrap();
            let matching_viewport = submitted.viewport == cx.viewport;
            let pending = submitted.frame;
            if matching_viewport
                && pending.texture.width() == cx.viewport.width
                && pending.texture.height() == cx.viewport.height
            {
                return Ok(Some(Rendered::Changed(pending)));
            }
        }
        Ok(None)
    }
    /// `content_revision` must change when layout/content changes. Allocations
    /// are independently stamped here because every submission creates a target.
    pub fn render(
        &mut self,
        cx: &RenderContext<'_>,
        scene: &Scene,
        mut stamp: FrameStamp,
    ) -> Result<Rendered> {
        if self.pending.as_ref().is_some_and(|pending| {
            pending.frame.stamp.scope != stamp.scope
                || pending.frame.stamp.content_revision != stamp.content_revision
        }) {
            self.pending = None;
        }
        if let Some(rendered) = self.poll(cx)? {
            return Ok(rendered);
        }
        static NEXT_ALLOCATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        #[allow(deprecated)] // Preserve the shared MSRV's atomic spelling.
        let allocation = NEXT_ALLOCATION
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |value| value.checked_add(1),
            )
            .map_err(|_| Error::Exhausted)?;
        stamp.allocation_epoch = allocation;
        let scopes = [
            wgpu::ErrorFilter::OutOfMemory,
            wgpu::ErrorFilter::Internal,
            wgpu::ErrorFilter::Validation,
        ]
        .map(|filter| cx.device.push_error_scope(filter));
        let texture = cx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Cue retained content"),
            size: wgpu::Extent3d {
                width: cx.viewport.width,
                height: cx.viewport.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        // Surface key 1 is private to this renderer. Its allocation/cache cannot
        // collide with another producer or the host's retained surfaces.
        let submitted = self.renderer.render_vello_scaled_for_checked(
            1,
            scene,
            &view,
            netrender::ColorLoad::Clear(wgpu::Color::TRANSPARENT),
            cx.viewport.scale,
        );
        let futures = scopes
            .into_iter()
            .rev()
            .map(|scope| Box::pin(scope.pop()) as ErrorScope)
            .collect();
        submitted.map_err(|e| Error::Backend(format!("retained frame submission: {e}")))?;
        self.pending = Some(Pending {
            viewport: cx.viewport,
            frame: TextureFrame {
                texture,
                stamp,
                device_epoch: cx.device_epoch,
                alpha: Alpha::Straight,
                encoding: Encoding::Srgb,
            },
            scopes: futures,
        });
        Ok(Rendered::Pending)
    }
}
