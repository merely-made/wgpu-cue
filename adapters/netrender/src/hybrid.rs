// SPDX-License-Identifier: MPL-2.0
//! Host-device Hybrid rendering. Submission owns its encoder and target; frame
//! polling never submits another draw or waits for GPU completion.
use crate::scene_job::{RenderError, RenderLimits, RenderResult, SceneSnapshot, frame_bytes};
use crate::scene_profile::measure;
use netrender::vello_backends::{HybridSession, SparseResourceLimits, SparseTextLimits};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use webcue::*;
use wgpu_cue::{DeviceEpoch, RenderContext, Rendered, TextureFrame, TextureSource, wgpu};

type ErrorScope = Pin<Box<dyn Future<Output = Option<wgpu::Error>>>>;
fn scopes(device: &wgpu::Device) -> [wgpu::ErrorScopeGuard; 3] {
    [
        wgpu::ErrorFilter::OutOfMemory,
        wgpu::ErrorFilter::Internal,
        wgpu::ErrorFilter::Validation,
    ]
    .map(|filter| device.push_error_scope(filter))
}
fn pop(scopes: [wgpu::ErrorScopeGuard; 3]) -> Vec<ErrorScope> {
    scopes
        .into_iter()
        .rev()
        .map(|scope| Box::pin(scope.pop()) as ErrorScope)
        .collect()
}
fn poll_scopes(scopes: &mut Vec<ErrorScope>) -> Option<String> {
    let mut task = Context::from_waker(Waker::noop());
    let mut error = None;
    scopes.retain_mut(|scope| match scope.as_mut().poll(&mut task) {
        Poll::Pending => true,
        Poll::Ready(value) => {
            if let Some(value) = value {
                error = Some(value.to_string());
            }
            false
        }
    });
    error
}
struct Pending {
    frame: TextureFrame,
    done: Arc<AtomicBool>,
    scopes: Vec<ErrorScope>,
    publish: bool,
}

/// A raster-only host-device surface. Document controls and process/resource
/// authority remain with their existing owners. Scale-one complete scenes only.
pub struct HybridSceneSource {
    scope: Scope,
    viewport: Viewport,
    limits: RenderLimits,
    report: CapabilityReport,
    device: wgpu::Device,
    queue: wgpu::Queue,
    epoch: DeviceEpoch,
    session: HybridSession,
    initial_scopes: Vec<ErrorScope>,
    pending: Option<Pending>,
    ready: Option<TextureFrame>,
    last_revision: Option<u64>,
    allocation: u64,
    submissions: u64,
    fault: Option<String>,
    visible: bool,
    retired: bool,
}
impl HybridSceneSource {
    /// The trusted host supplies a device and its associated queue. Identity
    /// checks bind later calls to that pair; they are not a GPU-object sandbox.
    pub fn new(scope: Scope, cx: &RenderContext<'_>, limits: RenderLimits) -> RenderResult<Self> {
        cx.viewport.validate()?;
        frame_bytes(cx.viewport.width, cx.viewport.height, limits)?;
        let report = CapabilityReport::new(
            BackendIdentity {
                engine: "cue.netrender.hybrid".into(),
                version: None,
                platform: std::env::consts::OS.into(),
                build_features: vec!["hybrid".into()],
            },
            PresentationMode::HostDevice,
            Evidence::Constructed,
            SemanticProtocol::Unavailable,
            Feature::ALL.into_iter().map(|f| {
                (
                    f,
                    Support::Unsupported("document controls belong to the document owner".into()),
                )
            }),
        )?;
        let guards = scopes(cx.device);
        let session = HybridSession::new_with_text_limits(
            cx.device,
            cx.queue,
            SparseResourceLimits {
                max_source_bytes: limits.resource_bytes,
                max_cached_bytes: limits.resource_bytes,
                max_viewport_pixels: limits.frame_bytes / 4,
                ..Default::default()
            },
            SparseTextLimits {
                max_font_bytes: limits.resource_bytes,
                ..Default::default()
            },
        );
        let initial_scopes = pop(guards);
        Ok(Self {
            scope,
            viewport: cx.viewport,
            limits,
            report,
            device: cx.device.clone(),
            queue: cx.queue.clone(),
            epoch: cx.device_epoch,
            session: session.map_err(RenderError::Raster)?,
            initial_scopes,
            pending: None,
            ready: None,
            last_revision: None,
            allocation: 0,
            submissions: 0,
            fault: None,
            visible: true,
            retired: false,
        })
    }
    fn live(&self) -> Result<()> {
        if self.retired {
            Err(Error::Retired)
        } else {
            Ok(())
        }
    }
    fn context(&self, cx: &RenderContext<'_>) -> Result<()> {
        if cx.device != &self.device || cx.queue != &self.queue {
            return Err(Error::Invalid(
                "Hybrid belongs to another device/queue".into(),
            ));
        }
        if cx.device_epoch != self.epoch {
            return Err(Error::Stale);
        }
        if cx.viewport.validate()? != self.viewport {
            return Err(Error::Invalid(
                "Hybrid context differs from surface viewport".into(),
            ));
        }
        Ok(())
    }
    fn invalidate(&mut self) {
        self.ready = None;
        if let Some(pending) = &mut self.pending {
            pending.publish = false;
        }
    }
    fn poll_gpu(&mut self) -> Result<()> {
        self.device
            .poll(wgpu::PollType::Poll)
            .map_err(|error| Error::Backend(error.to_string()))?;
        if let Some(error) = poll_scopes(&mut self.initial_scopes) {
            self.fault = Some(error);
        }
        if let Some(pending) = &mut self.pending {
            if let Some(error) = poll_scopes(&mut pending.scopes) {
                self.fault = Some(error);
                pending.publish = false;
            }
            if pending.done.load(Ordering::Acquire) && pending.scopes.is_empty() {
                let pending = self.pending.take().expect("observed pending submission");
                if pending.publish && self.visible && !self.retired && self.fault.is_none() {
                    self.ready = Some(pending.frame);
                }
            }
        }
        if let Some(error) = &self.fault {
            return Err(Error::Backend(error.clone()));
        }
        Ok(())
    }
    /// May be polled after retirement to observe completion of retained work.
    /// This observes this adapter's submission, not all resource custody in a host.
    pub fn poll_idle(&mut self) -> Result<bool> {
        self.poll_gpu()?;
        Ok(self.pending.is_none() && self.initial_scopes.is_empty())
    }
    fn track(
        &mut self,
        texture: wgpu::Texture,
        stamp: FrameStamp,
        guards: [wgpu::ErrorScopeGuard; 3],
        publish: bool,
    ) {
        let scopes = pop(guards);
        let done = Arc::new(AtomicBool::new(false));
        let callback = done.clone();
        self.queue
            .on_submitted_work_done(move || callback.store(true, Ordering::Release));
        self.pending = Some(Pending {
            frame: TextureFrame {
                texture,
                stamp,
                device_epoch: self.epoch,
                alpha: Alpha::Premultiplied,
                encoding: Encoding::Srgb,
            },
            done,
            scopes,
            publish,
        });
    }
    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }
    /// Successfully admitted draws; cleanup fences after refusals are excluded.
    pub fn submitted_count(&self) -> u64 {
        self.submissions
    }

    /// Explicit bounded CPU preparation, encoding and immediate submission. The
    /// adapter creates all GPU objects on its owned device. An outstanding job
    /// returns Backpressure until the host explicitly polls completion.
    pub fn submit(
        &mut self,
        cx: &RenderContext<'_>,
        revision: u64,
        snapshot: &SceneSnapshot,
    ) -> RenderResult<FrameStamp> {
        measure("hybrid.submit_total", snapshot.resource_bytes(), || {
            self.submit_inner(cx, revision, snapshot)
        })
    }
    fn submit_inner(
        &mut self,
        cx: &RenderContext<'_>,
        revision: u64,
        snapshot: &SceneSnapshot,
    ) -> RenderResult<FrameStamp> {
        self.live()?;
        self.context(cx)?;
        if !self.visible {
            return Err(Error::Invalid("hidden Hybrid endpoint".into()).into());
        }
        if self.pending.is_some() {
            return Err(Error::Backpressure.into());
        }
        self.poll_gpu()?;
        if !self.initial_scopes.is_empty() {
            return Err(Error::Backpressure.into());
        }
        if self.viewport.scale != 1.0 {
            return Err(
                Error::Unsupported("Hybrid scene jobs currently require scale one".into()).into(),
            );
        }
        if snapshot.size() != FrameSize::new(self.viewport.width, self.viewport.height) {
            return Err(Error::Invalid("scene extent differs from viewport".into()).into());
        }
        if self.last_revision.is_some_and(|last| revision <= last) {
            return Err(Error::Stale.into());
        }
        let allocation = self.allocation.checked_add(1).ok_or(Error::Exhausted)?;
        let submissions = self.submissions.checked_add(1).ok_or(Error::Exhausted)?;
        frame_bytes(self.viewport.width, self.viewport.height, self.limits)?;
        let scene = measure("hybrid.decode", snapshot.resource_bytes(), || {
            snapshot.decode(self.limits)
        })?;
        if scene.viewport_width > self.device.limits().max_texture_dimension_2d
            || scene.viewport_height > self.device.limits().max_texture_dimension_2d
        {
            return Err(Error::Invalid("Hybrid scene exceeds device texture limits".into()).into());
        }
        measure("hybrid.clear_images", 0, || self.session.clear_images());
        measure("hybrid.clear_text", 0, || self.session.clear_text_cache());
        let guards = scopes(&self.device);
        let texture = measure("hybrid.create_target", 0, || {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Cue Hybrid scene"),
                size: wgpu::Extent3d {
                    width: scene.viewport_width,
                    height: scene.viewport_height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        });
        let mut encoder = measure("hybrid.create_encoder", 0, || {
            self.device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Cue Hybrid submission"),
                })
        });
        let result = measure("hybrid.session_render", 0, || {
            self.session
                .render(&scene, &self.device, &self.queue, &mut encoder, &texture)
        });
        let stamp = FrameStamp {
            scope: self.scope,
            allocation_epoch: allocation,
            content_revision: revision,
        };
        if let Err(error) = result {
            // A late provider error can follow staged image uploads. Flush and
            // fence those writes even though no draw was admitted. Revision and
            // draw counters remain unchanged; this target can never publish.
            self.queue.submit([]);
            self.track(texture, stamp, guards, false);
            return Err(RenderError::Raster(error));
        }
        // This MUST precede another HybridSession::render: provider queue writes
        // mutate shared buffers used by the encoder just recorded.
        let commands = measure("hybrid.finish_encoder", 0, || encoder.finish());
        measure("hybrid.queue_submit", 0, || self.queue.submit([commands]));
        measure("hybrid.track_completion", 0, || {
            self.track(texture, stamp, guards, true)
        });
        self.ready = None;
        self.allocation = allocation;
        self.submissions = submissions;
        self.last_revision = Some(revision);
        Ok(stamp)
    }
}
impl TextureSource for HybridSceneSource {
    fn render(&mut self, cx: &RenderContext<'_>, needs_frame: bool) -> Result<Rendered> {
        self.live()?;
        self.context(cx)?;
        self.poll_gpu()?;
        if let Some(frame) = self.ready.take() {
            return Ok(Rendered::Changed(frame));
        }
        Ok(if self.pending.is_some() || needs_frame {
            Rendered::Pending
        } else {
            Rendered::Unchanged
        })
    }
}
impl Surface for HybridSceneSource {
    fn scope(&self) -> Scope {
        self.scope
    }
    fn capabilities(&self) -> &CapabilityReport {
        &self.report
    }
    fn set_viewport(&mut self, viewport: Viewport) -> Result<()> {
        self.live()?;
        let viewport = viewport.validate()?;
        if viewport != self.viewport {
            self.viewport = viewport;
            self.invalidate();
        }
        Ok(())
    }
    fn set_visible(&mut self, visible: bool) -> Result<()> {
        self.live()?;
        self.visible = visible;
        if !visible {
            self.invalidate();
        }
        Ok(())
    }
    fn input(&mut self, _: Input) -> Result<()> {
        self.live()?;
        Err(Error::Unsupported(
            "raster endpoint has no input owner".into(),
        ))
    }
    fn request(&mut self, _: RequestId, _: Command) -> Result<RequestKey> {
        self.live()?;
        Err(Error::Unsupported(
            "raster endpoint has no document command owner".into(),
        ))
    }
    fn semantic_action(&mut self, _: SemanticAction) -> Result<()> {
        self.live()?;
        Err(Error::Unsupported(
            "raster endpoint has no semantic owner".into(),
        ))
    }
    fn advance(&mut self, _: Duration) -> Result<()> {
        self.live()
    }
    fn poll_event(&mut self) -> Option<Envelope> {
        None
    }
    fn retire(&mut self) -> Result<()> {
        self.retired = true;
        self.invalidate();
        Ok(())
    }
}
