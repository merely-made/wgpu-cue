// SPDX-License-Identifier: MPL-2.0
//! Host-owned render admission and terminal outcomes, independent of placement.
//! Document effects stay with the document owner. Frame/GPU custody survives a
//! render terminal outcome and remains with the presenter and native importer.
use crate::scene_job::{RenderJob, RenderLimits, SceneSnapshot, frame_bytes};
use std::collections::{BTreeMap, VecDeque};
use webcue::{
    execution::{ExecutionId, SendStatus, StopOutcome},
    *,
};

/// Keep this clock with the document installation across renderer replacements.
/// Each output gets a fresh publication/allocation stamp even when rerendering
/// the same document snapshot. Renderer incarnation is a separate identity.
pub struct RenderClock {
    scope: Scope,
    allocation: u64,
    revision: u64,
}
impl RenderClock {
    pub fn new(scope: Scope) -> Self {
        Self {
            scope,
            allocation: 0,
            revision: 0,
        }
    }
    pub fn job(&mut self, revision: u64, snapshot: SceneSnapshot) -> Result<RenderJob> {
        if revision <= self.revision {
            return Err(Error::Stale);
        }
        let allocation = self.allocation.checked_add(1).ok_or(Error::Exhausted)?;
        self.revision = revision;
        self.allocation = allocation;
        Ok(RenderJob {
            stamp: FrameStamp {
                scope: self.scope,
                allocation_epoch: allocation,
                content_revision: revision,
            },
            snapshot,
        })
    }
}
#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub execution: ExecutionId,
    pub job: RenderJob,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderFailure {
    Refused(String),
    Cancelled,
    ExecutionLost(String),
}
pub struct RenderReply<F> {
    pub execution: ExecutionId,
    pub stamp: FrameStamp,
    pub result: std::result::Result<F, RenderFailure>,
}
/// Descriptive checks only. Device binding, native readiness, GPU completion and
/// exported-slot release still belong to their concrete adapters.
pub trait RenderedFrame {
    fn stamp(&self) -> FrameStamp;
    fn size(&self) -> FrameSize;
    fn validate(&self, limits: RenderLimits) -> Result<()>;
}
impl RenderedFrame for CpuFrame {
    fn stamp(&self) -> FrameStamp {
        self.stamp
    }
    fn size(&self) -> FrameSize {
        self.size
    }
    fn validate(&self, limits: RenderLimits) -> Result<()> {
        CpuFrame::validate(self)?;
        if self.rgba.len() > limits.frame_bytes {
            return Err(Error::Invalid("CPU output budget".into()));
        }
        Ok(())
    }
}
impl RenderedFrame for NativeFrame {
    fn stamp(&self) -> FrameStamp {
        self.stamp
    }
    fn size(&self) -> FrameSize {
        self.frame.metadata().size
    }
    fn validate(&self, _: RenderLimits) -> Result<()> {
        if self.frame.metadata().generation != self.stamp.allocation_epoch {
            return Err(Error::Stale);
        }
        Ok(())
    }
}
impl RenderedFrame for wgpu_cue::TextureFrame {
    fn stamp(&self) -> FrameStamp {
        self.stamp
    }
    fn size(&self) -> FrameSize {
        FrameSize::new(self.texture.width(), self.texture.height())
    }
    fn validate(&self, _: RenderLimits) -> Result<()> {
        if self.texture.depth_or_array_layers() != 1 || self.texture.sample_count() != 1 {
            return Err(Error::Invalid(
                "render output must be a single-layer single-sample texture".into(),
            ));
        }
        Ok(())
    }
}
/// Host-supplied, ordered endpoint. Methods never wait for I/O, raster work, GPU
/// completion or executor termination. Busy means nothing was admitted. Bound
/// encoded buffers independently; the session's byte accounting is logical,
/// not process RSS. Drain queued replies before reporting a disconnected peer.
/// No renderer, launcher, IPC implementation or Send bound is selected here.
pub trait RenderTransport {
    type Frame: RenderedFrame;
    fn try_send(&mut self, request: &RenderRequest) -> Result<SendStatus>;
    fn try_recv(&mut self) -> Result<Option<RenderReply<Self::Frame>>>;
    fn request_stop(&mut self) -> Result<()>;
    fn poll_stopped(&mut self) -> Result<Option<StopOutcome>>;
}
struct Reservation {
    stamp: FrameStamp,
    size: FrameSize,
    bytes: usize,
    sent: bool,
}
pub struct RenderSession<T: RenderTransport> {
    execution: ExecutionId,
    transport: T,
    limits: RenderLimits,
    capacity: usize,
    byte_budget: usize,
    bytes: usize,
    last: Option<FrameStamp>,
    outgoing: VecDeque<RenderRequest>,
    pending: BTreeMap<u64, Reservation>,
    replies: VecDeque<(RenderReply<T::Frame>, usize)>,
    closed: bool,
    stop_requested: bool,
    stopped: Option<StopOutcome>,
}
impl<T: RenderTransport> RenderSession<T> {
    pub fn new(
        execution: ExecutionId,
        transport: T,
        capacity: usize,
        byte_budget: usize,
        limits: RenderLimits,
    ) -> Result<Self> {
        if execution.incarnation == 0 || !(1..=64).contains(&capacity) || byte_budget == 0 {
            return Err(Error::Invalid(
                "positive render incarnation, byte budget and capacity 1..=64 required".into(),
            ));
        }
        Ok(Self {
            execution,
            transport,
            limits,
            capacity,
            byte_budget,
            bytes: 0,
            last: None,
            outgoing: VecDeque::new(),
            pending: BTreeMap::new(),
            replies: VecDeque::new(),
            closed: false,
            stop_requested: false,
            stopped: None,
        })
    }
    pub fn execution(&self) -> ExecutionId {
        self.execution
    }
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }
    pub fn accounted_bytes(&self) -> usize {
        self.bytes
    }
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
    pub fn stopped(&self) -> Option<StopOutcome> {
        self.stopped
    }
    pub fn submit(&mut self, job: RenderJob) -> Result<FrameStamp> {
        if self.closed {
            return Err(Error::Retired);
        }
        if job.stamp.scope != self.execution.scope
            || job.stamp.content_revision == 0
            || job.stamp.allocation_epoch == 0
            || self.last.is_some_and(|s| {
                job.stamp.content_revision <= s.content_revision
                    || job.stamp.allocation_epoch <= s.allocation_epoch
            })
        {
            return Err(Error::Stale);
        }
        let size = job.snapshot.size();
        let pixels = frame_bytes(size.width, size.height, self.limits)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        if job.snapshot.accounted_bytes() > self.limits.snapshot_bytes
            || job.snapshot.resource_bytes() > self.limits.resource_bytes
        {
            return Err(Error::Invalid("render input exceeds job limits".into()));
        }
        // Charge source bytes conservatively as well as the encoded snapshot
        // and expected output. Hold the reservation until its outcome is read.
        let charge = job
            .snapshot
            .accounted_bytes()
            .checked_add(job.snapshot.resource_bytes())
            .and_then(|n| n.checked_add(pixels))
            .ok_or(Error::Exhausted)?;
        if self.pending.len() + self.replies.len() >= self.capacity
            || charge > self.byte_budget.saturating_sub(self.bytes)
        {
            return Err(Error::Backpressure);
        }
        self.bytes += charge;
        self.last = Some(job.stamp);
        self.pending.insert(
            job.stamp.content_revision,
            Reservation {
                stamp: job.stamp,
                size,
                bytes: charge,
                sent: false,
            },
        );
        let stamp = job.stamp;
        self.outgoing.push_back(RenderRequest {
            execution: self.execution,
            job,
        });
        Ok(stamp)
    }
    fn finish_pending(&mut self, reason: RenderFailure) {
        self.outgoing.clear();
        for (_, reservation) in std::mem::take(&mut self.pending) {
            self.replies.push_back((
                RenderReply {
                    execution: self.execution,
                    stamp: reservation.stamp,
                    result: Err(reason.clone()),
                },
                reservation.bytes,
            ));
        }
    }
    fn stop(&mut self) -> Result<()> {
        if !self.stop_requested {
            self.stop_requested = true;
            self.transport.request_stop()?;
        }
        Ok(())
    }
    fn lose(&mut self, reason: String) -> Result<()> {
        self.closed = true;
        self.finish_pending(RenderFailure::ExecutionLost(reason));
        self.stop()
    }
    pub fn retire(&mut self) -> Result<()> {
        if !self.closed {
            self.closed = true;
            self.finish_pending(RenderFailure::Cancelled);
        }
        self.stop()
    }
    pub fn advance(&mut self) -> Result<()> {
        if !self.closed {
            for _ in 0..64 {
                let Some(request) = self.outgoing.front() else {
                    break;
                };
                match self.transport.try_send(request) {
                    Ok(SendStatus::Sent) => {
                        self.pending
                            .get_mut(&request.job.stamp.content_revision)
                            .expect("outgoing reservation")
                            .sent = true;
                        self.outgoing.pop_front();
                    }
                    Ok(SendStatus::Busy) => break,
                    Err(e) => {
                        self.lose(e.to_string())?;
                        break;
                    }
                }
            }
        }
        if !self.closed {
            for _ in 0..64 {
                match self.transport.try_recv() {
                    Ok(Some(reply)) => {
                        let expected = self.pending.get(&reply.stamp.content_revision);
                        let valid = reply.execution == self.execution
                            && expected.is_some_and(|r| {
                                if !r.sent || r.stamp != reply.stamp {
                                    return false;
                                }
                                match reply.result.as_ref() {
                                    Ok(frame) => {
                                        frame.stamp() == r.stamp
                                            && frame.size() == r.size
                                            && frame.validate(self.limits).is_ok()
                                    }
                                    Err(_) => true,
                                }
                            });
                        if !valid {
                            self.lose("stale, duplicate or malformed renderer reply".into())?;
                            break;
                        }
                        let reservation = self
                            .pending
                            .remove(&reply.stamp.content_revision)
                            .expect("validated reservation");
                        self.replies.push_back((reply, reservation.bytes));
                    }
                    Ok(None) => break,
                    Err(e) => {
                        self.lose(e.to_string())?;
                        break;
                    }
                }
            }
        }
        self.stopped = self.transport.poll_stopped()?;
        if !self.closed && self.stopped.is_some() {
            self.lose("renderer stopped".into())?;
        }
        Ok(())
    }
    pub fn poll_reply(&mut self) -> Option<RenderReply<T::Frame>> {
        self.replies.pop_front().map(|(reply, bytes)| {
            self.bytes -= bytes;
            reply
        })
    }
}
