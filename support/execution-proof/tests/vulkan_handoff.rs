// SPDX-License-Identifier: MPL-2.0
#![cfg(all(feature = "vulkan-handoff", target_os = "linux"))]
use cue_execution_proof::vulkan_handoff::{VulkanProcess, gpu};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use webcue::{execution::*, *};
use wgpu_cue::*;
use wgpu_cue_genet::{GenetDocument, HeldResources};
use wgpu_cue_netrender::{hybrid::HybridSceneSource, render_session::*, scene_job::*};
fn viewport(width: u32, height: u32) -> Viewport {
    Viewport {
        width,
        height,
        scale: 1.0,
    }
}
fn scope() -> Scope {
    Scope {
        owner: (1u128 << 100) + 711,
        generation: 1,
    }
}
fn wait(mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < end, "native Vulkan deadline");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn read(native: &grafting::HostWgpuContext, texture: &wgpu::Texture) -> Vec<u8> {
    let width = texture.width();
    let height = texture.height();
    let stride = (width * 4).div_ceil(256) * 256;
    let buffer = native.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("qualification readback"),
        size: u64::from(stride) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = native.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(height),
            },
        },
        texture.size(),
    );
    native.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    wait(|| {
        native.device.poll(wgpu::PollType::Poll).unwrap();
        match rx.try_recv() {
            Ok(result) => {
                result.unwrap();
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(error) => panic!("{error}"),
        }
    });
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    let result = mapped
        .chunks(stride as usize)
        .flat_map(|row| row[..width as usize * 4].to_vec())
        .collect();
    drop(mapped);
    buffer.unmap();
    result
}
fn snapshot(
    document: &mut GenetDocument,
    viewport: Viewport,
    text: &str,
    color: &str,
) -> SceneSnapshot {
    document.execute(Command::LoadHtml{address:"https://fixture.invalid/vulkan".into(),html:format!("<!doctype html><style>body{{margin:0;background:{color};color:white;font:16px Arial}}p{{margin:8px}}</style><p>{text}</p>")},viewport).unwrap();
    let scene = document.prepare(viewport).unwrap();
    assert!(scene.iter_glyph_runs().next().is_some());
    SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap()
}
fn kernel_buffer(frame: &NativeFrame) -> (libc::dev_t, libc::ino_t) {
    use std::os::fd::AsRawFd;
    let webcue::grafting_frame::OwnedNativeImage::DmaBufImage(image) = frame.frame.image() else {
        panic!("expected DMA-BUF");
    };
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    assert_eq!(
        unsafe { libc::fstat(image.buffers()[0].as_raw_fd(), stat.as_mut_ptr()) },
        0
    );
    let stat = unsafe { stat.assume_init() };
    (stat.st_dev, stat.st_ino)
}
fn local(host: &GpuHost, native: &grafting::HostWgpuContext, job: &RenderJob) -> Vec<u8> {
    let size = job.snapshot.size();
    let cx = host.context(viewport(size.width, size.height)).unwrap();
    let mut source = HybridSceneSource::new(scope(), &cx, RenderLimits::default()).unwrap();
    wait(|| source.poll_idle().unwrap());
    source.submit(&cx, 1, &job.snapshot).unwrap();
    let mut frame = None;
    wait(|| {
        if let Rendered::Changed(result) = source.render(&cx, true).unwrap() {
            frame = Some(result);
            true
        } else {
            false
        }
    });
    read(native, &frame.unwrap().texture)
}
fn session(execution: ExecutionId, snapshot: &SceneSnapshot) -> RenderSession<VulkanProcess> {
    let process = VulkanProcess::spawn_execution(
        Path::new(env!("CARGO_BIN_EXE_cue-execution-proof")),
        execution,
        snapshot,
    )
    .unwrap();
    assert_ne!(process.pid(), std::process::id());
    eprintln!("helper pid={} adapter={}", process.pid(), process.adapter());
    RenderSession::new(
        execution,
        process,
        4,
        64 * 1024 * 1024,
        RenderLimits::default(),
    )
    .unwrap()
}
fn result(session: &mut RenderSession<VulkanProcess>) -> NativeFrame {
    let mut frame = None;
    wait(|| {
        session.advance().unwrap();
        if let Some(reply) = session.poll_reply() {
            frame = Some(reply.result.unwrap());
            true
        } else {
            false
        }
    });
    frame.unwrap()
}
fn import(
    importer: &mut GraftVulkanSurfaceImporter,
    host: &GpuHost,
    frame: NativeFrame,
) -> TextureFrame {
    let size = frame.frame.metadata().size;
    match importer
        .try_import(
            &host.context(viewport(size.width, size.height)).unwrap(),
            frame,
        )
        .unwrap()
    {
        Imported::Ready(frame) => frame,
        _ => panic!("completed native export must be ready"),
    }
}
fn released(importer: &mut GraftVulkanSurfaceImporter) -> FrameStamp {
    let mut result = None;
    wait(|| {
        let ready = importer.poll_releases().unwrap();
        assert!(ready.len() <= 1);
        if let Some(stamp) = ready.first() {
            result = Some(*stamp);
            true
        } else {
            false
        }
    });
    result.unwrap()
}

#[test]
#[ignore = "requires hardware Vulkan DMA-BUF and two local processes; run explicitly"]
fn persistent_genet_vulkan_pixels_slots_resize_restart_and_custody() {
    let (_, host, native) = gpu().unwrap();
    let small = viewport(160, 96);
    let large = viewport(192, 112);
    let mut document = GenetDocument::new(
        small,
        "https://fixture.invalid/vulkan".into(),
        "<p>initial</p>".into(),
        HeldResources::default(),
    )
    .unwrap();
    let mut clock = RenderClock::new(scope());
    let first = clock
        .job(1, snapshot(&mut document, small, "Alpha", "#204080"))
        .unwrap();
    let mut session = session(
        ExecutionId {
            scope: scope(),
            incarnation: 1,
        },
        &first.snapshot,
    );
    let mut importer = GraftVulkanSurfaceImporter::new(scope(), host.epoch());
    let expected1 = local(&host, &native, &first);
    session.submit(first).unwrap();
    let frame1 = import(&mut importer, &host, result(&mut session));
    assert_eq!(read(&native, &frame1.texture), expected1);
    let second = clock
        .job(2, snapshot(&mut document, small, "Beta", "#804020"))
        .unwrap();
    let expected2 = local(&host, &native, &second);
    session.submit(second).unwrap();
    let frame2 = import(&mut importer, &host, result(&mut session));
    assert_eq!(read(&native, &frame2.texture), expected2);
    assert_ne!(expected1, expected2);
    let old_lease = session.transport_mut().lease(frame2.stamp).unwrap();
    let third = clock
        .job(3, snapshot(&mut document, large, "Gamma", "#208040"))
        .unwrap();
    let expected3 = local(&host, &native, &third);
    session.submit(third).unwrap();
    for _ in 0..30 {
        session.advance().unwrap();
        assert!(
            session.poll_reply().is_none(),
            "two leased slots must stall resize"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    let held_view = frame1.texture.create_view(&Default::default());
    importer.seal_use(frame1.stamp).unwrap();
    assert!(importer.seal_use(frame1.stamp).is_err());
    let stamp1 = frame1.stamp;
    drop(frame1);
    for _ in 0..30 {
        assert!(
            importer.poll_releases().unwrap().is_empty(),
            "retained view must block release"
        );
        session.advance().unwrap();
        assert!(session.poll_reply().is_none());
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(held_view);
    assert_eq!(released(&mut importer), stamp1);
    session.transport_mut().release(stamp1).unwrap();
    let native3 = result(&mut session);
    let buffer3 = kernel_buffer(&native3);
    let frame3 = import(&mut importer, &host, native3);
    let lease3 = session.transport_mut().lease(frame3.stamp).unwrap();
    assert_eq!(read(&native, &frame3.texture), expected3);
    // Reuse the released slot at the SAME extent as well: this exercises a
    // FOREIGN acquire of previously used storage, not just resize allocation.
    importer.seal_use(frame3.stamp).unwrap();
    let stamp3 = frame3.stamp;
    drop(frame3);
    assert_eq!(released(&mut importer), stamp3);
    session.transport_mut().release(stamp3).unwrap();
    let fourth = clock
        .job(4, snapshot(&mut document, large, "Delta", "#604080"))
        .unwrap();
    let expected4 = local(&host, &native, &fourth);
    session.submit(fourth).unwrap();
    let native4 = result(&mut session);
    assert_eq!(
        kernel_buffer(&native4),
        buffer3,
        "same-size reuse must preserve the actual kernel DMA-BUF backing"
    );
    let frame4 = import(&mut importer, &host, native4);
    let lease4 = session.transport_mut().lease(frame4.stamp).unwrap();
    assert_eq!(lease4.slot, lease3.slot);
    assert!(lease4.epoch > lease3.epoch);
    assert_eq!(read(&native, &frame4.texture), expected4);
    let lost = clock
        .job(5, snapshot(&mut document, large, "Lost render", "#402060"))
        .unwrap();
    let lost_stamp = lost.stamp;
    session.submit(lost).unwrap();
    session.advance().unwrap();
    assert_eq!(session.pending(), 1);
    assert!(session.poll_reply().is_none(), "both slots are still held");
    session.transport_mut().kill().unwrap();
    wait(|| {
        session.advance().unwrap();
        session.stopped().is_some()
    });
    assert_eq!(session.stopped(), Some(StopOutcome::Terminated));
    let lost_reply = session
        .poll_reply()
        .expect("accepted job gets one terminal result");
    assert_eq!(lost_reply.stamp, lost_stamp);
    assert!(matches!(
        lost_reply.result,
        Err(RenderFailure::ExecutionLost(_))
    ));
    assert!(session.poll_reply().is_none());
    assert_eq!(
        read(&native, &frame2.texture),
        expected2,
        "import must retain storage after producer exit"
    );
    let fifth = clock
        .job(6, snapshot(&mut document, small, "Epsilon", "#406080"))
        .unwrap();
    let expected5 = local(&host, &native, &fifth);
    let mut replacement = self::session(
        ExecutionId {
            scope: scope(),
            incarnation: 2,
        },
        &fifth.snapshot,
    );
    replacement.submit(fifth).unwrap();
    let frame5 = import(&mut importer, &host, result(&mut replacement));
    assert_eq!(read(&native, &frame5.texture), expected5);
    replacement.transport_mut().send_release(old_lease).unwrap();
    wait(|| {
        replacement.transport_mut().poll().unwrap();
        replacement.transport_mut().stale().contains(&old_lease)
    });
    importer.seal_use(frame2.stamp).unwrap();
    let stamp2 = frame2.stamp;
    drop(frame2);
    assert_eq!(released(&mut importer), stamp2);
    assert!(replacement.transport_mut().release(stamp2).is_err());
    importer.seal_use(frame4.stamp).unwrap();
    let stamp4 = frame4.stamp;
    drop(frame4);
    assert_eq!(released(&mut importer), stamp4);
    importer.seal_use(frame5.stamp).unwrap();
    let stamp5 = frame5.stamp;
    drop(frame5);
    assert_eq!(released(&mut importer), stamp5);
    replacement.transport_mut().release(stamp5).unwrap();
    wait(|| {
        replacement.transport_mut().poll().unwrap();
        !replacement.transport_mut().released().is_empty()
    });
    assert_eq!(importer.in_flight(), 0);
    let reply = document
        .execute(
            Command::Find {
                text: "Epsilon".into(),
                backwards: false,
            },
            small,
        )
        .unwrap()
        .reply;
    assert!(
        matches!(reply, Reply::Find(DocumentFindState { count: 1, .. })),
        "renderer replacement must preserve document state"
    );
    replacement.retire().unwrap();
    wait(|| {
        replacement.advance().unwrap();
        replacement.stopped().is_some()
    });
}

/// Keep a direct local renderer control beside the process fixture. Validation
/// diagnostics here precede any export allocation or native frame transport.
#[test]
#[ignore = "requires hardware Vulkan; run explicitly"]
fn local_hybrid_pixel_control() {
    let (_, host, native) = gpu().unwrap();
    let viewport = viewport(160, 96);
    let mut document = GenetDocument::new(
        viewport,
        "https://fixture.invalid/control".into(),
        "<p>initial</p>".into(),
        HeldResources::default(),
    )
    .unwrap();
    let job = RenderClock::new(scope())
        .job(
            1,
            snapshot(&mut document, viewport, "Local control", "#204080"),
        )
        .unwrap();
    let pixels = local(&host, &native, &job);
    assert_eq!(pixels.len(), 160 * 96 * 4);
    assert!(pixels.iter().any(|byte| *byte != 0));
    eprintln!("local Hybrid control passed on the host device");
}
