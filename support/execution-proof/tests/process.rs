use cue_execution_proof::{Local, process::Process};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use webcue::{execution::*, *};

fn identity(generation: u64) -> ExecutionId {
    ExecutionId {
        scope: Scope {
            owner: 91,
            generation,
        },
        incarnation: generation + 100,
    }
}
fn command_session<T: ExecutionTransport>(
    transport: T,
    generation: u64,
    termination: Termination,
) -> ExecutionSession<T> {
    ExecutionSession::new(identity(generation), transport, 8, termination).unwrap()
}
fn drive<T: ExecutionTransport>(
    session: &mut ExecutionSession<T>,
    request: RequestKey,
) -> Vec<Envelope> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut events = Vec::new();
    loop {
        session.advance().unwrap();
        while let Some(event) = session.poll_event() {
            let done =
                matches!(&event.event, Event::Completed { request: key, .. } if *key == request);
            events.push(event);
            if done {
                return events;
            }
        }
        assert!(
            Instant::now() < deadline,
            "executor did not settle accepted request"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn run<T: ExecutionTransport>(session: &mut ExecutionSession<T>) -> Vec<String> {
    let commands = [
        Command::LoadHtml {
            address: "https://fixture.invalid/document".into(),
            html:
                "<!doctype html><title>Execution proof</title><p>needle here</p><p>needle there</p>"
                    .into(),
        },
        Command::Find {
            text: "needle".into(),
            backwards: false,
        },
        Command::Zoom(1.25),
    ];
    let mut transcript = Vec::new();
    for (index, command) in commands.into_iter().enumerate() {
        let key = session
            .request(RequestId(index as u64 + 1), command)
            .unwrap();
        for envelope in drive(session, key) {
            assert_eq!(envelope.scope, session.id().scope);
            let line = match envelope.event {
                Event::NavigationStarted { address } => format!("started:{address}"),
                Event::AddressChanged(address) => format!("address:{address}"),
                Event::TitleChanged(title) => format!("title:{title}"),
                Event::NavigationFinished { address, success } => {
                    format!("finished:{address}:{success}")
                }
                Event::Completed { request, result } => {
                    assert_eq!(request, key);
                    match result.unwrap() {
                        Reply::Done => "done".into(),
                        Reply::Find(state) => {
                            assert_eq!(state.count, 2);
                            assert!(state.complete);
                            format!(
                                "find:{}:{}:{:?}",
                                state.query.text, state.count, state.current
                            )
                        }
                        Reply::Zoom(state) => {
                            assert_eq!(state.requested, 1.25);
                            assert_eq!(state.applied, 1.25);
                            format!("zoom:{}:{}", state.requested, state.applied)
                        }
                        other => panic!("unexpected reply: {other:?}"),
                    }
                }
                other => panic!("unexpected observation: {other:?}"),
            };
            transcript.push(format!("{}:{line}", envelope.sequence));
        }
    }
    assert!(
        transcript
            .iter()
            .any(|line| line.ends_with("title:Execution proof"))
    );
    transcript
}
fn wait_stopped<T: ExecutionTransport>(session: &mut ExecutionSession<T>) -> StopOutcome {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        session.advance().unwrap();
        if let Some(outcome) = session.stopped() {
            return outcome;
        }
        assert!(
            Instant::now() < deadline,
            "executor was not observed stopped"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn same_genet_document_commands_run_locally_and_in_a_process() {
    let helper = Path::new(env!("CARGO_BIN_EXE_cue-execution-proof"));
    let mut local = command_session(Local::new().unwrap(), 1, Termination::Cooperative);
    let mut remote = command_session(
        Process::spawn(helper, false).unwrap(),
        2,
        Termination::Forced,
    );
    assert_eq!(run(&mut local), run(&mut remote));
    local.retire().unwrap();
    remote.retire().unwrap();
    assert_eq!(wait_stopped(&mut local), StopOutcome::Exited);
    assert_eq!(wait_stopped(&mut remote), StopOutcome::Terminated);
}

#[test]
fn accepted_work_is_settled_on_host_timeout_and_child_is_reaped() {
    let helper = Path::new(env!("CARGO_BIN_EXE_cue-execution-proof"));
    let mut session = command_session(
        Process::spawn(helper, true).unwrap(),
        3,
        Termination::Forced,
    );
    let key = session.request(RequestId(1), Command::Reload).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        session.advance().unwrap();
        if let Some(event) = session.poll_event() {
            assert!(
                matches!(event.event, Event::Message(ref value) if value == "proof hold entered")
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "helper did not enter the injected stall"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    session.lose("host deadline".into()).unwrap();
    let events = std::iter::from_fn(|| session.poll_event()).collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    assert!(
        matches!(&events[0].event, Event::Completed { request, result: Err(Error::ExecutionLost(reason)) } if *request == key && reason == "host deadline")
    );
    assert_eq!(wait_stopped(&mut session), StopOutcome::Terminated);
    session.retire().unwrap();
    assert!(session.poll_event().is_none());
    assert_eq!(
        session.request(RequestId(2), Command::Reload),
        Err(Error::Retired)
    );
}
