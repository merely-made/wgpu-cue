// SPDX-License-Identifier: MPL-2.0
use scrying::{
    NavigationEvent, WebSurfaceEvent, WebSurfaceFrame, WebSurfaceProducer, WkWebViewProducer,
};
use std::{collections::VecDeque, time::Duration};
use webcue::grafting_frame::{FrameMetadata, FrameSync, MetalTexture, OwnedNativeImage};
use webcue::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WkFrames {
    Snapshot,
    /// Caller-declared conventions of the configured capture pipeline.
    Native {
        alpha: Alpha,
        encoding: Encoding,
    },
}
/// Takes exclusive ownership of a concrete WKWebView producer. Its parent NSView
/// must outlive this wrapper, as required by Scry's constructor. Construction,
/// calls and destruction stay on the platform main thread.
pub struct WkSurface {
    producer: WkWebViewProducer,
    protocol: Protocol,
    report: CapabilityReport,
    viewport: Viewport,
    mode: WkFrames,
    snapshot_pending: bool,
    revision: u64,
    commands: VecDeque<(RequestKey, Command)>,
    outgoing: Option<Event>,
    completion: Option<(RequestKey, Result<Reply>)>,
    report_dirty: bool,
}
fn backend(e: impl std::fmt::Display) -> Error {
    Error::Backend(e.to_string())
}
impl WkSurface {
    /// Native mode requires capture already started with `start_capture_async`.
    /// An unready capture yields no frame; it never falls back silently.
    pub fn new(
        producer: WkWebViewProducer,
        scope: Scope,
        viewport: Viewport,
        mode: WkFrames,
    ) -> Result<Self> {
        let viewport = viewport.validate()?;
        let report=CapabilityReport::new(BackendIdentity {engine:"scry.wkwebview".into(),version:Some("system WebKit / scrying 0.7.2".into()),platform:"macos".into(),build_features:vec!["wgpu-30".into()]},
            match mode {WkFrames::Snapshot=>PresentationMode::CpuRgba,WkFrames::Native {..}=>PresentationMode::NativeFrame},Evidence::Constructed,SemanticProtocol::Unavailable,
            Feature::ALL.map(|f|(f,match f {
                Feature::InlineHtml|Feature::ScriptResult=>Support::Supported,
                Feature::NativeKeyboard=>Support::Partial("AppKit keyCode and associated characters; scan_code is not used".into()),
                Feature::Mouse=>Support::Partial("move and primary/secondary/auxiliary buttons; control/shift modifiers only".into()),
                Feature::Navigation|Feature::Reload|Feature::Stop|Feature::History=>Support::Partial("request completion acknowledges dispatch; navigation outcomes remain ordered observations without supplier navigation IDs".into()),
                Feature::Focus=>Support::Partial("focus acquisition only; host owns release and key-loop traversal".into()),
                Feature::Keyboard=>Support::Unsupported("send native key codes with associated characters".into()),
                Feature::Ime=>Support::Unsupported("caret/focus observation is preserved, but explicit composition forwarding is not exposed by Scry".into()),
                Feature::Semantics|Feature::SemanticActions=>Support::Unsupported("system WebKit semantic transport is not exposed by Scry".into()),
                Feature::Capture=>Support::Unsupported("snapshot frames are exposed through FrameSource, not a correlated capture command".into()),
                Feature::Text=>Support::Unsupported("independent text/IME insertion is not exposed; native key characters are separate".into()),
                Feature::Scroll|Feature::CookieRead|Feature::Find|Feature::Zoom|Feature::Touch|Feature::Pen|Feature::Drag|Feature::ResourceInterception|Feature::Downloads|Feature::Permissions|Feature::Authentication|Feature::Popups|Feature::Devtools=>Support::Unsupported("not implemented in this concrete adapter".into()),
            })))?;
        Ok(Self {
            producer,
            protocol: Protocol::new(scope, 256)?,
            report,
            viewport,
            mode,
            snapshot_pending: false,
            revision: 0,
            commands: VecDeque::new(),
            outgoing: None,
            completion: None,
            report_dirty: false,
        })
    }
    fn flush_supplier(&mut self) -> Result<bool> {
        if let Some(event) = self.outgoing.take() {
            match self.protocol.emit(event.clone()) {
                Ok(()) => {}
                Err(Error::Backpressure) => {
                    self.outgoing = Some(event);
                    return Ok(false);
                }
                Err(e) => return Err(e),
            }
        }
        // Bounded turn. The producer's authoritative queue is the only queue drained.
        for _ in 0..32 {
            let Some(event) = self.producer.poll_web_surface_event() else {
                return Ok(true);
            };
            if let WebSurfaceEvent::ScriptCompleted { id, result } = event {
                self.protocol.settle(
                    RequestKey {
                        scope: self.scope(),
                        id: RequestId(id.get()),
                    },
                    result.map(Reply::Text).map_err(Error::Backend),
                )?;
                continue;
            }
            let event = match event {
                WebSurfaceEvent::Navigation(NavigationEvent::Starting { url }) => {
                    Event::NavigationStarted { address: url }
                }
                WebSurfaceEvent::Navigation(NavigationEvent::SourceChanged { url }) => {
                    Event::AddressChanged(url)
                }
                WebSurfaceEvent::Navigation(NavigationEvent::Completed { url, success }) => {
                    Event::NavigationFinished {
                        address: url,
                        success,
                    }
                }
                WebSurfaceEvent::Navigation(NavigationEvent::TitleChanged { title }) => {
                    Event::TitleChanged(title)
                }
                WebSurfaceEvent::Navigation(NavigationEvent::NewWindowRequested { url }) => {
                    Event::NavigationRequested { address: url }
                }
                WebSurfaceEvent::Navigation(
                    NavigationEvent::TextInputFocused { state }
                    | NavigationEvent::TextInputChanged { state },
                ) => Event::TextInput(Some(TextInputState {
                    element_kind: state.element_kind,
                    input_type: state.input_type,
                    input_mode: state.input_mode,
                    autocomplete: state.autocomplete,
                    multiline: state.is_multiline,
                    password: state.is_password,
                    selection_utf16: (state.selection_start, state.selection_end),
                    caret: LogicalRect {
                        x: state.caret_rect.x,
                        y: state.caret_rect.y,
                        width: state.caret_rect.width,
                        height: state.caret_rect.height,
                    },
                })),
                WebSurfaceEvent::Navigation(NavigationEvent::TextInputBlurred) => {
                    Event::TextInput(None)
                }
                WebSurfaceEvent::WebMessage(message) => Event::Message(message),
                other => Event::Diagnostic(format!("{other:?}")),
            };
            match self.protocol.emit(event.clone()) {
                Ok(()) => {}
                Err(Error::Backpressure) => {
                    self.outgoing = Some(event);
                    return Ok(false);
                }
                Err(e) => return Err(e),
            }
        }
        Ok(false)
    }
    fn execute(&mut self, key: RequestKey, command: Command) -> Option<Result<Reply>> {
        let result = match command {
            Command::Navigate { address } => self.producer.load_url(&address),
            Command::LoadHtml { address, html } => {
                self.producer.load_html_with_base_url(&html, &address)
            }
            Command::Reload => self.producer.reload(),
            Command::Stop => self.producer.stop(),
            Command::Back => self.producer.go_back().map(|_| ()),
            Command::Forward => self.producer.go_forward().map(|_| ()),
            Command::Script(script) => {
                return match self
                    .producer
                    .request_script_result(scrying::WebRequestId::new(key.id.0), &script)
                {
                    Ok(()) => None,
                    Err(e) => Some(Err(backend(e))),
                };
            }
            _ => return Some(Err(Error::Unsupported("operation is not exposed".into()))),
        };
        Some(result.map(|_| Reply::Dispatched).map_err(backend))
    }
}
impl Surface for WkSurface {
    fn scope(&self) -> Scope {
        self.protocol.scope()
    }
    fn capabilities(&self) -> &CapabilityReport {
        &self.report
    }
    fn set_viewport(&mut self, v: Viewport) -> Result<()> {
        self.protocol.ensure_live()?;
        let v = v.validate()?;
        self.producer
            .resize(dpi::PhysicalSize::new(v.width, v.height))
            .map_err(backend)?;
        self.viewport = v;
        Ok(())
    }
    fn set_visible(&mut self, v: bool) -> Result<()> {
        self.protocol.ensure_live()?;
        self.producer.set_visible(v).map_err(backend)
    }
    fn input(&mut self, input: Input) -> Result<()> {
        self.protocol.ensure_live()?;
        use scrying::{
            KeyEventKind, KeyModifierFlags, KeyboardInput, MouseEventKind as M, MouseInput,
            MouseVirtualKeys,
        };
        match input {
            Input::NativeKey(k) => self
                .producer
                .send_keyboard_input(KeyboardInput {
                    kind: if k.pressed {
                        KeyEventKind::Down
                    } else {
                        KeyEventKind::Up
                    },
                    virtual_key_code: k.key_code,
                    characters: k.characters,
                    characters_ignoring_modifiers: k.characters_ignoring_modifiers,
                    modifiers: KeyModifierFlags {
                        shift: k.modifiers.shift,
                        control: k.modifiers.control,
                        alt: k.modifiers.alt,
                        meta: k.modifiers.meta,
                        caps_lock: false,
                    },
                    is_repeat: k.repeat,
                })
                .map_err(backend),
            Input::Document(SessionInput::Focus(true)) => self
                .producer
                .move_focus(scrying::FocusReason::Programmatic)
                .map_err(backend),
            Input::Document(
                input @ (SessionInput::PointerMoved { .. } | SessionInput::PointerButton { .. }),
            ) => {
                let (x, y, modifiers, kind) = match input {
                    SessionInput::PointerMoved { x, y, modifiers } => (x, y, modifiers, M::Move),
                    SessionInput::PointerButton {
                        x,
                        y,
                        modifiers,
                        button,
                        state,
                    } => {
                        let down = state == SessionButtonState::Pressed;
                        let kind = match (button, down) {
                            (SessionPointerButton::Primary, true) => M::LeftButtonDown,
                            (SessionPointerButton::Primary, false) => M::LeftButtonUp,
                            (SessionPointerButton::Secondary, true) => M::RightButtonDown,
                            (SessionPointerButton::Secondary, false) => M::RightButtonUp,
                            (SessionPointerButton::Auxiliary, true) => M::MiddleButtonDown,
                            (SessionPointerButton::Auxiliary, false) => M::MiddleButtonUp,
                        };
                        (x, y, modifiers, kind)
                    }
                    _ => unreachable!(),
                };
                if !x.is_finite() || !y.is_finite() {
                    return Err(Error::Invalid("non-finite pointer".into()));
                }
                self.producer
                    .send_mouse_input(MouseInput {
                        kind,
                        point: (
                            (x * self.viewport.scale) as i32,
                            (y * self.viewport.scale) as i32,
                        ),
                        mouse_data: 0,
                        virtual_keys: MouseVirtualKeys {
                            shift: modifiers.shift,
                            control: modifiers.control,
                            ..Default::default()
                        },
                    })
                    .map_err(backend)
            }
            _ => Err(Error::Unsupported(
                "input is not exposed; consult this configuration's report".into(),
            )),
        }
    }
    fn request(&mut self, id: RequestId, command: Command) -> Result<RequestKey> {
        self.protocol.ensure_live()?;
        self.report.require(command.feature())?;
        // Keep observation headroom. If all slots were reserved for terminal
        // events, observations that must precede completion could deadlock.
        if self.protocol.pending() >= 128 {
            return Err(Error::Backpressure);
        }
        let key = self.protocol.accept(id)?;
        self.commands.push_back((key, command));
        Ok(key)
    }
    fn semantic_action(&mut self, _: SemanticAction) -> Result<()> {
        self.protocol.ensure_live()?;
        Err(Error::Unsupported(
            "system semantic transport is unavailable".into(),
        ))
    }
    fn advance(&mut self, _: Duration) -> Result<()> {
        self.protocol.ensure_live()?;
        if !self.flush_supplier()? {
            return Ok(());
        }
        if self.report_dirty {
            match self
                .protocol
                .emit(Event::CapabilitiesChanged(self.report.clone()))
            {
                Ok(()) => self.report_dirty = false,
                Err(Error::Backpressure) => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        if let Some((key, result)) = self.completion.take() {
            self.protocol.settle(key, result)?;
        }
        if let Some((key, command)) = self.commands.pop_front()
            && let Some(result) = self.execute(key, command)
        {
            self.completion = Some((key, result));
        }
        Ok(())
    }
    fn poll_event(&mut self) -> Option<Envelope> {
        self.protocol.poll()
    }
    fn retire(&mut self) -> Result<()> {
        self.commands.clear();
        self.completion = None;
        self.outgoing = None;
        // Retirement must settle accepted work even if platform shutdown refuses.
        let stopped = self.producer.stop().map_err(backend);
        self.protocol.retire()?;
        stopped
    }
}
impl FrameSource for WkSurface {
    fn try_acquire_frame(&mut self) -> Result<Option<Frame>> {
        self.protocol.ensure_live()?;
        let frame = match self.mode {
            WkFrames::Native { .. } => self.producer.try_acquire_frame().map_err(backend)?,
            WkFrames::Snapshot => match self.producer.poll_snapshot() {
                Some(result) => {
                    self.snapshot_pending = false;
                    Some(result.map_err(backend)?)
                }
                None => {
                    if !self.snapshot_pending {
                        self.producer.request_snapshot().map_err(backend)?;
                        self.snapshot_pending = true;
                    }
                    None
                }
            },
        };
        let Some(frame) = frame else {
            return Ok(None);
        };
        self.revision = self.revision.checked_add(1).ok_or(Error::Exhausted)?;
        let stamp = FrameStamp {
            scope: self.scope(),
            allocation_epoch: self.revision,
            content_revision: self.revision,
        };
        let frame = match frame {
            WebSurfaceFrame::CpuRgba { size, pixels, .. } if self.mode == WkFrames::Snapshot => {
                Frame::Cpu(CpuFrame {
                    stamp,
                    size: FrameSize::new(size.width, size.height),
                    stride: size.width.checked_mul(4).ok_or(Error::Exhausted)?,
                    rgba: pixels.into_raw(),
                    alpha: Alpha::Straight,
                    encoding: Encoding::Srgb,
                })
            }
            WebSurfaceFrame::Native(scrying::native_frame::NativeFrame::MetalTextureRef(f))
                if matches!(self.mode, WkFrames::Native { .. }) =>
            {
                let format = match f.format {
                    wgpu_cue::wgpu::TextureFormat::Rgba8Unorm => PixelFormat::Rgba8Unorm,
                    wgpu_cue::wgpu::TextureFormat::Rgba8UnormSrgb => PixelFormat::Rgba8UnormSrgb,
                    wgpu_cue::wgpu::TextureFormat::Bgra8Unorm => PixelFormat::Bgra8Unorm,
                    wgpu_cue::wgpu::TextureFormat::Bgra8UnormSrgb => PixelFormat::Bgra8UnormSrgb,
                    _ => return Err(Error::Unsupported("Scry pixel format".into())),
                };
                let sync = match f.producer_sync {
                    scrying::native_frame::SyncMechanism::None => FrameSync::None,
                    scrying::native_frame::SyncMechanism::ExplicitMetalEvent => {
                        FrameSync::MetalSharedEvent {
                            event: self.producer.metal_shared_event().ok_or_else(|| {
                                Error::Invalid(
                                    "Scry frame requires its missing shared event".into(),
                                )
                            })?,
                            value: f.signal_value,
                        }
                    }
                    _ => return Err(Error::Unsupported("Scry synchronization kind".into())),
                };
                let metadata = FrameMetadata {
                    size: FrameSize::new(f.size.width, f.size.height),
                    format,
                    generation: f.generation,
                };
                Frame::Native(NativeFrame {
                    stamp,
                    frame: OwnedNativeFrame::new(
                        metadata,
                        OwnedNativeImage::MetalTexture(MetalTexture::new(f.into_retained())),
                        sync,
                    ),
                    alpha: match self.mode {
                        WkFrames::Native { alpha, .. } => alpha,
                        _ => unreachable!(),
                    },
                    encoding: match self.mode {
                        WkFrames::Native { encoding, .. } => encoding,
                        _ => unreachable!(),
                    },
                })
            }
            _ => {
                return Err(Error::Unsupported(
                    "Scry emitted an unadvertised frame transport".into(),
                ));
            }
        };
        if self.report.evidence != Evidence::Observed {
            self.report.evidence = Evidence::Observed;
            self.report_dirty = true;
        }
        Ok(Some(frame))
    }
}
