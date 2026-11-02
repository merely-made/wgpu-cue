// SPDX-License-Identifier: MPL-2.0
#![cfg(feature = "scene-jobs")]
use std::collections::VecDeque;
use webcue::{execution::*, *};
use wgpu_cue_netrender::{Scene, render_session::*, scene_job::*};
struct Transport {
    replies: VecDeque<Result<RenderReply<CpuFrame>>>,
    sent: usize,
    busy: bool,
    stop: bool,
}
impl RenderTransport for Transport {
    type Frame = CpuFrame;
    fn try_send(&mut self, _: &RenderRequest) -> Result<SendStatus> {
        if self.busy {
            Ok(SendStatus::Busy)
        } else {
            self.sent += 1;
            Ok(SendStatus::Sent)
        }
    }
    fn try_recv(&mut self) -> Result<Option<RenderReply<CpuFrame>>> {
        self.replies.pop_front().transpose()
    }
    fn request_stop(&mut self) -> Result<()> {
        self.stop = true;
        Ok(())
    }
    fn poll_stopped(&mut self) -> Result<Option<StopOutcome>> {
        Ok(None)
    }
}
fn id(incarnation: u64) -> ExecutionId {
    ExecutionId {
        scope: Scope {
            owner: 99,
            generation: 2,
        },
        incarnation,
    }
}
fn snapshot() -> SceneSnapshot {
    SceneSnapshot::capture(&Scene::new(2, 2), RenderLimits::default()).unwrap()
}
fn job(revision: u64) -> RenderJob {
    RenderJob {
        stamp: FrameStamp {
            scope: id(1).scope,
            allocation_epoch: revision,
            content_revision: revision,
        },
        snapshot: snapshot(),
    }
}
fn session(capacity: usize, bytes: usize) -> RenderSession<Transport> {
    RenderSession::new(
        id(1),
        Transport {
            replies: VecDeque::new(),
            sent: 0,
            busy: false,
            stop: false,
        },
        capacity,
        bytes,
        RenderLimits::default(),
    )
    .unwrap()
}
fn reply(execution: ExecutionId, stamp: FrameStamp) -> RenderReply<CpuFrame> {
    RenderReply {
        execution,
        stamp,
        result: Ok(CpuFrame {
            stamp,
            size: FrameSize::new(2, 2),
            stride: 8,
            rgba: vec![0; 16],
            alpha: Alpha::Premultiplied,
            encoding: Encoding::Srgb,
        }),
    }
}
#[test]
fn terminal_reservation_holds_capacity_and_bytes_until_read() {
    let mut s = session(1, 1_000_000);
    let a = s.submit(job(1)).unwrap();
    let bytes = s.accounted_bytes();
    s.advance().unwrap();
    s.transport_mut().replies.push_back(Ok(reply(id(1), a)));
    s.advance().unwrap();
    assert_eq!(s.pending(), 0);
    assert_eq!(s.accounted_bytes(), bytes);
    assert_eq!(s.submit(job(2)), Err(Error::Backpressure));
    assert!(s.poll_reply().unwrap().result.is_ok());
    assert_eq!(s.accounted_bytes(), 0);
    s.submit(job(2)).unwrap();
}
#[test]
fn byte_budget_refusal_consumes_neither_capacity_nor_stamp() {
    let mut s = session(2, 1);
    assert_eq!(s.submit(job(1)), Err(Error::Backpressure));
    assert_eq!(s.pending(), 0);
    assert_eq!(s.accounted_bytes(), 0);
}
#[test]
fn busy_retirement_cancels_once_without_inventing_death() {
    let mut s = session(2, 1_000_000);
    s.transport_mut().busy = true;
    s.submit(job(1)).unwrap();
    s.advance().unwrap();
    assert_eq!(s.transport_mut().sent, 0);
    s.retire().unwrap();
    s.retire().unwrap();
    assert!(matches!(
        s.poll_reply().unwrap().result,
        Err(RenderFailure::Cancelled)
    ));
    assert!(s.poll_reply().is_none());
    assert_eq!(s.stopped(), None);
    assert!(s.transport_mut().stop);
    assert_eq!(s.submit(job(2)), Err(Error::Retired));
}
#[test]
fn old_incarnation_cannot_complete_current_jobs() {
    let mut s = session(2, 1_000_000);
    let stamp = s.submit(job(1)).unwrap();
    s.advance().unwrap();
    s.transport_mut().replies.push_back(Ok(reply(id(2), stamp)));
    s.advance().unwrap();
    assert!(matches!(
        s.poll_reply().unwrap().result,
        Err(RenderFailure::ExecutionLost(_))
    ));
    assert!(s.poll_reply().is_none());
    assert_eq!(s.submit(job(2)), Err(Error::Retired));
}
#[test]
fn duplicate_reply_closes_admission_and_settles_other_jobs_once() {
    let mut s = session(3, 1_000_000);
    let a = s.submit(job(1)).unwrap();
    let b = s.submit(job(2)).unwrap();
    s.advance().unwrap();
    s.transport_mut()
        .replies
        .extend([Ok(reply(id(1), a)), Ok(reply(id(1), a))]);
    s.advance().unwrap();
    assert!(s.poll_reply().unwrap().result.is_ok());
    let other = s.poll_reply().unwrap();
    assert_eq!(other.stamp, b);
    assert!(matches!(other.result, Err(RenderFailure::ExecutionLost(_))));
    assert!(s.poll_reply().is_none());
    assert_eq!(s.transport_mut().sent, 2);
}
#[test]
fn malformed_frame_does_not_publish() {
    let mut s = session(1, 1_000_000);
    let a = s.submit(job(1)).unwrap();
    s.advance().unwrap();
    let mut result = reply(id(1), a);
    result.result.as_mut().unwrap().size = FrameSize::new(1, 4);
    s.transport_mut().replies.push_back(Ok(result));
    s.advance().unwrap();
    assert!(matches!(
        s.poll_reply().unwrap().result,
        Err(RenderFailure::ExecutionLost(_))
    ));
}
#[test]
fn reply_before_transport_admission_is_rejected() {
    let mut s = session(1, 1_000_000);
    s.transport_mut().busy = true;
    let a = s.submit(job(1)).unwrap();
    s.transport_mut().replies.push_back(Ok(reply(id(1), a)));
    s.advance().unwrap();
    assert!(matches!(
        s.poll_reply().unwrap().result,
        Err(RenderFailure::ExecutionLost(_))
    ));
}
#[test]
fn renderer_clock_survives_incarnation_changes_in_one_document_scope() {
    let mut clock = RenderClock::new(id(1).scope);
    let a = clock.job(1, snapshot()).unwrap();
    let mut s = session(1, 1_000_000);
    s.submit(a.clone()).unwrap();
    s.retire().unwrap();
    let b = clock.job(2, snapshot()).unwrap();
    assert_eq!(a.stamp.scope, b.stamp.scope);
    assert!(b.stamp.allocation_epoch > a.stamp.allocation_epoch);
    assert!(clock.job(2, snapshot()).is_err());
    let t = Transport {
        replies: VecDeque::new(),
        sent: 0,
        busy: false,
        stop: false,
    };
    let mut next = RenderSession::new(id(2), t, 1, 1_000_000, RenderLimits::default()).unwrap();
    next.submit(b).unwrap();
}
#[test]
fn explicit_backend_refusal_is_terminal_without_closing_session() {
    let mut s = session(1, 1_000_000);
    let a = s.submit(job(1)).unwrap();
    s.advance().unwrap();
    s.transport_mut().replies.push_back(Ok(RenderReply {
        execution: id(1),
        stamp: a,
        result: Err(RenderFailure::Refused("sampling".into())),
    }));
    s.advance().unwrap();
    assert!(matches!(
        s.poll_reply().unwrap().result,
        Err(RenderFailure::Refused(_))
    ));
    s.submit(job(2)).unwrap();
}
