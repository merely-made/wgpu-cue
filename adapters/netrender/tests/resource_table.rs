// SPDX-License-Identifier: MPL-2.0
#![cfg(feature = "scene-jobs")]
use webcue::{Scope, execution::ExecutionId};
use wgpu_cue_netrender::{Scene, resource_table::*, scene_job::*};
fn execution(incarnation: u64) -> ExecutionId {
    ExecutionId {
        scope: Scope {
            owner: 42,
            generation: 3,
        },
        incarnation,
    }
}
fn snapshot() -> SceneSnapshot {
    let mut s = Scene::new(32, 32);
    s.push_image(
        0.,
        0.,
        8.,
        8.,
        1,
        wgpu_cue_netrender::netrender::ImageData::from_bytes(1, 1, vec![1, 2, 3, 4]),
    );
    SceneSnapshot::capture_owned(s, RenderLimits::default()).unwrap()
}
#[test]
fn resources_travel_once_and_snapshot_custody_outlives_table() {
    let mut tx = ResourceSender::new(execution(1), RenderLimits::default()).unwrap();
    let mut rx = ResourceReceiver::new(execution(1), RenderLimits::default()).unwrap();
    let s = snapshot();
    let (first, next) = tx.prepare(&s).unwrap();
    assert_eq!(first.uploads.len(), 1);
    // Busy did not commit: retry produces the same resource packet/sequence.
    let (retry, _) = tx.prepare(&s).unwrap();
    assert_eq!(first.sequence, retry.sequence);
    assert_eq!(first.uploads[0].token, retry.uploads[0].token);
    let held = rx.receive(first).unwrap();
    tx = next;
    let (second, _) = tx.prepare(&s).unwrap();
    assert!(second.uploads.is_empty());
    assert_eq!(second.sequence, 2);
    let second = rx.receive(second).unwrap();
    let a = held.decode(RenderLimits::default()).unwrap();
    let b = second.decode(RenderLimits::default()).unwrap();
    assert_eq!(
        a.image_sources[&1].data.as_ref().as_ptr(),
        b.image_sources[&1].data.as_ref().as_ptr()
    );
    assert_ne!(a.image_sources[&1].data.id(), b.image_sources[&1].data.id());
    assert_eq!(rx.retained_bytes(), 4);
    drop(rx);
    drop(tx);
    drop(second);
    assert_eq!(a.image_sources[&1].data.as_ref(), &[1, 2, 3, 4]);
}
#[test]
fn rejected_packets_do_not_mutate_state_and_old_incarnations_fail() {
    let tx = ResourceSender::new(execution(1), RenderLimits::default()).unwrap();
    let (first, next) = tx.prepare(&snapshot()).unwrap();
    let mut rx = ResourceReceiver::new(execution(1), RenderLimits::default()).unwrap();
    let mut bad = first.clone();
    bad.images[0].1 = 999;
    assert!(rx.receive(bad).is_err());
    assert_eq!(rx.entries(), 0);
    let mut bad = first.clone();
    bad.uploads.push(bad.uploads[0].clone());
    assert!(rx.receive(bad).is_err());
    assert_eq!(rx.retained_bytes(), 0);
    rx.receive(first.clone()).unwrap();
    assert!(rx.receive(first.clone()).is_err());
    let (second, _) = next.prepare(&snapshot()).unwrap(); // equal content from another backing still deduplicates
    assert!(second.uploads.is_empty());
    rx.receive(second).unwrap();
    let mut replacement = ResourceReceiver::new(execution(2), RenderLimits::default()).unwrap();
    assert!(replacement.receive(first).is_err());
    assert_eq!(replacement.entries(), 0);
    let new = ResourceSender::new(execution(2), RenderLimits::default()).unwrap();
    let (packet, _) = new.prepare(&snapshot()).unwrap();
    assert_eq!(packet.uploads.len(), 1);
    replacement.receive(packet).unwrap();
}
#[test]
fn cumulative_table_and_packet_budgets_are_not_relaxed_by_reference_reuse() {
    let tx = ResourceSender::new(execution(1), RenderLimits::default()).unwrap();
    let (packet, _) = tx.prepare(&snapshot()).unwrap();
    let mut rx = ResourceReceiver::new(
        execution(1),
        RenderLimits {
            resource_bytes: 3,
            ..RenderLimits::default()
        },
    )
    .unwrap();
    assert!(rx.receive(packet).is_err());
    assert_eq!(rx.retained_bytes(), 0);
    let mut tx = ResourceSender::new(
        execution(1),
        RenderLimits {
            resource_bytes: 4,
            ..RenderLimits::default()
        },
    )
    .unwrap();
    let (_, next) = tx.prepare(&snapshot()).unwrap();
    tx = next;
    let mut s = Scene::new(32, 32);
    s.push_image(
        0.,
        0.,
        8.,
        8.,
        1,
        wgpu_cue_netrender::netrender::ImageData::from_bytes(1, 1, vec![5, 6, 7, 8]),
    );
    assert!(
        tx.prepare(&SceneSnapshot::capture_owned(s, RenderLimits::default()).unwrap())
            .is_err()
    );
    assert_eq!(tx.retained_bytes(), 4);
}

#[test]
fn many_aliases_share_one_bounded_resource_entry() {
    let mut scene = Scene::new(32, 32);
    for key in 0..300 {
        scene.image_sources.insert(
            key,
            wgpu_cue_netrender::netrender::ImageData::from_bytes(1, 1, vec![1, 2, 3, 4]),
        );
    }
    let snapshot = SceneSnapshot::capture_owned(scene, RenderLimits::default()).unwrap();
    let tx = ResourceSender::new(execution(1), RenderLimits::default()).unwrap();
    let mut rx = ResourceReceiver::new(execution(1), RenderLimits::default()).unwrap();
    let (packet, _) = tx.prepare(&snapshot).unwrap();
    assert_eq!(packet.uploads.len(), 1);
    let received = rx.receive(packet).unwrap();
    assert_eq!(rx.entries(), 1);
    assert_eq!(rx.retained_bytes(), 4);
    assert_eq!(
        received
            .decode(RenderLimits::default())
            .unwrap()
            .image_sources
            .len(),
        300
    );
}
