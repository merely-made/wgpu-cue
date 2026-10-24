#![cfg(target_os = "linux")]
use glow::HasContext;
use std::{
    io::Write,
    os::fd::OwnedFd,
    os::unix::net::UnixStream,
    rc::Rc,
    time::{Duration, Instant},
};
use webcue::grafting_frame::{
    DmaBufImage, DmaBufPlane, FrameMetadata, FrameSync, OwnedNativeImage,
};
use wgpu_cue::{webcue::*, *};

fn fake_frame(sync: FrameSync, modifier: u64) -> (NativeFrame, UnixStream) {
    let (owned, peer) = UnixStream::pair().unwrap();
    let frame = OwnedNativeFrame::new(
        FrameMetadata {
            size: FrameSize::new(32, 32),
            format: PixelFormat::Rgba8Unorm,
            generation: 1,
        },
        OwnedNativeImage::DmaBufImage(
            DmaBufImage::new(
                vec![OwnedFd::from(owned)],
                vec![DmaBufPlane {
                    buffer_index: 0,
                    offset: 0,
                    stride: 128,
                }],
                u32::from_le_bytes(*b"AB24"),
                modifier,
            )
            .unwrap(),
        ),
        sync,
    );
    (
        NativeFrame {
            stamp: FrameStamp {
                scope: Scope {
                    owner: 1,
                    generation: 1,
                },
                allocation_epoch: 1,
                content_revision: 1,
            },
            frame,
            alpha: Alpha::Premultiplied,
            encoding: Encoding::Srgb,
        },
        peer,
    )
}

#[test]
#[ignore = "requires hardware Vulkan, GBM and EGL DMA-BUF import"]
fn vulkan_owned_frame_pixels_and_refusals() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: Default::default(),
        backend_options: Default::default(),
        display: None,
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let info = adapter.get_info();
    assert!(matches!(
        info.device_type,
        wgpu::DeviceType::IntegratedGpu | wgpu::DeviceType::DiscreteGpu
    ));
    assert_eq!(info.backend, wgpu::Backend::Vulkan);
    eprintln!("actual adapter: {info:?}");
    let native =
        grafting::vulkan_dmabuf::create_dmabuf_host_context(&adapter, &Default::default()).unwrap();
    let host = GpuHost::new(native.device.clone(), native.queue.clone()).unwrap();
    let viewport = Viewport {
        width: 32,
        height: 32,
        scale: 1.,
    };
    let cx = host.context(viewport).unwrap();
    let mut importer = GraftVulkanImporter;
    // Refusal must close both image and synchronization custody, including
    // unsupported synchronization that cannot be interpreted as a sync_file.
    let (semaphore, mut semaphore_peer) = UnixStream::pair().unwrap();
    let (frame, mut peer) = fake_frame(FrameSync::VulkanSemaphore(semaphore.into()), 0);
    assert!(matches!(
        importer.try_import(&cx, frame),
        Err(Error::Unsupported(_))
    ));
    assert!(peer.write_all(b"closed").is_err());
    assert!(semaphore_peer.write_all(b"closed").is_err());
    let (frame, mut peer) = fake_frame(FrameSync::None, (1u64 << 56) - 1);
    assert!(matches!(
        importer.try_import(&cx, frame),
        Err(Error::Unsupported(_))
    ));
    assert!(peer.write_all(b"closed").is_err());
    let (mut frame, mut peer) = fake_frame(FrameSync::None, 0);
    frame.stamp.allocation_epoch = 2;
    assert!(matches!(
        importer.try_import(&cx, frame),
        Err(Error::Invalid(_))
    ));
    assert!(peer.write_all(b"closed").is_err());

    let connection = surfman::Connection::new().unwrap();
    let adapter = connection.create_adapter().unwrap();
    let context =
        Rc::new(grafting::surfman_gl::SurfmanFrameContext::new(&connection, &adapter).unwrap());
    let surface = context
        .create_surface(surfman::SurfaceType::Generic {
            size: euclid::default::Size2D::new(32, 32),
        })
        .unwrap();
    context.bind_surface(surface).unwrap();
    context.make_current().unwrap();
    context.prepare_for_rendering();
    let producer = grafting::surfman_gl::SurfmanFrameProducer::new(
        context.clone(),
        dpi::PhysicalSize::new(32, 32),
    );
    unsafe {
        context.glow_gl.clear_color(0.5, 0., 0., 0.5);
        context.glow_gl.clear(glow::COLOR_BUFFER_BIT);
    }
    let mut pending = producer.export_dmabuf_frame().unwrap();
    let start = Instant::now();
    let frame = loop {
        if let Some(frame) = pending.try_take_frame().unwrap() {
            break frame;
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(1));
    };
    let stamp = FrameStamp {
        scope: Scope {
            owner: 22,
            generation: 1,
        },
        allocation_epoch: frame.metadata().generation,
        content_revision: 1,
    };
    let imported = importer
        .try_import(
            &cx,
            NativeFrame {
                frame,
                stamp,
                alpha: Alpha::Premultiplied,
                encoding: Encoding::Srgb,
            },
        )
        .unwrap();
    let Imported::Ready(frame) = imported else {
        panic!("producer-complete frame should submit immediately")
    };
    assert_eq!(frame.stamp, stamp);
    assert_eq!(frame.device_epoch, host.epoch());
    assert_eq!(frame.alpha, Alpha::Premultiplied);
    assert_eq!(frame.encoding, Encoding::Srgb);
    drop(pending);
    drop(producer);
    let mut surface = context.unbind_surface().unwrap().unwrap();
    context
        .device
        .borrow()
        .destroy_surface(&mut context.context.borrow_mut(), &mut surface)
        .unwrap();
    drop(context);
    let buffer = native.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256 * 32,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = native.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        frame.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(32),
            },
        },
        wgpu::Extent3d {
            width: 32,
            height: 32,
            depth_or_array_layers: 1,
        },
    );
    native.queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |r| send.send(r).unwrap());
    native
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    receive
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let bytes = buffer.slice(..).get_mapped_range().unwrap();
    for row in bytes.chunks_exact(256) {
        for pixel in row[..128].chunks_exact(4) {
            assert_eq!(pixel, &[128, 0, 0, 128]);
        }
    }
    drop(bytes);
    buffer.unmap();
}
