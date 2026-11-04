// SPDX-License-Identifier: MPL-2.0
#![cfg(feature = "scene-profile")]
use std::sync::{Arc, Mutex};
use wgpu_cue_netrender::{Scene, netrender::peniko::Blob, scene_job::*, scene_profile};

#[test]
fn observation_preserves_snapshot_bytes_resource_rebinding_and_budget_refusal() {
    let records = Arc::new(Mutex::new(Vec::new()));
    let observed = records.clone();
    scene_profile::install(Arc::new(move |sample| {
        observed.lock().unwrap().push(sample)
    }))
    .unwrap();
    assert!(scene_profile::install(Arc::new(|_| {})).is_err());
    let mut scene = Scene::new(32, 32);
    scene.push_rect(0., 0., 8., 8., [0.5, 0., 0., 0.5]);
    let mut font = scene.fonts[0].clone();
    font.data = Blob::new(Arc::new(vec![1u8, 2, 3, 4]));
    scene.fonts.push(font);
    let snapshot = SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap();
    assert_eq!(snapshot.bytes(), serde_json::to_vec(&scene).unwrap());
    let decoded = snapshot.decode(RenderLimits::default()).unwrap();
    assert_eq!(decoded.fonts[1].data.as_ref(), &[1, 2, 3, 4]);
    assert_ne!(decoded.fonts[1].data.id(), scene.fonts[1].data.id());
    assert!(matches!(
        snapshot.decode(RenderLimits {
            resource_bytes: 3,
            ..RenderLimits::default()
        }),
        Err(RenderError::Budget(_))
    ));
    let records = records.lock().unwrap();
    for name in [
        "capture.json_encode",
        "from_bytes.json_parse",
        "decode.json_parse",
        "decode.rebind_resources",
    ] {
        assert!(records.iter().any(|s| s.stage == name), "missing {name}");
    }
    let parsed = records
        .iter()
        .find(|s| s.stage == "from_bytes.json_parse")
        .unwrap();
    assert_eq!(parsed.bytes, snapshot.bytes().len());
}
