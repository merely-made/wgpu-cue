// SPDX-License-Identifier: MPL-2.0
//! Run explicitly against a native Metal or Vulkan adapter.
use std::{sync::Arc, time::Duration};
use webcue::*;
use wgpu_cue_genet::*;

fn drain(surface: &mut dyn Surface) -> Vec<Envelope> {
    std::iter::from_fn(|| surface.poll_event()).collect()
}
#[test]
#[ignore = "requires a native GPU adapter; run explicitly"]
fn shared_genet_text_input_geometry_without_semantic_publication() {
    let core = Arc::new(
        genet_render_host::RenderCore::boot(netrender::NetrenderOptions {
            enable_vello: true,
            tile_cache_size: Some(32),
            ..Default::default()
        })
        .unwrap(),
    );
    let adapter = core.renderer().wgpu_device.core.adapter.get_info();
    assert_ne!(
        adapter.device_type,
        wgpu_cue::wgpu::DeviceType::Cpu,
        "native geometry qualification requires a hardware adapter"
    );
    #[cfg(target_os = "linux")]
    assert_eq!(adapter.backend, wgpu_cue::wgpu::Backend::Vulkan);
    #[cfg(target_os = "macos")]
    assert_eq!(adapter.backend, wgpu_cue::wgpu::Backend::Metal);
    eprintln!("text-input geometry adapter: {adapter:?}");
    let viewport = Viewport {
        width: 640,
        height: 400,
        scale: 2.0,
    };
    let scope = Scope {
        owner: 74,
        generation: 1,
    };
    let mut surface = GenetSurface::new(scope, viewport, "cue:geometry".into(),
        "<!doctype html><body style='margin:0'><input value='a😀b' inputmode='email' autocomplete='username' style='margin:20px;width:200px'>".into(),
        HeldResources::default(), core).unwrap();
    surface.advance(Duration::ZERO).unwrap();
    assert!(
        !drain(&mut surface)
            .iter()
            .any(|e| matches!(e.event, Event::TextInput(Some(_))))
    );
    surface
        .input(Input::Document(SessionInput::PointerButton {
            x: 30.0,
            y: 25.0,
            button: SessionPointerButton::Primary,
            state: SessionButtonState::Pressed,
            modifiers: Default::default(),
        }))
        .unwrap();
    surface
        .input(Input::Document(SessionInput::PointerButton {
            x: 30.0,
            y: 25.0,
            button: SessionPointerButton::Primary,
            state: SessionButtonState::Released,
            modifiers: Default::default(),
        }))
        .unwrap();
    surface.advance(Duration::ZERO).unwrap();
    let first = drain(&mut surface);
    assert!(
        !first
            .iter()
            .any(|e| matches!(e.event, Event::Semantics { .. }))
    );
    let state = first
        .into_iter()
        .find_map(|e| match e.event {
            Event::TextInput(Some(state)) => Some(state),
            _ => None,
        })
        .expect("focused control publishes geometry with semantics disabled");
    assert_eq!(state.input_mode, "email");
    assert_eq!(state.autocomplete, "username");
    assert!(
        state.caret.x >= 20.0 && state.caret.x < 220.0,
        "producer coordinates remain logical at device scale 2"
    );
    surface
        .input(Input::Document(SessionInput::Ime(SessionIme::Commit(
            "😀".into(),
        ))))
        .unwrap();
    let cleared = drain(&mut surface);
    assert!(
        cleared
            .iter()
            .any(|e| matches!(e.event, Event::TextInput(None)))
    );
    surface.advance(Duration::ZERO).unwrap();
    let after = drain(&mut surface)
        .into_iter()
        .find_map(|e| match e.event {
            Event::TextInput(Some(state)) => Some(state),
            _ => None,
        })
        .unwrap();
    assert_eq!(after.selection_utf16.0, state.selection_utf16.0 + 2);
    surface.set_visible(false).unwrap();
    assert!(
        drain(&mut surface)
            .iter()
            .any(|e| matches!(e.event, Event::TextInput(None)))
    );
    surface.advance(Duration::ZERO).unwrap();
    assert!(
        !drain(&mut surface)
            .iter()
            .any(|e| matches!(e.event, Event::TextInput(Some(_))))
    );
    surface.set_visible(true).unwrap();
    surface.advance(Duration::ZERO).unwrap();
    drain(&mut surface);
    surface.retire().unwrap();
    let retired = drain(&mut surface);
    assert!(
        retired
            .iter()
            .any(|e| matches!(e.event, Event::TextInput(None)))
    );
    assert!(retired.iter().all(|e| e.scope == scope));
    assert!(retired.windows(2).all(|e| e[0].sequence < e[1].sequence));
}

#[test]
#[ignore = "requires a native GPU adapter; run explicitly"]
fn shared_genet_password_caret_matches_masked_glyphs() {
    use wgpu_cue::{GpuHost, Rendered, TextureSource};
    let core = Arc::new(
        genet_render_host::RenderCore::boot(netrender::NetrenderOptions {
            enable_vello: true,
            tile_cache_size: Some(32),
            ..Default::default()
        })
        .unwrap(),
    );
    let adapter = core.renderer().wgpu_device.core.adapter.get_info();
    assert_ne!(adapter.device_type, wgpu_cue::wgpu::DeviceType::Cpu);
    #[cfg(target_os = "linux")]
    assert_eq!(adapter.backend, wgpu_cue::wgpu::Backend::Vulkan);
    #[cfg(target_os = "macos")]
    assert_eq!(adapter.backend, wgpu_cue::wgpu::Backend::Metal);
    eprintln!("password geometry adapter: {adapter:?}");
    let viewport = Viewport {
        width: 640,
        height: 400,
        scale: 2.0,
    };
    let mut surface = GenetSurface::new(Scope { owner:75,generation:1 }, viewport,
        "cue:password".into(), "<!doctype html><style>body{margin:0;background:white}input{display:block;margin:20px;width:220px;height:40px;font:20px monospace}</style><input type=password value=abcd><input value='••••'>".into(),
        HeldResources::default(), core.clone()).unwrap();
    fn observe(surface: &mut GenetSurface) -> TextInputState {
        surface.advance(Duration::ZERO).unwrap();
        let events = drain(surface);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e.event, Event::Semantics { .. }))
        );
        events
            .into_iter()
            .find_map(|e| match e.event {
                Event::TextInput(Some(state)) => Some(state),
                _ => None,
            })
            .expect("current password/control geometry without semantic publication")
    }
    fn end(surface: &mut GenetSurface) {
        surface
            .input(Input::Document(SessionInput::Key {
                key: SessionKey::End,
                state: SessionButtonState::Pressed,
                modifiers: Default::default(),
                repeat: false,
            }))
            .unwrap();
    }
    surface.advance(Duration::ZERO).unwrap();
    drain(&mut surface);
    surface
        .input(Input::Document(SessionInput::FocusMove(
            SessionFocusDirection::Forward,
        )))
        .unwrap();
    end(&mut surface);
    let password = observe(&mut surface);
    assert!(password.password);
    assert_eq!(password.selection_utf16, (4, 4));
    assert!(!format!("{password:?}").contains("abcd"));
    surface
        .input(Input::Document(SessionInput::FocusMove(
            SessionFocusDirection::Forward,
        )))
        .unwrap();
    end(&mut surface);
    let reference = observe(&mut surface);
    assert!(!reference.password);
    assert!((password.caret.x - reference.caret.x).abs() < 0.01);
    surface
        .input(Input::Document(SessionInput::FocusMove(
            SessionFocusDirection::Backward,
        )))
        .unwrap();
    end(&mut surface);
    let _ = observe(&mut surface);
    surface
        .input(Input::Document(SessionInput::Ime(SessionIme::Commit(
            "\u{0301}".into(),
        ))))
        .unwrap();
    let events = drain(&mut surface);
    assert!(
        events
            .iter()
            .any(|e| matches!(e.event, Event::TextInput(None)))
    );
    let edited = observe(&mut surface);
    assert_eq!(edited.selection_utf16, (5, 5));
    assert!(
        (edited.caret.x - password.caret.x).abs() < 0.01,
        "same-mask composition must preserve its correct glyph-edge geometry"
    );
    let host = GpuHost::new(core.device().clone(), core.queue().clone()).unwrap();
    let cx = host.context(viewport).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let texture = loop {
        if let Rendered::Changed(frame) = surface.render(&cx, true).unwrap() {
            break frame.texture;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "password frame never completed"
        );
        core.device().poll(wgpu_cue::wgpu::PollType::Poll).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    };
    let pixels = core.read_rgba8_texture(&texture, 640, 400).unwrap();
    assert_eq!(pixels.rgba.len(), 640 * 400 * 4);
    assert!(
        pixels
            .rgba
            .chunks_exact(4)
            .any(|p| p[3] > 0 && p[0] < 100 && p[1] < 100 && p[2] < 100),
        "native password/reference frame must contain rendered foreground pixels"
    );
}
