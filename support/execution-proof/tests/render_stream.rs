// SPDX-License-Identifier: MPL-2.0
#![cfg(feature = "cpu-raster")]
use cue_execution_proof::render_stream::{LocalCpu, ProcessCpu};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use webcue::{execution::*, *};
use wgpu_cue_genet::{GenetDocument, HeldResources};
use wgpu_cue_netrender::{render_session::*, scene_job::*};
fn scope() -> Scope {
    Scope {
        owner: (1u128 << 100) + 121,
        generation: 1,
    }
}
fn id(incarnation: u64) -> ExecutionId {
    ExecutionId {
        scope: scope(),
        incarnation,
    }
}
fn viewport() -> Viewport {
    Viewport {
        width: 160,
        height: 96,
        scale: 1.0,
    }
}
fn html(color: &str, text: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0;background:{color};color:white;font:16px Arial}}</style><p>{text}</p>"
    )
}
fn snapshot(doc: &mut GenetDocument, v: Viewport) -> SceneSnapshot {
    SceneSnapshot::capture(&doc.prepare(v).unwrap(), RenderLimits::default()).unwrap()
}
fn endpoint<T: RenderTransport>(execution: ExecutionId, transport: T) -> RenderSession<T> {
    RenderSession::new(
        execution,
        transport,
        2,
        32 * 1024 * 1024,
        RenderLimits::default(),
    )
    .unwrap()
}
fn wait<T: RenderTransport>(session: &mut RenderSession<T>) -> RenderReply<T::Frame> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        session.advance().unwrap();
        if let Some(reply) = session.poll_reply() {
            return reply;
        }
        assert!(Instant::now() < deadline, "renderer deadline");
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn changing_genet_frames_resize_and_restart_share_local_process_contract() {
    let executable = Path::new(env!("CARGO_BIN_EXE_cue-execution-proof"));
    let mut local = endpoint(id(1), LocalCpu::new().unwrap());
    let mut process = endpoint(id(1), ProcessCpu::spawn(executable, false).unwrap());
    let pid = process.transport_mut().pid();
    assert_ne!(pid, std::process::id());
    let mut doc = GenetDocument::new(
        viewport(),
        "https://fixture.invalid/stream".into(),
        html("#204080", "Alpha"),
        HeldResources::default(),
    )
    .unwrap();
    let mut clock = RenderClock::new(scope());
    let mut held = Vec::new();
    for revision in 1..=3 {
        let size = if revision == 3 {
            Viewport {
                width: 192,
                height: 112,
                ..viewport()
            }
        } else {
            viewport()
        };
        if revision == 2 {
            doc.execute(
                Command::LoadHtml {
                    address: "https://fixture.invalid/stream".into(),
                    html: html("#802040", "Beta"),
                },
                viewport(),
            )
            .unwrap();
        }
        let job = clock.job(revision, snapshot(&mut doc, size)).unwrap();
        local.submit(job.clone()).unwrap();
        process.submit(job).unwrap();
        let a = wait(&mut local).result.unwrap();
        let b = wait(&mut process).result.unwrap();
        assert_eq!(a.rgba, b.rgba);
        assert_eq!(a.stamp, b.stamp);
        assert_eq!(b.size, FrameSize::new(size.width, size.height));
        assert_eq!(process.transport_mut().pid(), pid);
        held.push(b);
    }
    assert_ne!(held[0].rgba, held[1].rgba);
    assert_eq!(held[0].size, FrameSize::new(160, 96));
    // Exercise a deterministic backend refusal independently of system fonts.
    let mut invalid = wgpu_cue_netrender::Scene::new(160, 96);
    invalid.push_rect(0.0, 0.0, 16.0, 16.0, [1.0, 0.0, 0.0, 0.25]);
    let refused = clock
        .job(
            4,
            SceneSnapshot::capture(&invalid, RenderLimits::default()).unwrap(),
        )
        .unwrap();
    local.submit(refused.clone()).unwrap();
    process.submit(refused).unwrap();
    assert!(matches!(
        wait(&mut local).result,
        Err(RenderFailure::Refused(_))
    ));
    assert!(matches!(
        wait(&mut process).result,
        Err(RenderFailure::Refused(_))
    ));
    process.retire().unwrap();
    drop(process);
    let mut restarted = endpoint(id(2), ProcessCpu::spawn(executable, false).unwrap());
    let job = clock.job(5, snapshot(&mut doc, viewport())).unwrap();
    assert!(job.stamp.allocation_epoch > held[2].stamp.allocation_epoch);
    local.submit(job.clone()).unwrap();
    restarted.submit(job).unwrap();
    let a = wait(&mut local).result.unwrap();
    let b = wait(&mut restarted).result.unwrap();
    assert_eq!(a.rgba, b.rgba);
    assert_eq!(b.stamp.scope, scope());
    assert_ne!(restarted.transport_mut().pid(), pid);
    let report = doc
        .execute(
            Command::Find {
                text: "Beta".into(),
                backwards: false,
            },
            viewport(),
        )
        .unwrap();
    let Reply::Find(found) = report.reply else {
        panic!("expected preserved document")
    };
    assert_eq!(found.count, 1);
    local.retire().unwrap();
    restarted.retire().unwrap();
}
#[test]
fn held_renderer_is_terminated_and_accepted_jobs_are_never_replayed() {
    let mut process = endpoint(
        id(4),
        ProcessCpu::spawn(Path::new(env!("CARGO_BIN_EXE_cue-execution-proof")), true).unwrap(),
    );
    let mut doc = GenetDocument::new(
        viewport(),
        "https://fixture.invalid/hold".into(),
        html("#204080", "Held"),
        HeldResources::default(),
    )
    .unwrap();
    let mut clock = RenderClock::new(scope());
    let job = clock.job(1, snapshot(&mut doc, viewport())).unwrap();
    process.submit(job).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !process.transport_mut().started(1) {
        process.advance().unwrap();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    process.transport_mut().kill().unwrap();
    let reply = wait(&mut process);
    assert!(matches!(reply.result, Err(RenderFailure::ExecutionLost(_))));
    assert!(process.poll_reply().is_none());
    while process.stopped().is_none() {
        process.advance().unwrap();
        assert!(Instant::now() < deadline);
    }
    assert_eq!(process.stopped(), Some(StopOutcome::Terminated));
    assert!(matches!(
        process.submit(clock.job(2, snapshot(&mut doc, viewport())).unwrap()),
        Err(Error::Retired)
    ));
}

#[test]
fn natural_exit_is_not_reclassified_as_forced_stop() {
    let mut session = endpoint(
        id(9),
        ProcessCpu::spawn(Path::new(env!("CARGO_BIN_EXE_cue-execution-proof")), false).unwrap(),
    );
    session.transport_mut().finish_input();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(outcome) = session.transport_mut().poll_stopped().unwrap() {
            assert_eq!(outcome, StopOutcome::Exited);
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    session.advance().unwrap();
    assert_eq!(session.stopped(), Some(StopOutcome::Exited));
}
