// SPDX-License-Identifier: MPL-2.0
//! CPU rasterization and a nonblocking CPU-frame receiving endpoint. The host
//! supplies scheduling and process policy; document controls retain their owner.
use crate::scene_job::frame_bytes;
pub use crate::scene_job::{RenderError, RenderJob, RenderLimits, RenderResult, SceneSnapshot};
use netrender::vello_backends::{CpuSession, SparseResourceLimits, SparseTextLimits};
use std::time::Duration;
use webcue::*;

/// Explicit blocking CPU raster operation. Schedule it outside frame acquisition.
/// A fresh session per full snapshot prevents resources from accumulating across
/// jobs and makes resource-key reuse independent of previous jobs.
pub fn rasterize(job: &RenderJob, limits: RenderLimits) -> RenderResult<CpuFrame> {
    let scene = job.snapshot.decode(limits)?;
    let bytes = frame_bytes(scene.viewport_width, scene.viewport_height, limits)?;
    let images = SparseResourceLimits {
        max_source_bytes: limits.resource_bytes,
        max_cached_bytes: limits.resource_bytes,
        max_viewport_pixels: limits.frame_bytes / 4,
        ..Default::default()
    };
    let text = SparseTextLimits {
        max_font_bytes: limits.resource_bytes,
        ..Default::default()
    };
    let mut session = CpuSession::new_with_text_limits(images, text);
    let pixmap = session.render(&scene).map_err(RenderError::Raster)?;
    let rgba = pixmap.data_as_u8_slice().to_vec();
    if rgba.len() != bytes {
        return Err(RenderError::Budget("unexpected raster extent"));
    }
    let frame = CpuFrame {
        stamp: job.stamp,
        size: job.snapshot.size(),
        stride: scene
            .viewport_width
            .checked_mul(4)
            .ok_or(RenderError::Budget("stride"))?,
        rgba,
        alpha: Alpha::Premultiplied,
        encoding: Encoding::Srgb,
    };
    frame.validate()?;
    Ok(frame)
}

/// Host-owned one-frame inbox implementing `FrameSource`. Controls belong to the
/// document owner; this raster endpoint advertises no document/input features.
/// The host bounds external job queues and supplies executor supervision.
pub struct CpuSceneSource {
    scope: Scope,
    viewport: Viewport,
    limits: RenderLimits,
    report: CapabilityReport,
    last_revision: Option<u64>,
    allocation: u64,
    expected: Option<FrameStamp>,
    frame: Option<CpuFrame>,
    visible: bool,
    retired: bool,
}
impl CpuSceneSource {
    pub fn new(scope: Scope, viewport: Viewport, limits: RenderLimits) -> Result<Self> {
        let report = CapabilityReport::new(
            BackendIdentity {
                engine: "cue.netrender.cpu".into(),
                version: None,
                platform: std::env::consts::OS.into(),
                build_features: vec!["cpu".into()],
            },
            PresentationMode::CpuRgba,
            Evidence::Constructed,
            SemanticProtocol::Unavailable,
            Feature::ALL.into_iter().map(|feature| {
                (
                    feature,
                    Support::Unsupported("document controls belong to the document owner".into()),
                )
            }),
        )?;
        Ok(Self {
            scope,
            viewport: viewport.validate()?,
            limits,
            report,
            last_revision: None,
            allocation: 0,
            expected: None,
            frame: None,
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
    fn invalidate(&mut self) {
        self.expected = None;
        self.frame = None;
    }
    pub fn job(&mut self, revision: u64, snapshot: SceneSnapshot) -> RenderResult<RenderJob> {
        self.live()?;
        if !self.visible {
            return Err(Error::Invalid("hidden raster endpoint".into()).into());
        }
        if self.viewport.scale != 1.0 {
            return Err(
                Error::Unsupported("CPU scene jobs currently require scale one".into()).into(),
            );
        }
        if snapshot.size() != FrameSize::new(self.viewport.width, self.viewport.height) {
            return Err(Error::Invalid("scene extent differs from viewport".into()).into());
        }
        if snapshot.bytes().len() > self.limits.snapshot_bytes {
            return Err(RenderError::Budget("snapshot bytes"));
        }
        if snapshot.resource_bytes() > self.limits.resource_bytes {
            return Err(RenderError::Budget("font/image source bytes"));
        }
        frame_bytes(self.viewport.width, self.viewport.height, self.limits)?;
        if self.last_revision.is_some_and(|old| revision <= old) {
            return Err(Error::Stale.into());
        }
        let allocation = self.allocation.checked_add(1).ok_or(Error::Exhausted)?;
        let stamp = FrameStamp {
            scope: self.scope,
            allocation_epoch: allocation,
            content_revision: revision,
        };
        self.last_revision = Some(revision);
        self.allocation = allocation;
        self.expected = Some(stamp);
        self.frame = None;
        Ok(RenderJob { stamp, snapshot })
    }
    pub fn complete(&mut self, frame: CpuFrame) -> RenderResult<()> {
        self.live()?;
        if self.expected != Some(frame.stamp) {
            return Err(Error::Stale.into());
        }
        frame.validate()?;
        if frame.size != FrameSize::new(self.viewport.width, self.viewport.height)
            || frame.alpha != Alpha::Premultiplied
            || frame.encoding != Encoding::Srgb
        {
            return Err(Error::Invalid(
                "CPU frame extent or pixel convention differs from contract".into(),
            )
            .into());
        }
        if frame.rgba.len() > self.limits.frame_bytes {
            return Err(RenderError::Budget("frame bytes"));
        }
        self.expected = None;
        self.frame = Some(frame);
        Ok(())
    }
}
impl Surface for CpuSceneSource {
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
impl FrameSource for CpuSceneSource {
    fn try_acquire_frame(&mut self) -> Result<Option<Frame>> {
        self.live()?;
        Ok(self.frame.take().map(Frame::Cpu))
    }
}
