// SPDX-License-Identifier: MPL-2.0
//! CPU attribution probe, not a rendering or presentation qualification.
use std::{path::PathBuf, sync::Arc};
use webcue::{Input, Viewport};
use wgpu_cue_genet::{GenetDocument, HeldResources};
use wgpu_cue_netrender::{
    netrender::Scene,
    scene_job::{RenderLimits, SceneSnapshot},
    scene_profile::measure,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: cue-scene-profile ABSOLUTE_FRESH_OUTPUT FONT_FAMILY".into());
    }
    let output = PathBuf::from(&args[0]);
    if !output.is_absolute()
        || output.exists()
        || !["Arial", "sans-serif", "monospace"].contains(&args[1].as_str())
    {
        return Err("fresh absolute output and supported fixture font required".into());
    }
    std::fs::create_dir(&output)?;
    cue_execution_proof::profile::install_at(output.clone())?;
    let mut viewport = Viewport {
        width: 800,
        height: 600,
        scale: 1.,
    };
    let mut doc = GenetDocument::new(
        viewport,
        "https://fixture.invalid/receiver".into(),
        cue_execution_proof::scene_fixture::html(&args[1]),
        HeldResources::default(),
    )?;
    let mut rows = vec![];
    for iteration in 0..72 {
        measure("probe.iteration", iteration, || ());
        if iteration == 40 {
            viewport.width = 960;
            viewport.height = 640;
        }
        let effect = doc.input(Input::Scroll {
            x: 100.,
            y: 100.,
            dx: 0.,
            dy: if (iteration / 16usize).is_multiple_of(2) {
                12.
            } else {
                -12.
            },
            modifiers: Default::default(),
        })?;
        let start = std::time::Instant::now();
        let scene = measure("probe.document_prepare", 0, || doc.prepare(viewport))?;
        let layout_ns = start.elapsed().as_nanos();
        let start = std::time::Instant::now();
        let snapshot = SceneSnapshot::capture(&scene, RenderLimits::default())?;
        let capture_ns = start.elapsed().as_nanos();
        let decoded = snapshot.decode(RenderLimits::default())?;
        assert_eq!(scene.ops.len(), decoded.ops.len());
        assert_eq!(scene.fonts.len(), decoded.fonts.len());
        for (before, after) in scene.fonts.iter().zip(&decoded.fonts) {
            assert_eq!(before.data.as_ref(), after.data.as_ref());
            if !before.data.as_ref().is_empty() {
                assert_ne!(before.data.id(), after.data.id());
            }
        }
        let mut hollow = scene.clone();
        for font in &mut hollow.fonts {
            font.data =
                wgpu_cue_netrender::netrender::peniko::Blob::new(Arc::new(Vec::<u8>::new()));
        }
        for image in hollow.image_sources.values_mut() {
            image.data =
                wgpu_cue_netrender::netrender::peniko::Blob::new(Arc::new(Vec::<u8>::new()));
        }
        let bytes = measure("ablation.no_resource_bytes.encode", 0, || {
            serde_json::to_vec(&hollow)
        })?;
        let _: Scene = measure("ablation.no_resource_bytes.parse", bytes.len(), || {
            serde_json::from_slice(&bytes)
        })?;
        rows.push(serde_json::json!({"iteration":iteration,"warmup":iteration<8,"viewport":[viewport.width,viewport.height],
            "handled":matches!(effect.effect,webcue::document::SessionEffect::Handled),"document_prepare_ns":layout_ns,
            "capture_ns":capture_ns,"snapshot_bytes":snapshot.bytes().len(),"resource_bytes":snapshot.resource_bytes(),
            "without_resources_bytes":bytes.len(),"ops":scene.ops.len(),"fonts":scene.fonts.len()}));
    }
    std::fs::write(
        output.join("iterations.json"),
        serde_json::to_vec_pretty(&rows)?,
    )?;
    Ok(())
}
