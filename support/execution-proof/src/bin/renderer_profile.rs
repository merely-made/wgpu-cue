// SPDX-License-Identifier: MPL-2.0
//! Serialized native GPU attribution probe. Cache policies are diagnostic only.
//! CPU render/submit spans exclude completion wait, readback and pixel checks.
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::PathBuf,
    time::{Duration, Instant},
};
use webcue::{Input, Scope, Viewport, execution::ExecutionId};
use wgpu_cue::{GpuHost, wgpu};
use wgpu_cue_genet::{GenetDocument, HeldResources};
use wgpu_cue_netrender::{
    netrender::{
        ImageData, Scene, SceneOp,
        vello_backends::{HybridSession, SparseResourceLimits, SparseTextLimits},
    },
    resource_table::{ResourceReceiver, ResourceSender},
    scene_job::{RenderLimits, SceneSnapshot},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const VARIANTS: usize = 8;
const WARMUP: usize = 8;
const POLICIES: [&str; 4] = ["clear-both", "keep-text", "keep-images", "keep-both"];
fn gpu() -> Result<(wgpu::Adapter, GpuHost)> {
    #[cfg(target_os = "macos")]
    {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::METAL,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
        Ok((adapter, GpuHost::new(device, queue)?))
    }
    #[cfg(target_os = "linux")]
    {
        let (adapter, host, _) = cue_execution_proof::vulkan_handoff::gpu()?;
        Ok((adapter, host))
    }
    #[cfg(target_os = "windows")]
    {
        let (adapter, host, _) = cue_execution_proof::dx12_handoff::gpu()?;
        Ok((adapter, host))
    }
}
fn size(scene: &Scene) -> Viewport {
    Viewport {
        width: scene.viewport_width,
        height: scene.viewport_height,
        scale: 1.,
    }
}
fn scenes(font: &str) -> Result<Vec<Scene>> {
    let mut viewport = Viewport {
        width: 800,
        height: 600,
        scale: 1.,
    };
    let mut document = GenetDocument::new(
        viewport,
        "https://fixture.invalid/receiver".into(),
        cue_execution_proof::scene_fixture::html(font),
        HeldResources::default(),
    )?;
    let mut result = vec![];
    for i in 0..VARIANTS {
        if i == VARIANTS / 2 {
            viewport.width = 960;
            viewport.height = 640;
        }
        document.input(Input::Scroll {
            x: 100.,
            y: 100.,
            dx: 0.,
            dy: 12.,
            modifiers: Default::default(),
        })?;
        result.push(document.prepare(viewport)?);
    }
    assert!(
        result
            .iter()
            .all(|s| s.iter_glyph_runs().next().is_some() && s.image_sources.is_empty())
    );
    Ok(result)
}
fn workload(base: &[Scene], name: &str) -> Vec<Scene> {
    // One bounded RGBA image with changing content halfway through the cycle.
    // This supplement is intentionally separate from the image-free window input.
    let images: Vec<_> = (0..2)
        .map(|variant| {
            let pixels = (0..256 * 256)
                .flat_map(|i| {
                    [
                        ((i / 256 + variant * 60) % 256) as u8,
                        (i % 256) as u8,
                        160,
                        if i % 3 == 0 { 128 } else { 255 },
                    ]
                })
                .collect();
            ImageData::from_bytes(256, 256, pixels)
        })
        .collect();
    base.iter()
        .enumerate()
        .map(|(i, s)| {
            let mut s = s.clone();
            if matches!(name, "geometry" | "images") {
                s.ops.retain(|op| !matches!(op, SceneOp::GlyphRun(_)));
                s.fonts.truncate(1);
            }
            if matches!(name, "images" | "mixed") {
                s.push_image(8., 8., 264., 264., 77, images[i / 4].clone());
            }
            s
        })
        .collect()
}
fn target(device: &wgpu::Device, scene: &Scene) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("renderer profile target"),
        size: wgpu::Extent3d {
            width: scene.viewport_width,
            height: scene.viewport_height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}
fn read(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Result<Vec<u8>> {
    let width = texture.width();
    let height = texture.height();
    let stride = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("renderer profile readback"),
        size: stride as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
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
    let index = queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    device.poll(wgpu::PollType::Wait {
        submission_index: Some(index),
        timeout: Some(Duration::from_secs(30)),
    })?;
    rx.recv_timeout(Duration::from_secs(30))??;
    let mapped = buffer.get_mapped_range(..)?;
    let pixels = mapped
        .chunks(stride as usize)
        .flat_map(|row| row[..(width * 4) as usize].iter().copied())
        .collect();
    drop(mapped);
    buffer.unmap();
    Ok(pixels)
}
fn ns(start: Instant) -> u64 {
    start.elapsed().as_nanos() as u64
}
fn hash(pixels: &[u8]) -> String {
    // Diagnostic checksum only; equivalence below uses complete byte comparison.
    format!(
        "{:016x}",
        pixels
            .iter()
            .fold(0xcbf29ce484222325u64, |v, b| (v ^ *b as u64)
                .wrapping_mul(0x100000001b3))
    )
}
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let output=PathBuf::from(args.next().ok_or("usage: cue-renderer-profile FRESH_ABSOLUTE_OUTPUT FONT [--only WORKLOAD/DELIVERY/POLICY] [--frames N]")?);
    let font = args.next().ok_or("font required")?;
    let mut only = None;
    let mut frames = 40usize;
    let mut reverse = false;
    let mut export_pixels = false;
    let mut compare_pixels: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--only" => only = Some(args.next().ok_or("case required")?),
            "--reverse" => reverse = true,
            "--export-pixels" => export_pixels = true,
            "--compare-pixels" => {
                compare_pixels = Some(PathBuf::from(
                    args.next().ok_or("reference directory required")?,
                ))
            }
            "--frames" => frames = args.next().ok_or("frames required")?.parse()?,
            _ => return Err("unknown argument".into()),
        }
    }
    if !output.is_absolute()
        || output.exists()
        || !["Arial", "sans-serif", "monospace"].contains(&font.as_str())
        || !(16..=4096).contains(&frames)
    {
        return Err("fresh absolute output, supported font and 16..4096 frames required".into());
    }
    if export_pixels && compare_pixels.is_some() {
        return Err("choose export or comparison, not both".into());
    }
    if compare_pixels
        .as_ref()
        .is_some_and(|p| !p.is_absolute() || !p.is_dir())
    {
        return Err("reference pixels require an existing absolute directory".into());
    }
    // Long stack-sampling runs must be restricted to one selected case.
    if frames > 72 && only.is_none() {
        return Err("more than 72 frames requires --only".into());
    }
    let (adapter, host) = gpu()?;
    let info = adapter.get_info();
    assert_ne!(
        info.device_type,
        wgpu::DeviceType::Cpu,
        "hardware adapter required"
    );
    let seed = scenes(&font)?;
    std::fs::create_dir(&output)?;
    if export_pixels {
        std::fs::create_dir(output.join("pixels"))?;
    }
    let mut rows = vec![];
    let mut cases = vec![];
    for name in ["document", "geometry", "images", "mixed"] {
        let variants = workload(&seed, name);
        // Every case gets an independently cleared reference for all variants.
        let mut reference: Vec<Option<Vec<u8>>> = (0..VARIANTS).map(|_| None).collect();
        for delivery in ["typed-fresh", "table-current", "table-interned-control"] {
            let mut policies = POLICIES;
            if reverse {
                policies.reverse();
            }
            for policy in policies {
                let case = format!("{name}/{delivery}/{policy}");
                if only.as_ref().is_some_and(|selected| selected != &case) {
                    continue;
                }
                eprintln!("case {case} pid={}", std::process::id());
                let limits = RenderLimits::default();
                let cx = host.context(size(&variants[0]))?;
                let mut session = HybridSession::new_with_text_limits(
                    cx.device,
                    cx.queue,
                    SparseResourceLimits {
                        max_source_bytes: limits.resource_bytes,
                        max_cached_bytes: limits.resource_bytes,
                        max_viewport_pixels: limits.frame_bytes / 4,
                        ..Default::default()
                    },
                    SparseTextLimits {
                        max_font_bytes: limits.resource_bytes,
                        ..Default::default()
                    },
                )?;
                let execution = ExecutionId {
                    scope: Scope {
                        owner: 901,
                        generation: 1,
                    },
                    incarnation: 1,
                };
                let mut sender = ResourceSender::new(execution, limits)?;
                let mut receiver = ResourceReceiver::new(execution, limits)?;
                let mut interned: Vec<wgpu_cue_netrender::netrender::peniko::Blob<u8>> = vec![];
                let mut equal = 0;
                for iteration in 0..frames {
                    let variant = iteration % VARIANTS;
                    let original = &variants[variant];
                    let cx = host.context(size(original))?;
                    let start = Instant::now();
                    let mut snapshot = SceneSnapshot::capture_owned(original.clone(), limits)?;
                    if delivery != "typed-fresh" {
                        let (packet, next) = sender.prepare(&snapshot)?;
                        // Serialization is outside render spans; the receiver's full
                        // packet validation and lifetime semantics remain exercised.
                        let packet = serde_json::from_slice(&serde_json::to_vec(&packet)?)?;
                        snapshot = receiver.receive(packet)?;
                        sender = next;
                    }
                    let mut scene = snapshot.decode(limits)?;
                    if delivery == "table-interned-control" {
                        // Diagnostic counterfactual only. Current table receive
                        // remints IDs through capture_owned; intern already-validated
                        // local immutable owners by exact bytes to isolate that cost.
                        for blob in scene
                            .fonts
                            .iter_mut()
                            .map(|f| &mut f.data)
                            .chain(scene.image_sources.values_mut().map(|i| &mut i.data))
                            .filter(|b| !b.is_empty())
                        {
                            if let Some(prior) = interned
                                .iter()
                                .find(|prior| prior.as_ref() == blob.as_ref())
                            {
                                *blob = prior.clone();
                            } else {
                                assert!(interned.len() < 256);
                                assert!(
                                    interned.iter().map(|b| b.len()).sum::<usize>() + blob.len()
                                        <= limits.resource_bytes
                                );
                                interned.push(blob.clone());
                            }
                        }
                    }
                    let delivery_ns = ns(start);
                    let before = session.text_stats();
                    let images_before = session.stats();
                    let start = Instant::now();
                    if matches!(policy, "clear-both" | "keep-text") {
                        session.clear_images();
                    }
                    let clear_images_ns = ns(start);
                    let start = Instant::now();
                    if matches!(policy, "clear-both" | "keep-images") {
                        session.clear_text_cache();
                    }
                    let clear_text_ns = ns(start);
                    let start = Instant::now();
                    let texture = target(cx.device, &scene);
                    let target_ns = ns(start);
                    let start = Instant::now();
                    let mut encoder = cx.device.create_command_encoder(&Default::default());
                    let encoder_ns = ns(start);
                    let start = Instant::now();
                    session.render(&scene, cx.device, cx.queue, &mut encoder, &texture)?;
                    let render_ns = ns(start);
                    let start = Instant::now();
                    let command = encoder.finish();
                    let finish_ns = ns(start);
                    let start = Instant::now();
                    let submitted = cx.queue.submit([command]);
                    let submit_ns = ns(start);
                    let start = Instant::now();
                    cx.device.poll(wgpu::PollType::Wait {
                        submission_index: Some(submitted),
                        timeout: Some(Duration::from_secs(30)),
                    })?;
                    let completion_wait_ns = ns(start);
                    let start = Instant::now();
                    let pixels = read(cx.device, cx.queue, &texture)?;
                    let readback_ns = ns(start);
                    if let Some(expected) = &reference[variant] {
                        assert!(
                            &pixels == expected,
                            "pixel mismatch {case} variant={variant}"
                        );
                    } else {
                        let expected = if let Some(directory) = &compare_pixels {
                            let file = std::fs::File::open(
                                directory.join(format!("{name}-{variant}.rgba.gz")),
                            )?;
                            let decoder = flate2::read::GzDecoder::new(file);
                            let expected_bytes =
                                scene.viewport_width as u64 * scene.viewport_height as u64 * 4;
                            let mut bytes = Vec::new();
                            decoder.take(expected_bytes + 1).read_to_end(&mut bytes)?;
                            if bytes.len() as u64 != expected_bytes {
                                return Err("reference pixel size differs".into());
                            }
                            bytes
                        } else {
                            // --only retained runs still compare against an actual,
                            // independent clear-both render rather than self-reference.
                            let mut control = HybridSession::new(
                                cx.device,
                                cx.queue,
                                SparseResourceLimits::default(),
                            )?;
                            let target = target(cx.device, &scene);
                            let mut encoder = cx.device.create_command_encoder(&Default::default());
                            control.render(&scene, cx.device, cx.queue, &mut encoder, &target)?;
                            let index = cx.queue.submit([encoder.finish()]);
                            cx.device.poll(wgpu::PollType::Wait {
                                submission_index: Some(index),
                                timeout: Some(Duration::from_secs(30)),
                            })?;
                            read(cx.device, cx.queue, &target)?
                        };
                        assert!(
                            pixels == expected,
                            "reference pixels differ: {case} variant={variant}"
                        );
                        if export_pixels {
                            let file = std::fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(
                                    output
                                        .join("pixels")
                                        .join(format!("{name}-{variant}.rgba.gz")),
                                )?;
                            let mut encoder =
                                flate2::write::GzEncoder::new(file, flate2::Compression::fast());
                            encoder.write_all(&expected)?;
                            encoder.finish()?;
                        }
                        reference[variant] = Some(expected);
                    }
                    equal += 1;
                    let text = session.text_stats();
                    let images = session.stats();
                    let glyphs: usize = scene.iter_glyph_runs().map(|run| run.glyphs.len()).sum();
                    rows.push(json!({"case":case,"iteration":iteration,"warmup":iteration<WARMUP,"variant":variant,
                        "viewport":[scene.viewport_width,scene.viewport_height],"ops":scene.ops.len(),"glyphs":glyphs,
                        "font_ids":scene.fonts.iter().filter(|f| !f.data.is_empty()).map(|f| f.data.id()).collect::<Vec<_>>(),
                        "image_ids":scene.image_sources.values().map(|i| i.data.id()).collect::<Vec<_>>(),
                        "delivery_ns":delivery_ns,"clear_images_ns":clear_images_ns,"clear_text_ns":clear_text_ns,
                        "target_ns":target_ns,"encoder_ns":encoder_ns,"render_ns":render_ns,"finish_ns":finish_ns,
                        "submit_ns":submit_ns,"completion_wait_ns":completion_wait_ns,"readback_ns":readback_ns,
                        "cpu_submission_ns":clear_images_ns+clear_text_ns+target_ns+encoder_ns+render_ns+finish_ns+submit_ns,
                        "prepared_glyphs_delta":text.prepared_glyphs-before.prepared_glyphs,"culled_glyphs_delta":text.culled_glyphs-before.culled_glyphs,"cache_resets_delta":text.cache_resets-before.cache_resets,
                        "font_count":text.font_count,"font_bytes":text.font_bytes,"cached_glyphs":text.cached_glyphs,
                        "hydrated_images_delta":images.hydrated_images-images_before.hydrated_images,
                        "uploaded_images_delta":images.uploaded_images-images_before.uploaded_images,
                        "cached_images":images.cached_images,"gpu_texture_bytes":images.gpu_texture_bytes,
                        "pixels_equal":true,"pixel_checksum_fnv64":hash(&pixels)}));
                }
                cases.push(json!({"case":case,"frames":frames,"pixel_comparisons":equal,
                    "table_entries":sender.entries(),"table_bytes":sender.retained_bytes(),"table_packets":sender.packets()}));
                std::fs::write(output.join("rows.json"), serde_json::to_vec(&rows)?)?;
            }
        }
    }
    if cases.is_empty() {
        return Err("--only did not match a case".into());
    }
    #[cfg(target_os = "windows")]
    {
        let cx = host.context(size(&seed[0]))?;
        cue_execution_proof::dx12_handoff::check_validation(&grafting::HostWgpuContext::new(
            cx.device.clone(),
            cx.queue.clone(),
        ))?;
    }
    let summary: Value = json!({"schema":"cue-renderer-probe-v1","adapter":format!("{info:?}"),"font":font,
        "pixel_reference":compare_pixels,"exported_pixels":export_pixels,"reverse_policy_order":reverse,"frames_per_case":frames,"warmup":WARMUP,"variants":VARIANTS,"cases":cases,
        "note":"Same pinned supplier; opt-in cache ablations only. CPU spans exclude wait/readback/checks. Completion wait is CPU-observed, not GPU timestamp. Fresh/table delivery measured separately. No presentation or 60Hz qualification."});
    std::fs::write(
        output.join("summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    Ok(())
}
