// SPDX-License-Identifier: MPL-2.0
#![cfg(all(feature = "hybrid-raster", target_os = "macos"))]
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use webcue::*;
use wgpu_cue::*;
use wgpu_cue_genet::{GenetDocument, HeldResources};
use wgpu_cue_netrender::netrender::vello_backends::{
    BackendAdmissionError, SparseSessionError, VelloBackend,
};
use wgpu_cue_netrender::{Scene, hybrid::HybridSceneSource, scene_job::*};

fn viewport() -> Viewport {
    Viewport {
        width: 160,
        height: 96,
        scale: 1.0,
    }
}
fn scope(generation: u64) -> Scope {
    Scope {
        owner: (1u128 << 100) + 19,
        generation,
    }
}
fn core() -> Arc<genet_render_host::RenderCore> {
    let core = Arc::new(genet_render_host::RenderCore::boot(Default::default()).unwrap());
    let adapter = core.renderer().wgpu_device.core.adapter.get_info();
    assert_eq!(adapter.backend, wgpu::Backend::Metal);
    assert_ne!(adapter.device_type, wgpu::DeviceType::Cpu);
    eprintln!("Hybrid adapter: {adapter:?}");
    core
}
fn host(core: &genet_render_host::RenderCore) -> GpuHost {
    GpuHost::new(core.device().clone(), core.queue().clone()).unwrap()
}
fn settle(source: &mut HybridSceneSource) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !source.poll_idle().unwrap() {
        assert!(
            Instant::now() < deadline,
            "Hybrid submission did not become idle"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn solid() -> SceneSnapshot {
    let mut scene = Scene::new(viewport().width, viewport().height);
    scene.push_rect(0., 0., 32., 32., [0.5, 0., 0., 0.5]);
    SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap()
}
fn image() -> SceneSnapshot {
    let mut scene = Scene::new(viewport().width, viewport().height);
    scene.push_image(
        0.,
        0.,
        32.,
        32.,
        7,
        wgpu_cue_netrender::netrender::ImageData::from_bytes(1, 1, vec![0, 0, 255, 128]),
    );
    SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap()
}
fn genet() -> (GenetDocument, SceneSnapshot) {
    let mut document = GenetDocument::new(viewport(), "https://fixture.invalid/hybrid".into(),
        "<!doctype html><style>body{margin:0;background:#204080;color:white;font:16px Arial}p{margin:8px}</style><p>Genet raster job</p>".into(),HeldResources::default()).unwrap();
    let scene = document.prepare(viewport()).unwrap();
    assert!(scene.iter_glyph_runs().next().is_some());
    let snapshot = SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap();
    (document, snapshot)
}
fn read(core: &genet_render_host::RenderCore, texture: &wgpu::Texture) -> Vec<u8> {
    let mut readback = core
        .start_rgba8_readback(texture, viewport().width, viewport().height)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        core.device().poll(wgpu::PollType::Poll).unwrap();
        if let Some(result) = readback.poll() {
            return result.unwrap().rgba;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn frame(source: &mut HybridSceneSource, cx: &RenderContext<'_>) -> TextureFrame {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Rendered::Changed(frame) = source.render(cx, true).unwrap() {
            return frame;
        }
        assert!(
            Instant::now() < deadline,
            "Hybrid frame did not become ready"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
#[ignore = "requires a native Metal adapter; run explicitly"]
fn invalidation_does_not_publish_submitted_frame() {
    let core = core();
    let host = host(&core);
    let cx = host.context(viewport()).unwrap();
    let mut source = HybridSceneSource::new(scope(1), &cx, RenderLimits::default()).unwrap();
    settle(&mut source);
    source.submit(&cx, 1, &solid()).unwrap();
    assert!(source.in_flight());
    source.set_visible(false).unwrap();
    source.set_visible(true).unwrap();
    assert!(
        source.in_flight(),
        "invalidation must retain GPU work custody"
    );
    settle(&mut source);
    assert!(
        matches!(source.render(&cx, true).unwrap(), Rendered::Pending),
        "invalidated work must not publish"
    );
    source.submit(&cx, 2, &solid()).unwrap();
    source
        .set_viewport(Viewport {
            width: 64,
            ..viewport()
        })
        .unwrap();
    source.set_viewport(viewport()).unwrap();
    settle(&mut source);
    assert!(matches!(
        source.render(&cx, true).unwrap(),
        Rendered::Pending
    ));
}

#[test]
#[ignore = "requires a native Metal adapter; run explicitly"]
fn ordered_submissions_produce_premultiplied_pixels_and_retire_independently() {
    let core = core();
    let host = host(&core);
    let cx = host.context(viewport()).unwrap();
    let mut source = HybridSceneSource::new(scope(1), &cx, RenderLimits::default()).unwrap();
    settle(&mut source);
    let first = source.submit(&cx, 1, &solid()).unwrap();
    assert_eq!(source.submitted_count(), 1);
    assert!(matches!(
        source.submit(&cx, 2, &solid()),
        Err(RenderError::Control(Error::Backpressure))
    ));
    let first_frame = frame(&mut source, &cx);
    assert_eq!(first_frame.stamp, first);
    assert_eq!(first_frame.alpha, Alpha::Premultiplied);
    let pixels = read(&core, &first_frame.texture);
    let red = &pixels[(16 * viewport().width as usize + 16) * 4..][..4];
    let clear = &pixels[(64 * viewport().width as usize + 64) * 4..][..4];
    eprintln!("Hybrid premultiplied red: {red:?}; clear: {clear:?}");
    assert!(red[0].abs_diff(128) < 3 && red[1] == 0 && red[2] == 0 && red[3].abs_diff(128) < 3);
    assert_eq!(clear, [0; 4]);
    let second = source.submit(&cx, 2, &image()).unwrap();
    assert!(second.allocation_epoch > first.allocation_epoch);
    assert_eq!(source.submitted_count(), 2);
    let second_frame = frame(&mut source, &cx);
    let pixels = read(&core, &second_frame.texture);
    let blue = &pixels[(16 * viewport().width as usize + 16) * 4..][..4];
    eprintln!("Hybrid owned-image blue: {blue:?}");
    assert!(blue[0] == 0 && blue[1] == 0 && blue[2].abs_diff(128) < 3 && blue[3].abs_diff(128) < 3);
    source.submit(&cx, 3, &image()).unwrap();
    source.retire().unwrap();
    assert!(source.in_flight());
    assert!(matches!(source.render(&cx, true), Err(Error::Retired)));
    settle(&mut source);
    assert!(!source.in_flight());
}

#[test]
#[ignore = "requires a native Metal adapter; run explicitly"]
fn actual_genet_scene_renders_on_host_and_survives_renderer_reconstruction() {
    let core = core();
    let gpu_host = host(&core);
    let cx = gpu_host.context(viewport()).unwrap();
    let (mut document, snapshot) = genet();
    let mut source = HybridSceneSource::new(scope(1), &cx, RenderLimits::default()).unwrap();
    settle(&mut source);
    let stamp = source.submit(&cx, 1, &snapshot).unwrap();
    let mut presenter = Presenter::default();
    let mut importer = GraftMetalImporter;
    let deadline = Instant::now() + Duration::from_secs(30);
    let texture = loop {
        if let Some(frame) = presenter
            .present(
                &gpu_host,
                Source::HostDevice(&mut source),
                viewport(),
                &mut importer,
            )
            .unwrap()
        {
            assert_eq!(frame.stamp, stamp);
            assert_eq!(frame.alpha, Alpha::Premultiplied);
            break frame.texture.clone();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    };
    let pixels = read(&core, &texture);
    let rgba = pixels.as_chunks::<4>().0;
    assert!(
        rgba.iter()
            .filter(|p| p[0].abs_diff(32) < 3
                && p[1].abs_diff(64) < 3
                && p[2].abs_diff(128) < 3
                && p[3] == 255)
            .count()
            > 1000
    );
    assert!(
        rgba.iter()
            .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200 && p[3] == 255)
            .count()
            > 20
    );
    assert_eq!(
        source.submitted_count(),
        1,
        "polling must not encode another draw"
    );
    #[cfg(feature = "cpu-raster")]
    {
        let cpu = wgpu_cue_netrender::cpu::rasterize(
            &RenderJob {
                stamp,
                snapshot: snapshot.clone(),
            },
            RenderLimits::default(),
        )
        .unwrap();
        let scene = snapshot.decode(RenderLimits::default()).unwrap();
        let mut classic =
            wgpu_cue_netrender::SceneRasterizer::new(core.renderer().wgpu_device.core.clone())
                .unwrap();
        let texture = loop {
            if let Rendered::Changed(frame) = classic.render(&cx, &scene, stamp).unwrap() {
                break frame.texture;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        };
        let classic = read(&core, &texture);
        for (label, comparison) in [("CPU", &cpu.rgba), ("Classic", &classic)] {
            let rgba = comparison.as_chunks::<4>().0;
            assert!(
                rgba.iter()
                    .filter(|p| p[0].abs_diff(32) < 3
                        && p[1].abs_diff(64) < 3
                        && p[2].abs_diff(128) < 3
                        && p[3] == 255)
                    .count()
                    > 1000
            );
            assert!(
                rgba.iter()
                    .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200 && p[3] == 255)
                    .count()
                    > 20
            );
            let delta: u64 = pixels
                .iter()
                .zip(comparison)
                .map(|(a, b)| a.abs_diff(*b) as u64)
                .sum();
            eprintln!(
                "Same Genet scene Hybrid/{label} mean channel difference: {}",
                delta as f64 / pixels.len() as f64
            );
        }
    }
    // New logical device lifetime on the same physical device. This is renderer
    // reconstruction evidence, not a physical device-loss/driver-reset test.
    let replacement = host(&core);
    let new_cx = replacement.context(viewport()).unwrap();
    assert!(matches!(source.render(&new_cx, true), Err(Error::Stale)));
    source.retire().unwrap();
    settle(&mut source);
    let mut rebuilt = HybridSceneSource::new(scope(2), &new_cx, RenderLimits::default()).unwrap();
    settle(&mut rebuilt);
    rebuilt.submit(&new_cx, 1, &snapshot).unwrap();
    let rebuilt_frame = frame(&mut rebuilt, &new_cx);
    assert_eq!(read(&core, &rebuilt_frame.texture), pixels);
    let result = document
        .execute(
            Command::Find {
                text: "Genet".into(),
                backwards: false,
            },
            viewport(),
        )
        .unwrap();
    assert!(
        matches!(result.reply, Reply::Find(state) if state.count==1),
        "the existing document still owns its commands after renderer reconstruction"
    );
}

#[test]
#[ignore = "requires a native Metal adapter; run explicitly"]
fn foreign_devices_and_unsupported_sampling_are_refused_without_draw_submission() {
    let core = core();
    let host = host(&core);
    let cx = host.context(viewport()).unwrap();
    let mut source = HybridSceneSource::new(scope(1), &cx, RenderLimits::default()).unwrap();
    settle(&mut source);
    // RenderCore::boot shares a device. Request a distinct one explicitly.
    let (other_device, other_queue) = pollster::block_on(
        core.renderer()
            .wgpu_device
            .core
            .adapter
            .request_device(&Default::default()),
    )
    .unwrap();
    assert_ne!(&other_device, core.device());
    let other_host = GpuHost::new(other_device, other_queue.clone()).unwrap();
    assert!(matches!(
        source.submit(&other_host.context(viewport()).unwrap(), 1, &solid()),
        Err(RenderError::Control(Error::Invalid(_)))
    ));
    let mixed = RenderContext {
        device: cx.device,
        queue: &other_queue,
        device_epoch: cx.device_epoch,
        viewport: viewport(),
    };
    assert!(matches!(
        source.submit(&mixed, 1, &solid()),
        Err(RenderError::Control(Error::Invalid(_)))
    ));
    let scaled = Viewport {
        scale: 2.0,
        ..viewport()
    };
    source.set_viewport(scaled).unwrap();
    assert!(matches!(
        source.submit(&host.context(scaled).unwrap(), 1, &solid()),
        Err(RenderError::Control(Error::Unsupported(_)))
    ));
    source.set_viewport(viewport()).unwrap();
    let mut scene = Scene::new(viewport().width, viewport().height);
    scene.image_sources.insert(
        7,
        wgpu_cue_netrender::netrender::ImageData::from_bytes(1, 1, vec![255; 4]),
    );
    scene.push_pattern(7, [0., 0., 32., 32.], [1., 1.]);
    let snapshot = SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap();
    assert!(
        matches!(
            source.submit(&cx, 1, &snapshot),
            Err(RenderError::Raster(SparseSessionError::Admission(
                BackendAdmissionError::UnsupportedOperation {
                    backend: VelloBackend::Hybrid,
                    operation: "Pattern",
                    ..
                }
            )))
        ),
        "bilinear repeat must be refused"
    );
    assert_eq!(source.submitted_count(), 0);
    assert!(
        source.in_flight(),
        "a refused job retains a non-publishing cleanup fence"
    );
    settle(&mut source);
    assert!(matches!(
        source.render(&cx, true).unwrap(),
        Rendered::Pending
    ));
    source.submit(&cx, 1, &solid()).unwrap();
    let _ = frame(&mut source, &cx);
    assert_eq!(
        source.submitted_count(),
        1,
        "refused revision must remain available"
    );
    let mut limited = HybridSceneSource::new(
        scope(2),
        &cx,
        RenderLimits {
            resource_bytes: 3,
            ..Default::default()
        },
    )
    .unwrap();
    settle(&mut limited);
    assert!(matches!(
        limited.submit(&cx, 1, &snapshot),
        Err(RenderError::Budget("font/image source bytes"))
    ));
    assert_eq!(limited.submitted_count(), 0);
    assert!(!limited.in_flight());
}

#[test]
#[ignore = "requires native Metal; run explicitly"]
fn classic_and_hybrid_use_the_same_render_session_vocabulary() {
    use std::collections::VecDeque;
    use webcue::execution::{ExecutionId, SendStatus, StopOutcome};
    use wgpu_cue_netrender::{SceneRasterizer, render_session::*};
    struct Endpoint {
        jobs: VecDeque<RenderRequest>,
        replies: VecDeque<RenderReply<TextureFrame>>,
    }
    impl RenderTransport for Endpoint {
        type Frame = TextureFrame;
        fn try_send(&mut self, request: &RenderRequest) -> Result<SendStatus> {
            if self.jobs.len() >= 2 {
                return Ok(SendStatus::Busy);
            }
            self.jobs.push_back(request.clone());
            Ok(SendStatus::Sent)
        }
        fn try_recv(&mut self) -> Result<Option<RenderReply<TextureFrame>>> {
            Ok(self.replies.pop_front())
        }
        fn request_stop(&mut self) -> Result<()> {
            self.jobs.clear();
            Ok(())
        }
        fn poll_stopped(&mut self) -> Result<Option<StopOutcome>> {
            Ok(None)
        }
    }
    let core = core();
    let host = host(&core);
    let cx = host.context(viewport()).unwrap();
    let scope = scope(95);
    let mut clock = RenderClock::new(scope);
    let mut revision = 0;
    for classic in [true, false] {
        let execution = ExecutionId {
            scope,
            incarnation: if classic { 1 } else { 2 },
        };
        let mut session = RenderSession::new(
            execution,
            Endpoint {
                jobs: VecDeque::new(),
                replies: VecDeque::new(),
            },
            2,
            32 * 1024 * 1024,
            RenderLimits::default(),
        )
        .unwrap();
        let mut classic_source =
            SceneRasterizer::new(core.renderer().wgpu_device.core.clone()).unwrap();
        let mut hybrid_source =
            HybridSceneSource::new(scope, &cx, RenderLimits::default()).unwrap();
        settle(&mut hybrid_source);
        for color in [[0.5, 0.0, 0.0, 1.0], [0.0, 0.0, 0.5, 1.0]] {
            revision += 1;
            let mut scene = Scene::new(viewport().width, viewport().height);
            scene.push_rect(
                0.0,
                0.0,
                viewport().width as f32,
                viewport().height as f32,
                color,
            );
            let snapshot = SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap();
            session
                .submit(clock.job(revision, snapshot).unwrap())
                .unwrap();
            session.advance().unwrap();
            // The owner explicitly performs raster work outside transport polling.
            let request = session.transport_mut().jobs.pop_front().unwrap();
            let mut output = if classic {
                let scene = request
                    .job
                    .snapshot
                    .decode(RenderLimits::default())
                    .unwrap();
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    if let Rendered::Changed(frame) = classic_source
                        .render(&cx, &scene, request.job.stamp)
                        .unwrap()
                    {
                        break frame;
                    }
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(1));
                }
            } else {
                hybrid_source
                    .submit(&cx, revision, &request.job.snapshot)
                    .unwrap();
                frame(&mut hybrid_source, &cx)
            };
            // The host's publication clock is independent of each backend's
            // private allocation counters and survives renderer replacement.
            output.stamp = request.job.stamp;
            session.transport_mut().replies.push_back(RenderReply {
                execution,
                stamp: request.job.stamp,
                result: Ok(output),
            });
            session.advance().unwrap();
            let completed = session.poll_reply().unwrap();
            assert_eq!(completed.execution, execution);
            let output = completed.result.unwrap();
            assert_eq!(output.stamp.content_revision, revision);
            let pixels = read(&core, &output.texture);
            assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
        }
        session.retire().unwrap();
        assert_eq!(
            session.stopped(),
            None,
            "a local GPU owner is not an observed process exit"
        );
    }
}
