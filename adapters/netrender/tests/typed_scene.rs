// SPDX-License-Identifier: MPL-2.0
#![cfg(feature = "scene-jobs")]
use wgpu_cue_netrender::{Scene, scene_job::*};

#[test]
fn typed_payloads_are_owned_shared_and_exactly_accounted() {
    let mut scene = Scene::new(32, 32);
    scene.push_image(
        0.,
        0.,
        8.,
        8.,
        7,
        wgpu_cue_netrender::netrender::ImageData::from_bytes(1, 1, vec![0, 10, 100, 255]),
    );
    let retained = scene.image_sources[&7].data.clone();
    let snapshot = if std::env::var_os("CUE_TEST_BYTE_CONTROL").is_some() {
        SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap()
    } else {
        SceneSnapshot::capture_owned(scene, RenderLimits::default()).unwrap()
    };
    let decoded = snapshot.decode(RenderLimits::default()).unwrap();
    assert!(!snapshot.encoded_materialized());
    assert_eq!(
        decoded.image_sources[&7].data.as_ref().as_ptr(),
        retained.as_ref().as_ptr()
    );
    assert_ne!(decoded.image_sources[&7].data.id(), retained.id());
    assert_eq!(snapshot.accounted_bytes(), snapshot.bytes().len());
    drop(snapshot);
    assert_eq!(decoded.image_sources[&7].data.as_ref(), &[0, 10, 100, 255]);
}
#[test]
fn typed_validation_preserves_field_closure_and_stricter_receiver_budgets() {
    let mut invalid = Scene::new(32, 32);
    invalid.root_alpha = f32::NAN;
    assert!(SceneSnapshot::capture(&invalid, RenderLimits::default()).is_err());
    assert!(SceneSnapshot::capture_owned(invalid, RenderLimits::default()).is_err());
    let mut scene = Scene::new(32, 32);
    scene.push_image(
        0.,
        0.,
        8.,
        8.,
        7,
        wgpu_cue_netrender::netrender::ImageData::from_bytes(1, 1, vec![255; 4]),
    );
    let snapshot = SceneSnapshot::capture_owned(scene.clone(), RenderLimits::default()).unwrap();
    assert!(
        snapshot
            .decode(RenderLimits {
                resource_bytes: 3,
                ..RenderLimits::default()
            })
            .is_err()
    );
    assert!(
        snapshot
            .decode(RenderLimits {
                snapshot_bytes: snapshot.accounted_bytes() - 1,
                ..RenderLimits::default()
            })
            .is_err()
    );
    scene.image_sources.clear();
    assert!(matches!(
        SceneSnapshot::capture_owned(scene, RenderLimits::default()),
        Err(RenderError::UnresolvedResource(_))
    ));
}
