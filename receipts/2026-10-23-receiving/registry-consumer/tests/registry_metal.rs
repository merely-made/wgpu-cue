// SPDX-License-Identifier: MPL-2.0
#![cfg(target_os = "macos")]
use objc2_metal::{
    MTLCreateSystemDefaultDevice, MTLDevice, MTLPixelFormat, MTLRegion, MTLSharedEvent,
    MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use std::collections::VecDeque;
use webcue::grafting_frame::{FrameMetadata, FrameSync, MetalTexture, OwnedNativeImage};
use webcue::*;
use wgpu_cue::*;
struct Producer {
    protocol: Protocol,
    report: CapabilityReport,
    frames: VecDeque<Frame>,
    renders: VecDeque<Rendered>,
}
impl Surface for Producer {
    fn scope(&self) -> Scope {
        self.protocol.scope()
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
    fn advance(&mut self, _: std::time::Duration) -> Result<()> {
        Ok(())
    }
    fn poll_event(&mut self) -> Option<Envelope> {
        self.protocol.poll()
    }
    fn retire(&mut self) -> Result<()> {
        self.frames.clear();
        self.protocol.retire()
    }
}
impl FrameSource for Producer {
    fn try_acquire_frame(&mut self) -> Result<Option<Frame>> {
        self.protocol.ensure_live()?;
        Ok(self.frames.pop_front())
    }
}
fn source(mode: PresentationMode) -> Producer {
    Producer {
        protocol: Protocol::new(
            Scope {
                owner: 123,
                generation: 1,
            },
            16,
        )
        .unwrap(),
        report: CapabilityReport::new(
            BackendIdentity {
                engine: "native-fixture".into(),
                version: None,
                platform: "macos".into(),
                build_features: vec![],
            },
            mode,
            Evidence::Constructed,
            SemanticProtocol::Unavailable,
            Feature::ALL.map(|f| (f, Support::Unsupported("fixture".into()))),
        )
        .unwrap(),
        frames: VecDeque::new(),
        renders: VecDeque::new(),
    }
}
fn host() -> GpuHost {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::METAL,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    eprintln!("real presentation adapter: {:?}", adapter.get_info());
    let (d, q) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    GpuHost::new(d, q).unwrap()
}
#[test]
#[ignore = "requires native Metal; run explicitly"]
fn metal_custody_pending_retry_and_cpu_transport() {
    let host = host();
    let v = Viewport {
        width: 16,
        height: 16,
        scale: 1.,
    };
    let mut importer = GraftMetalImporter;
    let device = MTLCreateSystemDefaultDevice().expect("Metal device");
    let desc = MTLTextureDescriptor::new();
    unsafe {
        desc.setWidth(16);
        desc.setHeight(16);
    }
    desc.setPixelFormat(MTLPixelFormat::RGBA8Unorm);
    desc.setUsage(MTLTextureUsage::ShaderRead);
    desc.setStorageMode(MTLStorageMode::Shared);
    let texture = device.newTextureWithDescriptor(&desc).unwrap();
    let pixels = [8u8, 16, 32, 255].repeat(16 * 16);
    // Exact initialized 16x16 RGBA8 extent and stable backing bytes.
    unsafe {
        texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
            MTLRegion {
                origin: objc2_metal::MTLOrigin { x: 0, y: 0, z: 0 },
                size: objc2_metal::MTLSize {
                    width: 16,
                    height: 16,
                    depth: 1,
                },
            },
            0,
            std::ptr::NonNull::new(pixels.as_ptr().cast_mut())
                .unwrap()
                .cast(),
            64,
        );
    }
    let event = device.newSharedEvent().unwrap();
    let mut producer = source(PresentationMode::NativeFrame);
    let scope = producer.scope();
    producer.frames.push_back(Frame::Native(NativeFrame {
        stamp: FrameStamp {
            scope,
            allocation_epoch: 1,
            content_revision: 1,
        },
        alpha: Alpha::Opaque,
        encoding: Encoding::Srgb,
        frame: OwnedNativeFrame::new(
            FrameMetadata {
                size: FrameSize::new(16, 16),
                format: PixelFormat::Rgba8Unorm,
                generation: 1,
            },
            OwnedNativeImage::MetalTexture(MetalTexture::new(texture.clone())),
            FrameSync::MetalSharedEvent {
                event: event.clone(),
                value: 9,
            },
        ),
    }));
    drop(texture); // The frame, then wgpu, must own the native retain.
    let mut presenter = Presenter::default();
    assert!(
        presenter
            .present(&host, Source::Native(&mut producer), v, &mut importer)
            .unwrap()
            .is_none()
    );
    assert!(
        presenter
            .present(&host, Source::Native(&mut producer), v, &mut importer)
            .unwrap()
            .is_none()
    );
    // A CPU signal is a test fence, not evidence about WebKit's capture queue.
    event.setSignaledValue(9);
    let first = presenter
        .present(&host, Source::Native(&mut producer), v, &mut importer)
        .unwrap()
        .unwrap();
    assert_eq!(first.stamp.content_revision, 1);
    let handle = first.texture.clone();
    assert_eq!(&read_pixels(&host, &handle)[..4], &[8, 16, 32, 255]);
    let retained = presenter
        .present(&host, Source::Native(&mut producer), v, &mut importer)
        .unwrap()
        .unwrap();
    assert_eq!(retained.texture, &handle);
    producer.protocol = Protocol::new(
        Scope {
            owner: 123,
            generation: 2,
        },
        16,
    )
    .unwrap();
    assert!(
        presenter
            .present(&host, Source::Native(&mut producer), v, &mut importer)
            .unwrap()
            .is_none(),
        "old generation leaked through cache"
    );
    let mut cpu = source(PresentationMode::CpuRgba);
    cpu.frames.push_back(Frame::Cpu(CpuFrame {
        stamp: FrameStamp {
            scope: cpu.scope(),
            allocation_epoch: 1,
            content_revision: 1,
        },
        size: FrameSize::new(16, 16),
        stride: 64,
        rgba: vec![200; 16 * 16 * 4],
        alpha: Alpha::Straight,
        encoding: Encoding::Srgb,
    }));
    assert!(
        presenter
            .present(&host, Source::Native(&mut cpu), v, &mut importer)
            .unwrap()
            .is_some()
    );
    cpu.frames.push_back(Frame::Cpu(CpuFrame {
        stamp: FrameStamp {
            scope: cpu.scope(),
            allocation_epoch: 1,
            content_revision: 2,
        },
        size: FrameSize::new(16, 16),
        stride: 64,
        rgba: vec![100; 16 * 16 * 4],
        alpha: Alpha::Straight,
        encoding: Encoding::Srgb,
    }));
    assert!(
        matches!(
            presenter.present(&host, Source::Native(&mut cpu), v, &mut importer),
            Err(Error::Invalid(_))
        ),
        "replacement without allocation advance accepted"
    );
    eprintln!(
        "native custody: pending retry, fence readiness, retained frame, generation retirement, CPU route and invalid replacement passed"
    );
}

impl TextureSource for Producer {
    fn render(&mut self, _: &RenderContext<'_>, _: bool) -> Result<Rendered> {
        self.protocol.ensure_live()?;
        Ok(self.renders.pop_front().unwrap_or(Rendered::Unchanged))
    }
}
#[test]
#[ignore = "requires native Metal; run explicitly"]
fn host_device_refuses_unreusable_stale_and_wrong_device_frames() {
    let host = host();
    let v = Viewport {
        width: 16,
        height: 16,
        scale: 1.,
    };
    let mut producer = source(PresentationMode::HostDevice);
    let mut presenter = Presenter::default();
    let mut importer = GraftMetalImporter;
    assert!(matches!(
        presenter.present(&host, Source::HostDevice(&mut producer), v, &mut importer),
        Err(Error::Invalid(_))
    ));
    let cx = host.context(v).unwrap();
    let texture = cx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("host direction fixture"),
        size: wgpu::Extent3d {
            width: 16,
            height: 16,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let frame = |scope, device_epoch, revision| TextureFrame {
        texture: texture.clone(),
        stamp: FrameStamp {
            scope,
            allocation_epoch: revision,
            content_revision: revision,
        },
        device_epoch,
        alpha: Alpha::Straight,
        encoding: Encoding::Srgb,
    };
    producer.renders.push_back(Rendered::Changed(frame(
        Scope {
            owner: 999,
            generation: 1,
        },
        cx.device_epoch,
        1,
    )));
    assert!(matches!(
        presenter.present(&host, Source::HostDevice(&mut producer), v, &mut importer),
        Err(Error::Stale)
    ));
    let other = GpuHost::new(cx.device.clone(), cx.queue.clone()).unwrap();
    producer
        .renders
        .push_back(Rendered::Changed(frame(producer.scope(), other.epoch(), 1)));
    assert!(matches!(
        presenter.present(&host, Source::HostDevice(&mut producer), v, &mut importer),
        Err(Error::Stale)
    ));
    producer.renders.push_back(Rendered::Changed(frame(
        producer.scope(),
        cx.device_epoch,
        1,
    )));
    assert!(
        presenter
            .present(&host, Source::HostDevice(&mut producer), v, &mut importer)
            .unwrap()
            .is_some()
    );
    assert!(
        presenter
            .present(&host, Source::HostDevice(&mut producer), v, &mut importer)
            .unwrap()
            .is_some()
    );
    let resized = Viewport { width: 32, ..v };
    assert!(
        matches!(
            presenter.present(
                &host,
                Source::HostDevice(&mut producer),
                resized,
                &mut importer
            ),
            Err(Error::Invalid(_))
        ),
        "unchanged survived resize without a compatible texture"
    );
}

fn read_pixels(host: &GpuHost, texture: &wgpu::Texture) -> Vec<u8> {
    let cx = host
        .context(Viewport {
            width: 16,
            height: 16,
            scale: 1.,
        })
        .unwrap();
    let buffer = cx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("native fixture readback"),
        size: 256 * 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = cx.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(16),
            },
        },
        texture.size(),
    );
    cx.queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        sender.send(result).unwrap();
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        cx.device.poll(wgpu::PollType::Poll).unwrap();
        if let Ok(result) = receiver.try_recv() {
            result.unwrap();
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "native fixture readback timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let bytes = buffer.get_mapped_range(..).unwrap().to_vec();
    buffer.unmap();
    bytes
}
