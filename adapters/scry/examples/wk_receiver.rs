// SPDX-License-Identifier: MPL-2.0
//! Independent receiving host: no Inker, Pelt, Mere or Turnstone.
//! `cargo run -p wgpu-cue-scry --example wk_receiver`
#![cfg_attr(not(target_os = "macos"), allow(unused))]
#[cfg(target_os = "macos")]
mod native {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::time::{Duration, Instant};
    use webcue::*;
    use wgpu_cue::*;
    use wgpu_cue_scry::*;
    use winit::{
        application::ApplicationHandler,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
        window::{Window, WindowId},
    };
    #[derive(Default)]
    pub struct App {
        surface: Option<WkSurface>,
        window: Option<Window>,
        host: Option<GpuHost>,
        presenter: Presenter,
        start: Option<Instant>,
        navigation: bool,
        script: bool,
        focus: bool,
        frame: bool,
        last_sequence: u64,
        phase: u8,
        pub passed: bool,
    }
    impl ApplicationHandler for App {
        fn resumed(&mut self, el: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let window = el
                .create_window(
                    Window::default_attributes()
                        .with_title("wgpu-cue independent receiver")
                        .with_inner_size(winit::dpi::LogicalSize::new(320., 200.)),
                )
                .unwrap();
            let size = window.inner_size();
            let viewport = Viewport {
                width: size.width,
                height: size.height,
                scale: window.scale_factor() as f32,
            };
            let RawWindowHandle::AppKit(handle) = window.window_handle().unwrap().as_raw() else {
                panic!("AppKit required")
            };
            // The live owned window retains this NSView until after the producer is dropped.
            let producer = unsafe {
                scrying::WkWebViewProducer::new(
                    handle.ns_view.as_ptr(),
                    scrying::WkWebViewProducerConfig::new(
                        dpi::PhysicalSize::new(size.width, size.height),
                        std::path::PathBuf::new(),
                    )
                    .non_persistent(),
                )
            }
            .unwrap();
            let mut surface = WkSurface::new(
                producer,
                Scope {
                    owner: 9234,
                    generation: 1,
                },
                viewport,
                WkFrames::Snapshot,
            )
            .unwrap();
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::METAL,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter =
                pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
            eprintln!("WK receiver GPU: {:?}", adapter.get_info());
            let (device, queue) =
                pollster::block_on(adapter.request_device(&Default::default())).unwrap();
            surface.request(RequestId(1),Command::LoadHtml {address:"https://cue.invalid/fixture".into(),html:"<html><head><title>Cue WK proof</title></head><body style='background:#208040'><label>Name<input id='name'></label><p>Independent Webcue receiver</p></body></html>".into()}).unwrap();
            self.host = Some(GpuHost::new(device, queue).unwrap());
            self.surface = Some(surface);
            self.window = Some(window);
            self.start = Some(Instant::now());
        }
        fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
            if matches!(event, WindowEvent::CloseRequested) {
                self.surface.take();
                el.exit();
            }
        }
        fn about_to_wait(&mut self, el: &ActiveEventLoop) {
            let Some(surface) = &mut self.surface else {
                return;
            };
            let now = self.start.unwrap().elapsed();
            assert!(
                now < Duration::from_secs(30),
                "WK receiver timed out; nav={} script={} focus={} frame={}",
                self.navigation,
                self.script,
                self.focus,
                self.frame
            );
            surface.advance(now).unwrap();
            while let Some(e) = surface.poll_event() {
                assert!(e.sequence > self.last_sequence);
                self.last_sequence = e.sequence;
                match e.event {
                    Event::NavigationFinished { success, .. } => {
                        assert!(success);
                        self.navigation = true;
                    }
                    Event::Completed { request, result } => match (request.id.0, result) {
                        (1, Ok(Reply::Dispatched)) => {}
                        (2, Ok(Reply::Text(v))) => {
                            assert_eq!(v, "42");
                            self.script = true;
                        }
                        (3, Ok(Reply::Text(v))) => {
                            assert_eq!(v, "true");
                            self.focus = true;
                        }
                        (_, Err(e)) => panic!("WK request {} failed: {e}", request.id.0),
                        _ => {}
                    },
                    Event::TextInput(Some(state)) => {
                        assert!(state.caret.width.is_finite());
                        self.focus = true;
                    }
                    _ => {}
                }
            }
            if self.navigation && self.phase == 0 {
                surface
                    .request(RequestId(2), Command::Script("6*7".into()))
                    .unwrap();
                self.phase = 1;
            }
            if self.script && self.phase == 1 {
                surface.request(RequestId(3),Command::Script("document.getElementById('name').focus(); document.activeElement.id === 'name'".into())).unwrap();
                self.phase = 2;
            }
            let size = self.window.as_ref().unwrap().inner_size();
            let viewport = Viewport {
                width: size.width,
                height: size.height,
                scale: self.window.as_ref().unwrap().scale_factor() as f32,
            };
            if self.navigation
                && let Some(frame) = self
                    .presenter
                    .present(
                        self.host.as_ref().unwrap(),
                        Source::Native(surface),
                        viewport,
                        &mut GraftMetalImporter,
                    )
                    .unwrap()
            {
                assert_eq!(frame.texture.width(), viewport.width);
                assert_eq!(frame.texture.height(), viewport.height);
                self.frame = true;
            }
            if self.navigation && self.script && self.focus && self.frame {
                surface
                    .request(RequestId(4), Command::Script("1+1".into()))
                    .unwrap();
                surface.retire().unwrap();
                assert!(std::iter::from_fn(||surface.poll_event()).any(|e|matches!(e.event,Event::Completed {request,result:Err(Error::Retired)} if request.id==RequestId(4))));
                eprintln!(
                    "WK receiver PASS: ordered navigation, correlated script, DOM focus acknowledgment (no WK caret observation claimed), non-blocking CPU snapshot uploaded through the shared presenter; retirement settled accepted work"
                );
                self.passed = true;
                self.surface.take();
                el.exit();
            }
            el.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(8),
            ));
        }
    }
    pub fn run() {
        let event_loop = EventLoop::new().unwrap();
        let mut app = App::default();
        event_loop.run_app(&mut app).unwrap();
        assert!(app.passed, "receiver closed before qualification");
    }
}
#[cfg(target_os = "macos")]
fn main() {
    native::run();
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("WK receiver requires macOS");
    std::process::exit(1);
}
