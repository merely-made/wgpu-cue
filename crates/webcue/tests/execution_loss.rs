use webcue::*;

#[test]
fn lost_executor_preserves_observations_and_settles_unknown_outcomes_once() {
    let mut protocol = Protocol::new(
        Scope {
            owner: 1,
            generation: 2,
        },
        3,
    )
    .unwrap();
    let first = protocol.accept(RequestId(1)).unwrap();
    let second = protocol.accept(RequestId(2)).unwrap();
    protocol
        .emit(Event::TitleChanged("before disconnect".into()))
        .unwrap();
    protocol.execution_lost("pipe closed".into()).unwrap();
    protocol.execution_lost("later cause".into()).unwrap();
    protocol.retire().unwrap();
    assert!(matches!(
        protocol.poll().unwrap().event,
        Event::TitleChanged(_)
    ));
    for expected in [first, second] {
        let Event::Completed { request, result } = protocol.poll().unwrap().event else {
            panic!("missing terminal event")
        };
        assert_eq!(request, expected);
        assert_eq!(
            result.unwrap_err(),
            Error::ExecutionLost("pipe closed".into())
        );
    }
    assert!(protocol.poll().is_none());
    assert_eq!(protocol.accept(RequestId(3)), Err(Error::Retired));
    assert_eq!(
        protocol.settle(first, Ok(Reply::Done)),
        Err(Error::NotPending)
    );
}
