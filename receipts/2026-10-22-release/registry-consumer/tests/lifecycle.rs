use webcue::*;
fn scope() -> Scope {
    Scope {
        owner: 41,
        generation: 7,
    }
}
#[test]
fn completions_cannot_overtake_observations_and_keep_caller_identity() {
    let mut p = Protocol::new(scope(), 8).unwrap();
    p.emit(Event::AddressChanged("https://example.test/a".into()))
        .unwrap();
    let key = p.accept(RequestId(19)).unwrap();
    p.emit(Event::Message("before result".into())).unwrap();
    p.settle(key, Ok(Reply::Text("result".into()))).unwrap();
    let events = std::iter::from_fn(|| p.poll()).collect::<Vec<_>>();
    assert!(matches!(&events[0].event, Event::AddressChanged(_)));
    assert!(matches!(&events[1].event, Event::Message(_)));
    assert!(
        matches!(&events[2].event,Event::Completed {request,result:Ok(Reply::Text(_))} if *request==key)
    );
    assert_eq!(
        events.iter().map(|e| e.sequence).collect::<Vec<_>>(),
        [1, 2, 3]
    );
}
#[test]
fn observations_cannot_use_reserved_completion_slots() {
    let mut p = Protocol::new(scope(), 3).unwrap();
    let a = p.accept(RequestId(1)).unwrap();
    let b = p.accept(RequestId(2)).unwrap();
    p.emit(Event::Message("one".into())).unwrap();
    assert_eq!(
        p.emit(Event::Message("overflow".into())),
        Err(Error::Backpressure)
    );
    p.retire().unwrap();
    p.retire().unwrap();
    assert!(matches!(p.poll().unwrap().event, Event::Message(_)));
    for key in [a, b] {
        assert!(
            matches!(p.poll().unwrap().event,Event::Completed {request,result:Err(Error::Retired)} if request==key)
        );
    }
    assert!(p.poll().is_none());
    assert_eq!(p.accept(RequestId(3)), Err(Error::Retired));
    assert_eq!(p.settle(a, Ok(Reply::Done)), Err(Error::NotPending));
}
#[test]
fn rejected_acceptance_has_no_completion_and_can_be_retried_after_drain() {
    let mut p = Protocol::new(scope(), 1).unwrap();
    p.emit(Event::Message("full".into())).unwrap();
    assert_eq!(p.accept(RequestId(50)), Err(Error::Backpressure));
    p.poll();
    let key = p.accept(RequestId(50)).unwrap();
    p.settle(key, Ok(Reply::Done)).unwrap();
    assert_eq!(p.accept(RequestId(50)), Err(Error::RequestOrder));
    assert_eq!(p.settle(key, Ok(Reply::Done)), Err(Error::NotPending));
    assert!(p.poll().is_some());
    assert!(p.poll().is_none());
}
#[test]
fn old_generation_cannot_settle_an_identically_numbered_live_request() {
    let mut p = Protocol::new(scope(), 4).unwrap();
    let key = p.accept(RequestId(1)).unwrap();
    let old = RequestKey {
        scope: Scope {
            generation: 6,
            ..scope()
        },
        ..key
    };
    assert_eq!(p.settle(old, Ok(Reply::Done)), Err(Error::Stale));
    assert_eq!(p.pending(), 1);
    p.settle(key, Ok(Reply::Done)).unwrap();
}
#[test]
fn semantic_actions_expire_without_waiting_for_another_frame() {
    let mut p = PublicationTracker::new(scope());
    let first = p.publish(10).unwrap();
    p.validate(first).unwrap();
    let second = p.publish(11).unwrap();
    assert_eq!(p.validate(first), Err(Error::Stale));
    p.validate(second).unwrap();
    p.retire();
    assert_eq!(p.validate(second), Err(Error::Stale));
    assert_eq!(p.publish(11), Err(Error::Stale));
    let replacement = p.publish(12).unwrap();
    assert_ne!(replacement, first);
}
fn identity() -> BackendIdentity {
    BackendIdentity {
        engine: "fixture".into(),
        version: None,
        platform: "test".into(),
        build_features: vec![],
    }
}
#[test]
fn capability_report_refuses_missing_duplicate_and_unexplained_answers() {
    let entries = Feature::ALL.map(|f| (f, Support::Unsupported("not forwarded".into())));
    assert!(
        CapabilityReport::new(
            identity(),
            PresentationMode::HostDevice,
            Evidence::Constructed,
            SemanticProtocol::Unavailable,
            entries[..26].iter().cloned()
        )
        .is_err()
    );
    let mut duplicates = entries.to_vec();
    duplicates.push(entries[0].clone());
    assert!(
        CapabilityReport::new(
            identity(),
            PresentationMode::HostDevice,
            Evidence::Constructed,
            SemanticProtocol::Unavailable,
            duplicates
        )
        .is_err()
    );
    let mut empty = entries.to_vec();
    empty[0].1 = Support::Partial(" ".into());
    assert!(
        CapabilityReport::new(
            identity(),
            PresentationMode::HostDevice,
            Evidence::Constructed,
            SemanticProtocol::Unavailable,
            empty
        )
        .is_err()
    );
    let report = CapabilityReport::new(
        identity(),
        PresentationMode::HostDevice,
        Evidence::Constructed,
        SemanticProtocol::Unavailable,
        entries,
    )
    .unwrap();
    assert!(matches!(
        report.require(Feature::ScriptResult),
        Err(Error::Unsupported(_))
    ));
}
#[test]
fn malformed_cpu_frames_and_nonfinite_viewports_are_refused() {
    let mut f = CpuFrame {
        stamp: FrameStamp {
            scope: scope(),
            allocation_epoch: 1,
            content_revision: 1,
        },
        size: FrameSize::new(2, 2),
        stride: 8,
        rgba: vec![0; 16],
        alpha: Alpha::Straight,
        encoding: Encoding::Srgb,
    };
    f.validate().unwrap();
    f.rgba.pop();
    assert!(f.validate().is_err());
    f.size.width = u32::MAX;
    assert!(f.validate().is_err());
    assert!(
        Viewport {
            width: 1,
            height: 1,
            scale: f32::NAN
        }
        .validate()
        .is_err()
    );
}

#[test]
fn semantics_must_name_their_actual_protocol() {
    let entries = Feature::ALL.map(|f| {
        (
            f,
            if matches!(f, Feature::Semantics | Feature::SemanticActions) {
                Support::Supported
            } else {
                Support::Unsupported("fixture".into())
            },
        )
    });
    assert!(
        CapabilityReport::new(
            identity(),
            PresentationMode::HostDevice,
            Evidence::Constructed,
            SemanticProtocol::Unavailable,
            entries.clone()
        )
        .is_err()
    );
    let report = CapabilityReport::new(
        identity(),
        PresentationMode::HostDevice,
        Evidence::Constructed,
        SemanticProtocol::AccesskitTree,
        entries,
    )
    .unwrap();
    assert_eq!(report.semantics, SemanticProtocol::AccesskitTree);
}
