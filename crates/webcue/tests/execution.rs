use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use webcue::{execution::*, *};

#[derive(Default)]
struct State {
    sent: Vec<ExecutionRequest>,
    updates: VecDeque<ExecutionUpdate>,
    busy: bool,
    disconnected: bool,
    stop_requests: usize,
    stop_error: bool,
    supervision_error: bool,
    stopped: Option<StopOutcome>,
}
struct Transport(Rc<RefCell<State>>);
impl ExecutionTransport for Transport {
    fn termination(&self) -> Termination {
        Termination::Cooperative
    }
    fn try_send(&mut self, request: &ExecutionRequest) -> Result<SendStatus> {
        let mut state = self.0.borrow_mut();
        if state.disconnected {
            return Err(Error::Backend("closed".into()));
        }
        if state.busy {
            return Ok(SendStatus::Busy);
        }
        state.sent.push(request.clone());
        Ok(SendStatus::Sent)
    }
    fn try_recv(&mut self) -> Result<Option<ExecutionUpdate>> {
        let mut state = self.0.borrow_mut();
        if let Some(update) = state.updates.pop_front() {
            return Ok(Some(update));
        }
        if state.disconnected {
            return Err(Error::Backend("closed".into()));
        }
        Ok(None)
    }
    fn request_stop(&mut self) -> Result<()> {
        let mut state = self.0.borrow_mut();
        state.stop_requests += 1;
        if state.stop_error {
            return Err(Error::Backend("stop refused".into()));
        }
        Ok(())
    }
    fn poll_stopped(&mut self) -> Result<Option<StopOutcome>> {
        if self.0.borrow().supervision_error {
            return Err(Error::Backend("cannot observe executor".into()));
        }
        Ok(self.0.borrow().stopped)
    }
}
fn id() -> ExecutionId {
    ExecutionId {
        scope: Scope {
            owner: 4,
            generation: 7,
        },
        incarnation: 11,
    }
}
fn setup(capacity: usize) -> (ExecutionSession<Transport>, Rc<RefCell<State>>) {
    let state = Rc::new(RefCell::new(State::default()));
    (
        ExecutionSession::new(
            id(),
            Transport(state.clone()),
            capacity,
            Termination::Cooperative,
        )
        .unwrap(),
        state,
    )
}
fn update(state: &Rc<RefCell<State>>, sequence: u64, event: Event) {
    state.borrow_mut().updates.push_back(ExecutionUpdate {
        execution: id(),
        sequence,
        event,
    });
}
fn completed(request: RequestKey) -> Event {
    Event::Completed {
        request,
        result: Ok(Reply::Done),
    }
}

#[test]
fn mailbox_requires_room_for_editor_publication_and_its_clearing_slot() {
    for capacity in 0..3 {
        let result = ExecutionSession::new(
            id(),
            Transport(Rc::new(RefCell::new(State::default()))),
            capacity,
            Termination::Cooperative,
        );
        assert!(matches!(result, Err(Error::Invalid(_))));
    }
}

#[test]
fn executor_cannot_complete_work_that_transport_has_not_admitted() {
    let (mut session, state) = setup(4);
    state.borrow_mut().busy = true;
    let request = session.request(RequestId(1), Command::Reload).unwrap();
    update(&state, 1, completed(request));
    session.advance().unwrap();
    assert!(state.borrow().sent.is_empty());
    assert!(matches!(
        session.poll_event().unwrap().event,
        Event::Completed { request: key, result: Err(Error::ExecutionLost(_)) } if key == request
    ));
    assert!(
        session
            .loss()
            .unwrap()
            .contains("completion precedes transmission")
    );
    state.borrow_mut().busy = false;
    session.advance().unwrap();
    assert!(state.borrow().sent.is_empty());
    assert!(session.poll_event().is_none());
}

#[test]
fn local_acceptance_survives_transport_backpressure_without_duplicate_send() {
    let (mut session, state) = setup(4);
    state.borrow_mut().busy = true;
    let first = session.request(RequestId(1), Command::Reload).unwrap();
    session.request(RequestId(2), Command::Reload).unwrap();
    assert_eq!(
        session.request(RequestId(3), Command::Reload),
        Err(Error::Backpressure)
    );
    session.advance().unwrap();
    assert!(state.borrow().sent.is_empty());
    state.borrow_mut().busy = false;
    session.advance().unwrap();
    session.advance().unwrap();
    assert_eq!(state.borrow().sent.len(), 2);
    update(&state, 1, completed(first));
    session.advance().unwrap();
    session.poll_event().unwrap();
    assert!(session.request(RequestId(3), Command::Reload).is_ok());
}

#[test]
fn loss_settles_requests_but_does_not_claim_executor_stopped() {
    let (mut session, state) = setup(4);
    let request = session.request(RequestId(1), Command::Reload).unwrap();
    session.advance().unwrap();
    state.borrow_mut().disconnected = true;
    session.advance().unwrap();
    assert_eq!(session.stopped(), None);
    let Event::Completed {
        request: actual,
        result,
    } = session.poll_event().unwrap().event
    else {
        panic!()
    };
    assert_eq!(actual, request);
    assert!(matches!(result, Err(Error::ExecutionLost(_))));
    assert_eq!(
        session.request(RequestId(2), Command::Reload),
        Err(Error::Retired)
    );
    session.advance().unwrap();
    assert_eq!(state.borrow().sent.len(), 1);
    assert_eq!(state.borrow().stop_requests, 1);
    state.borrow_mut().stopped = Some(StopOutcome::Terminated);
    session.advance().unwrap();
    assert_eq!(session.stopped(), Some(StopOutcome::Terminated));
    assert!(session.poll_event().is_none());
}

#[test]
fn stale_execution_cannot_complete_current_request() {
    let (mut session, state) = setup(4);
    let request = session.request(RequestId(1), Command::Reload).unwrap();
    update(&state, 1, completed(request));
    state
        .borrow_mut()
        .updates
        .front_mut()
        .unwrap()
        .execution
        .incarnation -= 1;
    session.advance().unwrap();
    assert!(matches!(
        session.poll_event().unwrap().event,
        Event::Completed {
            result: Err(Error::ExecutionLost(_)),
            ..
        }
    ));
    assert!(session.loss().unwrap().contains("invalid update"));
}

#[test]
fn foreign_inner_request_identity_is_refused() {
    let (mut session, state) = setup(4);
    let mut request = session.request(RequestId(1), Command::Reload).unwrap();
    request.scope.generation -= 1;
    update(&state, 1, completed(request));
    session.advance().unwrap();
    assert!(matches!(
        session.poll_event().unwrap().event,
        Event::Completed {
            result: Err(Error::ExecutionLost(_)),
            ..
        }
    ));
}

#[test]
fn duplicate_completion_does_not_overwrite_success_or_strand_other_work() {
    let (mut session, state) = setup(6);
    let first = session.request(RequestId(1), Command::Reload).unwrap();
    let second = session.request(RequestId(2), Command::Reload).unwrap();
    update(&state, 1, completed(first));
    update(&state, 2, completed(first));
    session.advance().unwrap();
    assert!(
        matches!(session.poll_event().unwrap().event, Event::Completed { request, result: Ok(Reply::Done) } if request == first)
    );
    assert!(
        matches!(session.poll_event().unwrap().event, Event::Completed { request, result: Err(Error::ExecutionLost(_)) } if request == second)
    );
    assert!(session.poll_event().is_none());
}

#[test]
fn observation_pressure_keeps_stream_order_and_final_completion() {
    let (mut session, state) = setup(4);
    let request = session.request(RequestId(1), Command::Reload).unwrap();
    for sequence in 1..=5 {
        update(&state, sequence, Event::Message(sequence.to_string()));
    }
    update(&state, 6, completed(request));
    session.advance().unwrap();
    let mut events = Vec::new();
    for _ in 0..8 {
        if let Some(event) = session.poll_event() {
            events.push(event);
        }
        session.advance().unwrap();
    }
    assert_eq!(events.len(), 6);
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5, 6]
    );
    for (index, event) in events[..5].iter().enumerate() {
        assert!(matches!(&event.event, Event::Message(value) if value == &(index + 1).to_string()));
    }
    assert!(matches!(
        events[5].event,
        Event::Completed {
            result: Ok(Reply::Done),
            ..
        }
    ));
}

#[test]
fn retirement_is_distinct_from_stop_and_failed_stop_can_be_retried() {
    let (mut session, state) = setup(4);
    session.request(RequestId(1), Command::Reload).unwrap();
    state.borrow_mut().stop_error = true;
    assert!(session.retire().is_err());
    assert!(matches!(
        session.poll_event().unwrap().event,
        Event::Completed {
            result: Err(Error::Retired),
            ..
        }
    ));
    assert_eq!(session.stopped(), None);
    state.borrow_mut().stop_error = false;
    session.retire().unwrap();
    session.retire().unwrap();
    assert_eq!(state.borrow().stop_requests, 2);
    assert!(state.borrow().sent.is_empty());
    assert!(session.poll_event().is_none());
}

#[test]
fn termination_requirement_is_checked_before_accepting_work() {
    let state = Rc::new(RefCell::new(State::default()));
    let result = ExecutionSession::new(id(), Transport(state.clone()), 4, Termination::Forced);
    assert!(matches!(result, Err(Error::Unsupported(_))));
    assert!(state.borrow().sent.is_empty());
}

#[test]
fn buffered_reply_precedes_observed_exit() {
    let (mut session, state) = setup(4);
    let request = session.request(RequestId(1), Command::Reload).unwrap();
    update(&state, 1, completed(request));
    state.borrow_mut().stopped = Some(StopOutcome::Exited);
    session.advance().unwrap();
    assert_eq!(session.stopped(), Some(StopOutcome::Exited));
    assert!(matches!(
        session.poll_event().unwrap().event,
        Event::Completed {
            result: Ok(Reply::Done),
            ..
        }
    ));
    assert!(session.poll_event().is_none());
}

#[test]
fn sequence_gap_closes_admission_without_inventing_missing_observations() {
    let (mut session, state) = setup(4);
    session.request(RequestId(1), Command::Reload).unwrap();
    update(&state, 2, Event::TitleChanged("gap".into()));
    session.advance().unwrap();
    assert!(matches!(
        session.poll_event().unwrap().event,
        Event::Completed {
            result: Err(Error::ExecutionLost(_)),
            ..
        }
    ));
    assert!(session.poll_event().is_none());
}

#[test]
fn failed_supervision_settles_work_without_claiming_stop() {
    let (mut session, state) = setup(4);
    session.request(RequestId(1), Command::Reload).unwrap();
    state.borrow_mut().supervision_error = true;
    assert!(session.advance().is_err());
    assert_eq!(session.stopped(), None);
    assert!(matches!(
        session.poll_event().unwrap().event,
        Event::Completed {
            result: Err(Error::ExecutionLost(_)),
            ..
        }
    ));
    assert_eq!(session.pending(), 0);
}

#[test]
fn loss_clears_editor_geometry_even_when_mailbox_is_full() {
    let (mut session, state) = setup(3);
    session.request(RequestId(1), Command::Reload).unwrap();
    update(
        &state,
        1,
        Event::TextInput(Some(TextInputState {
            element_kind: "input".into(),
            input_type: "text".into(),
            input_mode: String::new(),
            autocomplete: String::new(),
            multiline: false,
            password: false,
            selection_utf16: (1, 1),
            caret: LogicalRect {
                x: 4.0,
                y: 12.0,
                width: 1.0,
                height: 18.0,
            },
        })),
    );
    session.advance().unwrap();
    assert!(session.text_input_state().is_some());
    session.lose("lost".into()).unwrap();
    assert!(session.text_input_state().is_none());
    assert!(matches!(
        session.poll_event().unwrap().event,
        Event::TextInput(Some(_))
    ));
    assert!(matches!(
        session.poll_event().unwrap().event,
        Event::TextInput(None)
    ));
    assert!(matches!(
        session.poll_event().unwrap().event,
        Event::Completed {
            result: Err(Error::ExecutionLost(_)),
            ..
        }
    ));
    assert!(session.poll_event().is_none());
}
