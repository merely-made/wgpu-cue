// SPDX-License-Identifier: MPL-2.0
#![cfg(target_os = "macos")]
use document_session_api::{DocumentA11yAction, DocumentA11yRole};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use webcue::*;
use wgpu_cue::*;
use wgpu_cue_genet::*;

fn drain(s: &mut dyn Surface) -> Vec<Envelope> {
    std::iter::from_fn(|| s.poll_event()).collect()
}
fn projection(s: &mut dyn Surface) -> (PublicationKey, DocumentA11yProjection) {
    s.advance(Duration::ZERO).unwrap();
    drain(s)
        .into_iter()
        .find_map(|e| match e.event {
            Event::Semantics {
                publication,
                payload: SemanticPayload::Snapshot(p),
            } => Some((publication, p)),
            _ => None,
        })
        .expect("semantic publication")
}
#[test]
#[ignore = "requires a real native wgpu adapter; run explicitly"]
fn real_genet_receiver_lifecycle_ime_and_pixels() {
    let core = Arc::new(
        genet_render_host::RenderCore::boot(netrender::NetrenderOptions {
            tile_cache_size: Some(32),
            enable_vello: true,
            ..Default::default()
        })
        .unwrap(),
    );
    let host = GpuHost::new(core.device().clone(), core.queue().clone()).unwrap();
    let viewport = Viewport {
        width: 320,
        height: 200,
        scale: 1.,
    };
    let scope = Scope {
        owner: 0x1_0000_0000_0000_0000,
        generation: 1,
    };
    let mut s=GenetSurface::new(scope,viewport,"cue:test".into(),"<html><body style='background:#204080'><label for='name'>Name</label><input id='name' value='hello'><p>Cue retained proof</p></body></html>".into(),HeldResources::default(),core.clone()).unwrap();
    assert_eq!(s.capabilities().presentation, PresentationMode::HostDevice);
    let key = s
        .request(RequestId(1), Command::SetSemantics(true))
        .unwrap();
    s.advance(Duration::ZERO).unwrap();
    let events = drain(&mut s);
    let semantic_position = events
        .iter()
        .position(|e| matches!(e.event, Event::Semantics { .. }))
        .unwrap();
    let completed_position = events
        .iter()
        .position(|e| matches!(e.event,Event::Completed {request,result:Ok(_)} if request==key))
        .unwrap();
    assert!(semantic_position < completed_position);
    let (pubkey, p) = events
        .into_iter()
        .find_map(|e| match e.event {
            Event::Semantics {
                publication,
                payload: SemanticPayload::Snapshot(p),
            } => Some((publication, p)),
            _ => None,
        })
        .unwrap();
    let field = p
        .nodes()
        .iter()
        .find(|n| n.role == DocumentA11yRole::TextField)
        .unwrap();
    let focus = SemanticAction::Document {
        publication: pubkey,
        action: DocumentA11yActionRequest {
            revision: p.revision(),
            target: field.id,
            action: DocumentA11yAction::Focus,
            data: None,
        },
    };
    s.semantic_action(focus.clone()).unwrap();
    assert_eq!(s.semantic_action(focus).unwrap_err(), Error::Stale);
    s.input(Input::Document(SessionInput::Ime(SessionIme::Enabled)))
        .unwrap();
    s.input(Input::Document(SessionInput::Ime(SessionIme::Preedit {
        text: "世界".into(),
        selection: Some((0, 6)),
    })))
    .unwrap();
    s.input(Input::Document(SessionInput::Ime(SessionIme::Commit(
        "世界".into(),
    ))))
    .unwrap();
    let (_, p) = projection(&mut s);
    assert!(
        p.nodes()
            .iter()
            .any(|n| n.value.as_deref().is_some_and(|v| v.contains("世界"))),
        "IME commit did not reach retained value: {:?}",
        p.nodes()
    );
    let mut presenter = Presenter::default();
    let mut importer = GraftMetalImporter;
    let deadline = Instant::now() + Duration::from_secs(30);
    let texture = loop {
        s.advance(Duration::ZERO).unwrap();
        drain(&mut s);
        if let Some(f) = presenter
            .present(&host, Source::HostDevice(&mut s), viewport, &mut importer)
            .unwrap()
        {
            break f.texture.clone();
        }
        assert!(
            Instant::now() < deadline,
            "Genet never published a qualified frame"
        );
        std::thread::sleep(Duration::from_millis(2));
    };
    let mut pixels = core.start_rgba8_readback(&texture, 320, 200).unwrap();
    let frame = loop {
        core.device().poll(wgpu::PollType::Poll).unwrap();
        if let Some(r) = pixels.poll() {
            break r.unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    assert!(
        frame
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| px[0].abs_diff(32) < 3
                && px[1].abs_diff(64) < 3
                && px[2].abs_diff(128) < 3
                && px[3] == 255)
            .count()
            > 1000,
        "expected blue background missing"
    );
    eprintln!(
        "Genet: qualified 320x200 GPU frame, {} bytes; retained IME commit and stale action refusal passed",
        frame.rgba.len()
    );
    s.request(RequestId(2),Command::LoadHtml {address:"cue:alpha".into(),html:"<html><body style='margin:0;background:transparent'><div style='width:100px;height:100px;background:rgba(255,0,0,0.5)'></div></body></html>".into()}).unwrap();
    let alpha_texture = loop {
        s.advance(Duration::ZERO).unwrap();
        drain(&mut s);
        if let Some(f) = presenter
            .present(&host, Source::HostDevice(&mut s), viewport, &mut importer)
            .unwrap()
            && f.texture != &texture
        {
            break f.texture.clone();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    let mut readback = core.start_rgba8_readback(&alpha_texture, 320, 200).unwrap();
    let alpha_pixels = loop {
        core.device().poll(wgpu::PollType::Poll).unwrap();
        if let Some(r) = readback.poll() {
            break r.unwrap();
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    };
    let sample = &alpha_pixels.rgba[(50 * 320 + 50) * 4..(50 * 320 + 50) * 4 + 4];
    eprintln!("Genet alpha sample: {sample:?}");
    assert!(
        sample[0] == 255 && sample[1] == 0 && sample[2] == 0 && sample[3].abs_diff(128) < 3,
        "expected straight-alpha red"
    );
    s.request(RequestId(3), Command::Reload).unwrap();
    s.retire().unwrap();
    assert!(drain(&mut s).iter().any(|e| matches!(
        e.event,
        Event::Completed {
            result: Err(Error::Retired),
            ..
        }
    )));
    assert_eq!(
        s.input(Input::Document(SessionInput::Focus(true)))
            .unwrap_err(),
        Error::Retired
    );
    let mut flood = GenetSurface::new(
        Scope {
            owner: 9182,
            generation: 1,
        },
        viewport,
        "cue:flood".into(),
        "<p>initial</p>".into(),
        HeldResources::default(),
        core.clone(),
    )
    .unwrap();
    for id in 1..=128 {
        flood
            .request(
                RequestId(id),
                Command::LoadHtml {
                    address: format!("cue:{id}"),
                    html: "<p>replacement</p>".into(),
                },
            )
            .unwrap();
    }
    assert_eq!(
        flood.request(RequestId(129), Command::Reload),
        Err(Error::Backpressure)
    );
    let mut completed = 0;
    for _ in 0..140 {
        flood.advance(Duration::ZERO).unwrap();
        completed += drain(&mut flood)
            .into_iter()
            .filter(|e| matches!(e.event, Event::Completed { result: Ok(_), .. }))
            .count();
        if completed == 128 {
            break;
        }
    }
    assert_eq!(
        completed, 128,
        "accepted navigation work deadlocked behind observation reservations"
    );
    flood.retire().unwrap();
}
