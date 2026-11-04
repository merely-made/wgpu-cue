// SPDX-License-Identifier: MPL-2.0
#![cfg(all(feature = "dx12-handoff", target_os = "windows"))]
use cue_execution_proof::dx12_handoff::{Dx12Process, gpu};
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
        assert!(Instant::now() < end, "native DX12 deadline");
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
    document.execute(Command::LoadHtml{address:"https://fixture.invalid/dx12".into(),html:format!("<!doctype html><style>body{{margin:0;background:{color};color:white;font:16px Arial}}p{{margin:8px}}</style><p>{text}</p>")},viewport).unwrap();
    let scene = document.prepare(viewport).unwrap();
    assert!(scene.iter_glyph_runs().next().is_some());
    SceneSnapshot::capture(&scene, RenderLimits::default()).unwrap()
}
fn resource_handle(frame: &NativeFrame) -> std::os::windows::io::OwnedHandle {
    let webcue::grafting_frame::OwnedNativeImage::Dx12SharedTexture(image) = frame.frame.image()
    else {
        panic!("expected DX12 resource");
    };
    image.resource().as_handle().try_clone_to_owned().unwrap()
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
fn session(
    execution: ExecutionId,
    snapshot: &SceneSnapshot,
    native: &grafting::HostWgpuContext,
) -> RenderSession<Dx12Process> {
    let process = Dx12Process::spawn_execution(
        Path::new(env!("CARGO_BIN_EXE_cue-execution-proof")),
        execution,
        snapshot,
        grafting::dx12_surface::adapter_luid(native).unwrap(),
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
fn result(session: &mut RenderSession<Dx12Process>) -> NativeFrame {
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
    importer: &mut GraftDx12SurfaceImporter,
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
fn released(importer: &mut GraftDx12SurfaceImporter) -> FrameStamp {
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
#[ignore = "requires hardware DX12, debug layer and two local processes; run explicitly"]
fn persistent_genet_dx12_pixels_slots_resize_restart_and_custody() {
    let (_, host, native) = gpu().unwrap();
    let small = viewport(160, 96);
    let large = viewport(192, 112);
    let mut document = GenetDocument::new(
        small,
        "https://fixture.invalid/dx12".into(),
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
        &native,
    );
    let mut importer = GraftDx12SurfaceImporter::new(scope(), host.epoch());
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
    let backing3 = resource_handle(&native3);

    let frame3 = import(&mut importer, &host, native3);
    let lease3 = session.transport_mut().lease(frame3.stamp).unwrap();
    assert_eq!(read(&native, &frame3.texture), expected3);
    // Reuse the released slot at the SAME extent as well: this exercises a
    // COMMON-state acquire of previously used storage, not just resize allocation.
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
    let backing4 = resource_handle(&native4);
    use std::os::windows::io::AsRawHandle;
    assert!(
        unsafe {
            windows::Win32::Foundation::CompareObjectHandles(
                windows::Win32::Foundation::HANDLE(backing3.as_raw_handle()),
                windows::Win32::Foundation::HANDLE(backing4.as_raw_handle()),
            )
        }
        .as_bool(),
        "same-size reuse must preserve the actual shared kernel allocation"
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
        &native,
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
    cue_execution_proof::dx12_handoff::check_validation(&native).unwrap();
    replacement.retire().unwrap();
    wait(|| {
        replacement.advance().unwrap();
        replacement.stopped().is_some()
    });
}

/// Keep a direct local renderer control beside the process fixture. Validation
/// diagnostics here precede any export allocation or native frame transport.
#[test]
#[ignore = "requires hardware DX12; run explicitly"]
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
    cue_execution_proof::dx12_handoff::check_validation(&native).unwrap();
    eprintln!("local Hybrid control passed on the host device");
}

#[test]
#[ignore = "requires hardware DX12, debug layer and two devices"]
fn queued_fence_wait_and_final_view_release() {
    use webcue::grafting_frame::{
        Dx12SharedFence, Dx12SharedResource, Dx12SharedTexture, FrameMetadata, FrameSync,
        OwnedNativeFrame, OwnedNativeImage,
    };
    use windows::Win32::Graphics::Direct3D12::*;
    let (_, producer_host, producer) = gpu().unwrap();
    let (_, consumer_host, consumer) = gpu().unwrap();
    assert_eq!(
        grafting::dx12_surface::adapter_luid(&producer).unwrap(),
        grafting::dx12_surface::adapter_luid(&consumer).unwrap()
    );
    let size = FrameSize::new(16, 8);
    let source = producer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("stalled producer control"),
        size: wgpu::Extent3d {
            width: 16,
            height: 8,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let expected = [0x20u8, 0x40, 0x80, 0xff].repeat(16 * 8);
    producer.queue.write_texture(
        source.as_image_copy(),
        &expected,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(64),
            rows_per_image: Some(8),
        },
        source.size(),
    );
    producer.queue.submit([]);
    let device = unsafe { producer.device.as_hal::<wgpu::hal::api::Dx12>() }.unwrap();
    let block: ID3D12Fence =
        unsafe { device.raw_device().CreateFence(0, D3D12_FENCE_FLAG_NONE) }.unwrap();
    drop(device);
    struct Unblock(ID3D12Fence);
    impl Drop for Unblock {
        fn drop(&mut self) {
            let _ = unsafe { self.0.Signal(1) };
        }
    }
    let unblock = Unblock(block.clone());
    let queue = unsafe { producer.queue.as_hal::<wgpu::hal::api::Dx12>() }.unwrap();
    unsafe { queue.as_raw().Wait(&block, 1) }.unwrap();
    drop(queue);
    let mut surface = grafting::dx12_surface::SharedDx12Surface::new(&producer, size).unwrap();
    unsafe { surface.copy_from(&source) }.unwrap();
    assert!(
        !surface.poll_ready().unwrap(),
        "producer work is intentionally GPU-blocked"
    );
    assert!(unsafe { surface.acknowledge_release() }.is_err());
    assert!(unsafe { surface.copy_from(&source) }.is_err());
    let (resource, fence, value) = surface.export().unwrap();
    let stamp = FrameStamp {
        scope: scope(),
        allocation_epoch: 1,
        content_revision: 1,
    };
    let frame = NativeFrame {
        stamp,
        alpha: Alpha::Premultiplied,
        encoding: Encoding::Srgb,
        frame: OwnedNativeFrame::new(
            FrameMetadata {
                size,
                format: PixelFormat::Rgba8Unorm,
                generation: 1,
            },
            OwnedNativeImage::Dx12SharedTexture(Dx12SharedTexture::new(
                Dx12SharedResource::from_owned_handle(resource),
            )),
            FrameSync::Dx12Fence {
                fence: Dx12SharedFence::from_owned_handle(fence),
                value,
            },
        ),
    };
    let mut importer = GraftDx12SurfaceImporter::new(scope(), consumer_host.epoch());
    let imported = import(&mut importer, &consumer_host, frame);
    let mut encoder = consumer.device.create_command_encoder(&Default::default());
    let buffer = consumer.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256 * 8,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        imported.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(8),
            },
        },
        imported.texture.size(),
    );
    consumer.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    for _ in 0..20 {
        consumer.device.poll(wgpu::PollType::Poll).unwrap();
        assert!(
            matches!(rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
            "consumer must wait for the producer GPU fence"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    unsafe { block.Signal(1) }.unwrap();
    wait(|| {
        consumer.device.poll(wgpu::PollType::Poll).unwrap();
        match rx.try_recv() {
            Ok(result) => {
                result.unwrap();
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(e) => panic!("{e}"),
        }
    });
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    let pixels: Vec<u8> = mapped
        .chunks(256)
        .flat_map(|row| row[..64].to_vec())
        .collect();
    assert_eq!(pixels, expected);
    drop(mapped);
    buffer.unmap();
    let held = imported.texture.create_view(&Default::default());
    importer.seal_use(stamp).unwrap();
    drop(imported);
    for _ in 0..20 {
        assert!(
            importer.poll_releases().unwrap().is_empty(),
            "retained view must block release after consumer completion"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    drop(held);
    assert_eq!(released(&mut importer), stamp);
    unsafe { surface.acknowledge_release() }.unwrap();
    assert!(unsafe { surface.acknowledge_release() }.is_err());
    cue_execution_proof::dx12_handoff::check_validation(&producer).unwrap();
    cue_execution_proof::dx12_handoff::check_validation(&consumer).unwrap();
    drop(unblock);
    drop(producer_host);
}
