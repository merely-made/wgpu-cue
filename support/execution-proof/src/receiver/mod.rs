// SPDX-License-Identifier: MPL-2.0
//! Private window workload. Raw present calls and OS feedback are separate.
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use webcue::*;
use wgpu_cue::*;
use wgpu_cue_genet::{GenetDocument, HeldResources};
use wgpu_cue_netrender::{
    render_session::RenderClock,
    scene_job::{RenderLimits, SceneSnapshot},
};
use winit::{
    application::ApplicationHandler,
    event::{MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId},
};
mod blit;
mod config;
use config::Config;
#[cfg(windows)]
mod dxgi_feedback;
mod feedback;
mod metrics;
mod renderer;
mod report;
#[cfg(target_os = "linux")]
mod vulkan_feedback;
#[cfg(target_os = "linux")]
mod wayland_feedback;
type AnyResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn scope() -> Scope {
    Scope {
        owner: (1u128 << 100) + 901,
        generation: 1,
    }
}
fn viewport_for(width: u32, height: u32) -> Viewport {
    Viewport {
        width,
        height,
        scale: 1.,
    }
}
struct Stress {
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}
impl Stress {
    fn new() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let threads = (0..2)
            .map(|n| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let mut x = n + 1u64;
                    while !stop.load(Ordering::Relaxed) {
                        for _ in 0..10_000 {
                            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                            std::hint::black_box(x);
                        }
                    }
                })
            })
            .collect();
        Self { stop, threads }
    }
}
impl Drop for Stress {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}
struct Note {
    input: Option<f64>,
    content: u64,
}
struct Receiver {
    // Drop Surface and its callbacks before the final Window owner.
    surface: wgpu::Surface<'static>,
    feedback: feedback::Feedback,
    window: Arc<Window>,
    host: GpuHost,
    config: wgpu::SurfaceConfiguration,
    blit: blit::Blit,
    renderer: renderer::Renderer,
    document: GenetDocument,
    clock: RenderClock,
    viewport: Viewport,
    pending_viewport: Viewport,
    current: Option<TextureFrame>,
    held: Vec<(FrameStamp, wgpu::TextureView, f64)>,
    notes: BTreeMap<u64, Note>,
    revision: u64,
    content_sequence: u64,
    sizes: Vec<(u32, u32)>,
    current_note: Note,
    input: Option<f64>,
    next_input: f64,
    inputs: u64,
    handled: u64,
    frames: Vec<metrics::Frame>,
    started: Instant,
    stress: Option<Stress>,
    resized: bool,
    max_imports: usize,
    adapter: String,
    errors: Arc<std::sync::Mutex<Vec<String>>>,
    skipped: u64,
}
impl Receiver {
    fn new(el: &ActiveEventLoop, cfg: &Config) -> AnyResult<Self> {
        let window = Arc::new(
            el.create_window(
                Window::default_attributes()
                    .with_title("Cue native handoff pacing — scroll to interact")
                    .with_inner_size(winit::dpi::PhysicalSize::new(800, 600)),
            )?,
        );
        let (instance, adapter, host) = renderer::gpu()?;
        let info = adapter.get_info();
        if !matches!(
            info.device_type,
            wgpu::DeviceType::DiscreteGpu | wgpu::DeviceType::IntegratedGpu
        ) {
            return Err("hardware GPU required".into());
        }
        eprintln!("Window receiver: {info:?}");
        let (surface, mut feedback) =
            feedback::Feedback::surface(&instance, window.clone(), &host)?;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| !f.is_srgb())
            .ok_or("linear swapchain format required")?;
        if !caps.present_modes.contains(&wgpu::PresentMode::Fifo) {
            return Err("FIFO present required".into());
        }
        window.focus_window();
        if !feedback.refresh.is_finite() || feedback.refresh <= 0. {
            return Err("display refresh unavailable".into());
        }
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            color_space: Default::default(),
        };
        let viewport = viewport_for(size.width, size.height);
        let cx = host.context(viewport)?;
        let errors = Arc::new(std::sync::Mutex::new(Vec::new()));
        let reported = errors.clone();
        cx.device.on_uncaptured_error(Arc::new(move |e| {
            let mut out = reported.lock().unwrap();
            if out.len() < 32 {
                out.push(e.to_string())
            }
        }));
        surface.configure(cx.device, &config);
        let blit = blit::Blit::new(cx.device, format);
        let rows=(0..120).map(|i|format!("<section style='background:#{:02x}4060;padding:12px;margin:6px;border:2px solid #90b0d0'><b>Record {i}</b> — owned document, native frame transfer, glyphs and borders. <span>Scroll this retained document.</span></section>",32+i%80)).collect::<String>();
        let html = format!(
            "<!doctype html><style>body{{margin:0;background:#203050;color:white;font:16px {}}}h1{{margin:12px}}</style><h1>Genet renderer process → receiving window</h1>{rows}",
            cfg.font
        );
        let mut document = GenetDocument::new(
            viewport,
            "https://fixture.invalid/receiver".into(),
            html,
            HeldResources::default(),
        )?;
        let scene = document.prepare(viewport)?;
        report::inputs(cfg, &scene)?;
        let snapshot = SceneSnapshot::capture(&scene, RenderLimits::default())?;
        let renderer = renderer::Renderer::new(cfg.local, &cfg.helper, &host, viewport, &snapshot)?;
        feedback.begin()?;
        Ok(Self {
            surface,
            feedback,
            window,
            host,
            config,
            blit,
            renderer,
            document,
            clock: RenderClock::new(scope()),
            viewport,
            pending_viewport: viewport,
            current: None,
            held: vec![],
            notes: BTreeMap::new(),
            revision: 0,
            content_sequence: 0,
            sizes: vec![(size.width, size.height)],
            current_note: Note {
                input: None,
                content: 0,
            },
            input: None,
            next_input: 0.,
            inputs: 0,
            handled: 0,
            frames: vec![],
            started: Instant::now(),
            stress: None,
            resized: false,
            max_imports: 0,
            adapter: format!("{info:?}"),
            errors,
            skipped: 0,
        })
    }
    fn scroll(&mut self, dx: f32, dy: f32, t: f64) -> AnyResult<()> {
        let result = self.document.input(Input::Scroll {
            x: 100.,
            y: 100.,
            dx,
            dy,
            modifiers: Default::default(),
        })?;
        self.inputs += 1;
        if matches!(result.effect, webcue::document::SessionEffect::Handled) {
            self.handled += 1;
            self.content_sequence += 1;
            self.input.get_or_insert(t);
        }
        Ok(())
    }
    fn drop_current(&mut self, phase: &str, t: f64) -> AnyResult<()> {
        if let Some(old) = self.current.take() {
            if phase == "custody" && self.held.is_empty() {
                self.held.push((
                    old.stamp,
                    old.texture.create_view(&Default::default()),
                    t + 0.1,
                ));
            }
            self.renderer.retire_frame(old.stamp)?;
        }
        Ok(())
    }
    fn resize(&mut self, width: u32, height: u32) -> AnyResult<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        if self.viewport.width != width || self.viewport.height != height {
            if self.sizes.len() >= 32 {
                return Err("resize receipt bound exceeded".into());
            }
            self.sizes.push((width, height));
            self.content_sequence += 1;
        }
        self.viewport = viewport_for(width, height);
        self.config.width = width;
        self.config.height = height;
        self.feedback.resized();
        self.surface
            .configure(self.host.context(self.viewport)?.device, &self.config);
        Ok(())
    }
    fn tick(&mut self, cfg: &Config) -> AnyResult<bool> {
        let t = self.started.elapsed().as_secs_f64();
        let phase = cfg.phase(t);
        self.feedback.drain(&self.surface, &mut self.frames)?;
        self.host
            .context(self.viewport)?
            .device
            .poll(wgpu::PollType::Poll)?;
        self.held.retain(|(_, _, deadline)| t < *deadline);
        if phase == "loaded" && self.stress.is_none() {
            self.stress = Some(Stress::new());
        }
        if phase == "loaded" && !self.resized {
            if let Some(size) = self
                .window
                .request_inner_size(winit::dpi::PhysicalSize::new(960, 640))
            {
                self.resize(size.width, size.height)?;
            }
            self.resized = true;
        }
        if phase == "custody" {
            self.stress.take();
        }
        let update = Instant::now();
        if let Some(frame) = self.renderer.poll(
            &self.host,
            self.pending_viewport,
            &self.held.iter().map(|h| h.0).collect::<Vec<_>>(),
        )? {
            self.drop_current(phase, t)?;
            self.current_note = self
                .notes
                .remove(&frame.stamp.content_revision)
                .ok_or("uncorrelated rendered revision")?;
            self.current = Some(frame);
        }
        self.max_imports = self.max_imports.max(self.renderer.imports());
        if self.renderer.imports() > 2 {
            return Err("two-slot import bound exceeded".into());
        }
        if phase == "drain" {
            self.stress.take();
            self.drop_current(phase, t)?;
            if t >= cfg.end() {
                if self.renderer.pending() || self.renderer.imports() != 0 || !self.held.is_empty()
                {
                    return Err("frame slots did not drain".into());
                }
                renderer::validate(&self.host, self.viewport)?;
                self.renderer.stop()?;
                return Ok(true);
            }
            return Ok(false);
        }
        if t >= self.next_input {
            let dy = if ((t / 2.) as u64).is_multiple_of(2) {
                12.
            } else {
                -12.
            };
            self.scroll(0., dy, t)?;
            self.next_input += ((t - self.next_input) * 60.).floor().max(0.) / 60. + 1. / 60.;
        }
        let mut prepare_ms = 0.;
        if !self.renderer.pending()
            && (self.revision == 0 || self.current_note.content != self.content_sequence)
        {
            let prepare = Instant::now();
            let scene = self.document.prepare(self.viewport)?;
            let snapshot = SceneSnapshot::capture(&scene, RenderLimits::default())?;
            prepare_ms = prepare.elapsed().as_secs_f64() * 1000.;
            self.revision += 1;
            let job = self.clock.job(self.revision, snapshot)?;
            self.notes.insert(
                self.revision,
                Note {
                    input: self.input.take(),
                    content: self.content_sequence,
                },
            );
            self.renderer.submit(&self.host, self.viewport, job)?;
            self.pending_viewport = self.viewport;
        }
        let update_ms = update.elapsed().as_secs_f64() * 1000.;
        if let Some(frame) = &self.current {
            let acquire = Instant::now();
            let image = match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t)
                | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
                wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                    self.skipped += 1;
                    return Ok(false);
                }
                wgpu::CurrentSurfaceTexture::Outdated => {
                    self.resize(self.viewport.width, self.viewport.height)?;
                    return Ok(false);
                }
                other => return Err(format!("surface acquisition {other:?}").into()),
            };
            let acquire_ms = acquire.elapsed().as_secs_f64() * 1000.;
            let cx = self.host.context(self.viewport)?;
            self.blit.draw(&cx, frame, &image.texture);
            self.feedback
                .prepare(&self.surface, self.frames.len() as u64 + 1)?;
            self.window.pre_present_notify();
            cx.queue.present(image);
            let id = self
                .feedback
                .id(&self.surface, self.frames.len() as u64 + 1)?;
            if self.frames.len() >= 8000 {
                return Err("frame receipt bound exceeded".into());
            }
            self.frames.push(metrics::Frame {
                id,
                revision: frame.stamp.content_revision,
                content_sequence: self.current_note.content,
                phase,
                present_call_s: self.started.elapsed().as_secs_f64(),
                os_present_s: None,
                input_s: self.current_note.input,
                acquire_ms,
                prepare_ms,
                update_ms,
            });
        }
        let cx = self.host.context(self.viewport)?;
        cx.device.poll(wgpu::PollType::Poll)?;
        if !self.errors.lock().unwrap().is_empty() {
            return Err("wgpu validation error; see receipt".into());
        }
        Ok(false)
    }
}
struct App {
    cfg: Config,
    receiver: Option<Receiver>,
    result: Option<AnyResult<bool>>,
}
impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.receiver.is_some() {
            return;
        }
        match Receiver::new(el, &self.cfg) {
            Ok(r) => self.receiver = Some(r),
            Err(e) => {
                self.result = Some(Err(e));
                el.exit();
            }
        }
    }
    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(r) = &mut self.receiver else { return };
        let result: AnyResult<bool> = match event {
            WindowEvent::RedrawRequested => r.tick(&self.cfg),
            WindowEvent::Resized(size) => r.resize(size.width, size.height).map(|_| false),
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x * 16., y * 16.),
                    MouseScrollDelta::PixelDelta(p) => (p.x as f32, p.y as f32),
                };
                r.scroll(dx, dy, r.started.elapsed().as_secs_f64())
                    .map(|_| false)
            }
            WindowEvent::CloseRequested => {
                Err("window closed before qualification completed".into())
            }
            _ => Ok(false),
        };
        match result {
            Ok(false) => {}
            Ok(true) => {
                self.result = Some(r.save(&self.cfg, None));
                el.exit()
            }
            Err(e) => {
                let _ = r.save(&self.cfg, Some(&e.to_string()));
                self.result = Some(Err(e));
                el.exit()
            }
        }
    }
    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        el.set_control_flow(ControlFlow::WaitUntil(
            Instant::now() + Duration::from_millis(1),
        ));
        if let Some(r) = &self.receiver {
            r.window.request_redraw();
        }
    }
}
pub fn run() -> AnyResult<()> {
    let cfg = Config::parse()?;
    let el = EventLoop::new()?;
    let mut app = App {
        cfg,
        receiver: None,
        result: None,
    };
    el.run_app(&mut app)?;
    let result = match app.result.take() {
        Some(Ok(true)) => Ok(()),
        Some(Ok(false)) => Err("pacing target not met; retained raw frames and summary".into()),
        Some(Err(error)) => Err(error),
        None => Err("window ended without result".into()),
    };
    if let Err(error) = &result {
        std::fs::write(
            app.cfg.output.join("error.json"),
            serde_json::to_vec_pretty(&serde_json::json!({"error":error.to_string()}))?,
        )?;
    }
    result
}
