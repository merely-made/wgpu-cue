// SPDX-License-Identifier: MPL-2.0
#![cfg(feature = "cpu-raster")]
use std::path::Path;
use webcue::*;
use wgpu_cue_genet::{GenetDocument, HeldResources};
use wgpu_cue_netrender::{Scene, cpu::*};

fn viewport() -> Viewport {
    Viewport {
        width: 160,
        height: 96,
        scale: 1.0,
    }
}
fn source() -> CpuSceneSource {
    CpuSceneSource::new(
        Scope {
            owner: (1u128 << 100) + 9,
            generation: 1,
        },
        viewport(),
        RenderLimits::default(),
    )
    .unwrap()
}
fn scene() -> Scene {
    let mut document = GenetDocument::new(viewport(), "https://fixture.invalid/raster".into(),
        "<!doctype html><style>body{margin:0;background:#204080;color:white;font:16px Arial}p{margin:8px}</style><p>Genet raster job</p>".into(), HeldResources::default()).unwrap();
    let scene = document.prepare(viewport()).unwrap();
    assert!(scene.iter_glyph_runs().next().is_some());
    scene
}
fn job(source: &mut CpuSceneSource) -> RenderJob {
    source
        .job(
            1,
            SceneSnapshot::capture(&scene(), RenderLimits::default()).unwrap(),
        )
        .unwrap()
}

#[test]
fn same_genet_scene_rasterizes_on_worker_and_process_with_owned_fonts() {
    let mut source = source();
    let job = job(&mut source);
    let worker_job = job.clone();
    let local =
        std::thread::spawn(move || rasterize(&worker_job, RenderLimits::default()).unwrap())
            .join()
            .unwrap();
    let remote = cue_execution_proof::raster::in_process(
        Path::new(env!("CARGO_BIN_EXE_cue-execution-proof")),
        &job,
    )
    .unwrap();
    assert_eq!(local.stamp, remote.stamp);
    assert_eq!(local.rgba, remote.rgba);
    assert_eq!(remote.alpha, Alpha::Premultiplied);
    assert_eq!(remote.encoding, Encoding::Srgb);
    assert!(
        remote
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0].abs_diff(32) < 3
                && px[1].abs_diff(64) < 3
                && px[2].abs_diff(128) < 3)
            .count()
            > 1000
    );
    source.complete(remote).unwrap();
    assert!(matches!(
        source.try_acquire_frame().unwrap(),
        Some(Frame::Cpu(_))
    ));
    assert!(source.try_acquire_frame().unwrap().is_none());
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires a native Metal adapter; run explicitly"]
fn rendered_cpu_frame_uploads_unchanged_to_host_device() {
    use wgpu_cue::*;
    let mut source = source();
    let job = job(&mut source);
    let frame = cue_execution_proof::raster::in_process(
        Path::new(env!("CARGO_BIN_EXE_cue-execution-proof")),
        &job,
    )
    .unwrap();
    let expected = frame.rgba.clone();
    source.complete(frame).unwrap();
    let core = genet_render_host::RenderCore::boot(Default::default()).unwrap();
    let adapter = core.renderer().wgpu_device.core.adapter.get_info();
    assert_eq!(adapter.backend, wgpu::Backend::Metal);
    assert_ne!(adapter.device_type, wgpu::DeviceType::Cpu);
    eprintln!("CPU receiving adapter: {adapter:?}");
    let host = GpuHost::new(core.device().clone(), core.queue().clone()).unwrap();
    let mut presenter = Presenter::default();
    let mut importer = GraftMetalImporter;
    let presented = presenter
        .present(
            &host,
            Source::Native(&mut source),
            viewport(),
            &mut importer,
        )
        .unwrap()
        .unwrap();
    assert_eq!(presented.alpha, Alpha::Premultiplied);
    let mut readback = core
        .start_rgba8_readback(presented.texture, viewport().width, viewport().height)
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let bytes = loop {
        core.device().poll(wgpu::PollType::Poll).unwrap();
        if let Some(result) = readback.poll() {
            break result.unwrap().rgba;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert_eq!(bytes, expected);

    // The identical owned Genet scene must also be admitted by Classic. This
    // bounded fixture checks its canvas and text, not general backend parity.
    let scene = job.snapshot.decode(RenderLimits::default()).unwrap();
    let mut classic =
        wgpu_cue_netrender::SceneRasterizer::new(core.renderer().wgpu_device.core.clone()).unwrap();
    let context = host.context(viewport()).unwrap();
    let texture = loop {
        if let Rendered::Changed(frame) = classic.render(&context, &scene, job.stamp).unwrap() {
            break frame.texture;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    let mut readback = core
        .start_rgba8_readback(&texture, viewport().width, viewport().height)
        .unwrap();
    let classic = loop {
        core.device().poll(wgpu::PollType::Poll).unwrap();
        if let Some(result) = readback.poll() {
            break result.unwrap().rgba;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    for pixels in [&bytes, &classic] {
        let pixels = pixels.as_chunks::<4>().0;
        let blue = pixels
            .iter()
            .filter(|p| {
                p[0].abs_diff(32) < 3
                    && p[1].abs_diff(64) < 3
                    && p[2].abs_diff(128) < 3
                    && p[3] == 255
            })
            .count();
        let text = pixels
            .iter()
            .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200 && p[3] == 255)
            .count();
        assert!(
            blue > 1000 && text > 20,
            "expected the scene's canvas and foreground text"
        );
    }
    let difference: u64 = bytes
        .iter()
        .zip(&classic)
        .map(|(a, b)| a.abs_diff(*b) as u64)
        .sum();
    eprintln!(
        "Same Genet scene CPU/Classic mean channel difference: {}",
        difference as f64 / bytes.len() as f64
    );
}
