// SPDX-License-Identifier: MPL-2.0
#![cfg(all(feature = "metal-handoff", target_os = "macos"))]
use cue_execution_proof::metal_handoff::{Lease, MetalProcess};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use webcue::*;
use wgpu_cue::*;
use wgpu_cue_genet::{GenetDocument, HeldResources};
use wgpu_cue_netrender::{hybrid::HybridSceneSource, scene_job::*};
fn viewport() -> Viewport {
    Viewport {
        width: 160,
        height: 96,
        scale: 1.0,
    }
}
fn scope(generation: u64) -> Scope {
    Scope {
        owner: (1u128 << 100) + 71,
        generation,
    }
}
fn snapshot() -> SceneSnapshot {
    let mut doc = GenetDocument::new(viewport(), "https://fixture.invalid/metal".into(),
        "<!doctype html><style>body{margin:0;background:#204080;color:white;font:16px Arial}p{margin:8px}</style><p>Genet Metal handoff</p>".into(), HeldResources::default()).unwrap();
    let scene = doc.prepare(viewport()).unwrap();
    assert!(scene.iter_glyph_runs().next().is_some());
    SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap()
}
fn process(scope: Scope, scene: &SceneSnapshot) -> MetalProcess {
    let p = MetalProcess::spawn(
        Path::new(env!("CARGO_BIN_EXE_cue-execution-proof")),
        scope,
        scene,
    )
    .unwrap();
    assert_ne!(p.pid(), std::process::id());
    p
}
fn core() -> genet_render_host::RenderCore {
    let core = genet_render_host::RenderCore::boot(Default::default()).unwrap();
    let info = core.renderer().wgpu_device.core.adapter.get_info();
    assert_eq!(info.backend, wgpu::Backend::Metal);
    assert_ne!(info.device_type, wgpu::DeviceType::Cpu);
    eprintln!(
        "Metal consumer pid={}, adapter={info:?}",
        std::process::id()
    );
    core
}
fn read(core: &genet_render_host::RenderCore, texture: &wgpu::Texture) -> Vec<u8> {
    let mut read = core
        .start_rgba8_readback(texture, viewport().width, viewport().height)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        core.device().poll(wgpu::PollType::Poll).unwrap();
        if let Some(result) = read.poll() {
            return result.unwrap().rgba;
        }
        assert!(Instant::now() < deadline, "GPU readback deadline");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn local(core: &genet_render_host::RenderCore, host: &GpuHost, scene: &SceneSnapshot) -> Vec<u8> {
    let cx = host.context(viewport()).unwrap();
    let mut source = HybridSceneSource::new(scope(91), &cx, RenderLimits::default()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !source.poll_idle().unwrap() {
        assert!(Instant::now() < deadline);
    }
    source.submit(&cx, 1, scene).unwrap();
    loop {
        if let Rendered::Changed(frame) = source.render(&cx, true).unwrap() {
            return read(core, &frame.texture);
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn import(
    p: &mut MetalProcess,
    lease: Lease,
    importer: &mut GraftIoSurfaceImporter,
    cx: &RenderContext<'_>,
) -> TextureFrame {
    p.allow_copy(lease).unwrap();
    p.wait_ready(lease).unwrap();
    let frame = p.try_frame(lease).unwrap().unwrap();
    match importer.try_import(cx, frame).unwrap() {
        Imported::Ready(f) => f,
        _ => panic!("already observed producer completion"),
    }
}
fn released(importer: &mut GraftIoSurfaceImporter) -> Vec<FrameStamp> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let ready = importer.poll_releases().unwrap();
        if !ready.is_empty() {
            return ready;
        }
        assert!(
            Instant::now() < deadline,
            "consumer texture references did not release"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
struct InboxSource {
    scope: Scope,
    report: CapabilityReport,
    frame: Option<NativeFrame>,
}
impl InboxSource {
    fn new(scope: Scope, frame: NativeFrame) -> Self {
        Self {
            scope,
            frame: Some(frame),
            report: CapabilityReport::new(
                BackendIdentity {
                    engine: "metal-handoff-proof".into(),
                    version: None,
                    platform: "macos".into(),
                    build_features: vec![],
                },
                PresentationMode::NativeFrame,
                Evidence::Constructed,
                SemanticProtocol::Unavailable,
                Feature::ALL.map(|f| (f, Support::Unsupported("raster fixture".into()))),
            )
            .unwrap(),
        }
    }
}
impl Surface for InboxSource {
    fn scope(&self) -> Scope {
        self.scope
    }
    fn capabilities(&self) -> &CapabilityReport {
        &self.report
    }
    fn set_viewport(&mut self, _: Viewport) -> Result<()> {
        Ok(())
    }
    fn set_visible(&mut self, _: bool) -> Result<()> {
        Ok(())
    }
    fn input(&mut self, _: Input) -> Result<()> {
        Ok(())
    }
    fn request(&mut self, _: RequestId, _: Command) -> Result<RequestKey> {
        Err(Error::Unsupported("fixture".into()))
    }
    fn semantic_action(&mut self, _: SemanticAction) -> Result<()> {
        Err(Error::Unsupported("fixture".into()))
    }
    fn advance(&mut self, _: Duration) -> Result<()> {
        Ok(())
    }
    fn poll_event(&mut self) -> Option<Envelope> {
        None
    }
    fn retire(&mut self) -> Result<()> {
        self.frame = None;
        Ok(())
    }
}
impl FrameSource for InboxSource {
    fn try_acquire_frame(&mut self) -> Result<Option<Frame>> {
        Ok(self.frame.take().map(Frame::Native))
    }
}

#[test]
#[ignore = "requires native Metal and two local processes; run explicitly"]
fn two_process_pixels_readiness_and_slot_custody() {
    let scene = snapshot();
    let core = core();
    let host = GpuHost::new(core.device().clone(), core.queue().clone()).unwrap();
    let cx = host.context(viewport()).unwrap();
    let expected = local(&core, &host, &scene);
    assert!(
        expected
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[3] == 255 && u16::from(p[2]) > u16::from(p[0]) * 2),
        "the reference must contain its blue background"
    );
    assert!(
        expected
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[3] == 255 && p[..3].iter().all(|v| *v > 220)),
        "the reference must contain its white text"
    );
    let mut p = process(scope(1), &scene);
    let mut importer = GraftIoSurfaceImporter::new(p.scope(), host.epoch());
    let a = p.acquire(1).unwrap();
    let b = p.acquire(2).unwrap();
    assert!(
        p.acquire(2).is_err(),
        "duplicate revisions must not return an existing lease"
    );
    assert!(p.try_frame(a).unwrap().is_none());
    assert!(p.try_frame(b).unwrap().is_none());
    assert!(
        p.acquire(3)
            .unwrap_err()
            .to_string()
            .contains("Backpressure")
    );
    p.allow_copy(a).unwrap();
    p.wait_ready(a).unwrap();
    let mut source = InboxSource::new(p.scope(), p.try_frame(a).unwrap().unwrap());
    let mut presenter = Presenter::default();
    let presented = presenter
        .present(
            &host,
            Source::Native(&mut source),
            viewport(),
            &mut importer,
        )
        .unwrap()
        .unwrap();
    let clone = presented.texture.clone();
    let retained_view = clone.create_view(&Default::default());
    let actual = read(&core, &clone);
    assert_eq!(
        actual, expected,
        "same Genet Hybrid scene must survive native handoff exactly"
    );
    eprintln!(
        "two-process Genet Hybrid pixels: {} RGBA bytes exactly equal",
        actual.len()
    );
    let stamp = presented.stamp;
    drop(presented);
    importer.seal_use(stamp).unwrap();
    assert!(importer.seal_use(stamp).is_err());
    assert!(
        importer.poll_releases().unwrap().is_empty(),
        "Presenter custody keeps slot busy"
    );
    presenter.retire();
    assert!(
        importer.poll_releases().unwrap().is_empty(),
        "texture clone keeps slot busy after GPU completion"
    );
    assert!(
        p.acquire(3)
            .unwrap_err()
            .to_string()
            .contains("Backpressure")
    );
    drop(clone);
    assert!(
        importer.poll_releases().unwrap().is_empty(),
        "a retained texture view also keeps the slot busy"
    );
    drop(retained_view);
    assert_eq!(released(&mut importer), [stamp]);
    assert!(importer.poll_releases().unwrap().is_empty());
    let mut stale = stamp;
    stale.scope.generation += 1;
    assert!(p.release(stale).is_err());
    assert_eq!(p.release(stamp).unwrap(), 1);
    assert!(p.release(stamp).is_err());
    let c = p.acquire(3).unwrap();
    assert_eq!(c.slot, a.slot);
    assert!(c.epoch > b.epoch);
    for lease in [b, c] {
        let f = import(&mut p, lease, &mut importer, &cx);
        let pixels = read(&core, &f.texture);
        assert_eq!(pixels, expected);
        importer.seal_use(f.stamp).unwrap();
        let stamp = f.stamp;
        drop(f);
        assert_eq!(released(&mut importer), [stamp]);
        p.release(stamp).unwrap();
    }
    assert_eq!(importer.in_flight(), 0);
    p.shutdown().unwrap();
}

#[test]
#[ignore = "requires native Metal and two local processes; run explicitly"]
fn producer_exit_before_readiness_never_publishes_and_restart_uses_fresh_scope() {
    let scene = snapshot();
    let mut p = process(scope(2), &scene);
    let lease = p.acquire(1).unwrap();
    assert!(p.try_frame(lease).unwrap().is_none());
    p.kill().unwrap();
    assert!(p.try_frame(lease).is_err());
    let mut next = process(scope(3), &scene);
    let fresh = next.acquire(1).unwrap();
    assert_ne!(lease.stamp(scope(2)).scope, fresh.stamp(next.scope()).scope);
    next.kill().unwrap();
}

#[test]
#[ignore = "requires native Metal and two local processes; run explicitly"]
fn imported_storage_survives_producer_exit_and_requires_consumer_fence() {
    let scene = snapshot();
    let core = core();
    let host = GpuHost::new(core.device().clone(), core.queue().clone()).unwrap();
    let cx = host.context(viewport()).unwrap();
    let expected = local(&core, &host, &scene);
    let mut p = process(scope(4), &scene);
    let mut importer = GraftIoSurfaceImporter::new(p.scope(), host.epoch());
    let lease = p.acquire(1).unwrap();
    let frame = import(&mut p, lease, &mut importer, &cx);
    p.kill().unwrap();
    assert_eq!(read(&core, &frame.texture), expected);
    let stamp = frame.stamp;
    drop(frame);
    assert!(
        importer.poll_releases().unwrap().is_empty(),
        "dropping CPU handles without a sealed GPU fence cannot acknowledge use"
    );
    importer.seal_use(stamp).unwrap();
    assert_eq!(released(&mut importer), [stamp]);
    assert!(p.release(stamp).is_err());
}

#[test]
#[ignore = "requires native Metal; run explicitly"]
fn native_metadata_and_device_generations_are_checked() {
    use grafting::metal_surface::*;
    use webcue::grafting_frame::{FrameMetadata, FrameSync, OwnedNativeImage};
    let core = core();
    let host = GpuHost::new(core.device().clone(), core.queue().clone()).unwrap();
    let cx = host.context(viewport()).unwrap();
    let native = grafting::HostWgpuContext::new(cx.device.clone(), cx.queue.clone());
    let size = FrameSize::new(viewport().width, viewport().height);
    let storage = SharedMetalSurface::new(&native, size, PixelFormat::Rgba8Unorm).unwrap();
    let frame = |scope: Scope, size: FrameSize, format: PixelFormat, generation: u64| {
        let surface = lookup_surface(&storage.export_right().unwrap()).unwrap();
        NativeFrame {
            stamp: FrameStamp {
                scope,
                allocation_epoch: 1,
                content_revision: 1,
            },
            frame: OwnedNativeFrame::new(
                FrameMetadata {
                    size,
                    format,
                    generation,
                },
                OwnedNativeImage::IoSurface(surface),
                FrameSync::None,
            ),
            alpha: Alpha::Premultiplied,
            encoding: Encoding::Srgb,
        }
    };
    let mut importer = GraftIoSurfaceImporter::new(scope(5), host.epoch());
    assert!(matches!(
        importer.try_import(&cx, frame(scope(6), size, PixelFormat::Rgba8Unorm, 1)),
        Err(Error::Stale)
    ));
    assert!(
        importer
            .try_import(
                &cx,
                frame(scope(5), FrameSize::new(1, 1), PixelFormat::Rgba8Unorm, 1)
            )
            .is_err()
    );
    assert!(
        importer
            .try_import(&cx, frame(scope(5), size, PixelFormat::Bgra8Unorm, 1))
            .is_err()
    );
    assert!(
        importer
            .try_import(&cx, frame(scope(5), size, PixelFormat::Rgba8Unorm, 2))
            .is_err()
    );
    let replacement = GpuHost::new(cx.device.clone(), cx.queue.clone()).unwrap();
    let other = replacement.context(viewport()).unwrap();
    assert!(matches!(
        importer.try_import(&other, frame(scope(5), size, PixelFormat::Rgba8Unorm, 1)),
        Err(Error::Stale)
    ));
    assert_eq!(importer.in_flight(), 0);
}
