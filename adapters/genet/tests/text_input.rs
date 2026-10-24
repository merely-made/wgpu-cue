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
