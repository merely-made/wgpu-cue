// SPDX-License-Identifier: MPL-2.0
//! Host-owned request accounting across a replaceable execution boundary.
//!
//! This is a command endpoint, not a complete remote `Surface`. Transport and
//! process supervision remain with the host. An endpoint identity is routing
//! identity, never authority to access an origin, storage partition or resource.
use crate::*;
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExecutionId {
    pub scope: Scope,
    /// Host-chosen incarnation, distinct from an OS PID and installation generation.
    pub incarnation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Termination {
    Cooperative,
    /// The trusted host can stop and observe the executor even if work never yields.
    /// This does not cancel submitted GPU work or release exported frame custody.
    Forced,
}

#[derive(Clone, Debug)]
pub struct ExecutionRequest {
    pub execution: ExecutionId,
    pub request: RequestKey,
    pub command: Command,
}

#[derive(Clone, Debug)]
pub struct ExecutionUpdate {
    pub execution: ExecutionId,
    /// Strictly consecutive within an execution, starting at one.
    pub sequence: u64,
    pub event: Event,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendStatus {
    Sent,
    /// No work was admitted; the caller retains the request and may retry it.
    Busy,
}

/// Executor-lifetime evidence only. GPU completion, frame custody and external
/// side effects have separate owners and cannot be inferred from this outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopOutcome {
    Exited,
    Terminated,
    Crashed,
}

/// A single-owner, ordered transport supplied by the embedding host.
///
/// Methods must not wait for I/O, callbacks or executor completion. `try_send`
/// must never return Busy after admitting work. Errors mean uncertain delivery
/// and permanently close this session; requests are never replayed. Backends
/// must bound both message counts and encoded bytes, validate their wire format,
/// and bind the channel to the expected execution identity.
///
/// `try_recv` returns queued updates before reporting disconnection. `poll_stopped`
/// reports evidence of actual executor termination (including descendant ownership
/// where promised), not merely EOF. A host may continue polling it after retirement
/// or transport failure. No Send/Sync requirement is imposed.
pub trait ExecutionTransport {
    fn termination(&self) -> Termination;
    fn try_send(&mut self, request: &ExecutionRequest) -> Result<SendStatus>;
    fn try_recv(&mut self) -> Result<Option<ExecutionUpdate>>;
    /// Idempotently request shutdown; this does not itself prove the executor stopped.
    fn request_stop(&mut self) -> Result<()>;
    fn poll_stopped(&mut self) -> Result<Option<StopOutcome>>;
}

/// A surviving host owns admission and terminal event reservations. Dropping
/// this value discards that ledger; durability across host death is not claimed.
pub struct ExecutionSession<T> {
    id: ExecutionId,
    transport: T,
    protocol: Protocol,
    outgoing: VecDeque<ExecutionRequest>,
    held: Option<ExecutionUpdate>,
    last_sequence: u64,
    pending_limit: usize,
    closed: bool,
    stop_requested: bool,
    stopped: Option<StopOutcome>,
    loss: Option<String>,
}

impl<T: ExecutionTransport> ExecutionSession<T> {
    pub fn new(
        id: ExecutionId,
        transport: T,
        capacity: usize,
        required_termination: Termination,
    ) -> Result<Self> {
        if capacity < 3 {
            return Err(Error::Invalid(
                "execution mailbox needs a completion, an editor update and its clearing slot"
                    .into(),
            ));
        }
        if required_termination == Termination::Forced
            && transport.termination() != Termination::Forced
        {
            return Err(Error::Unsupported(
                "host requires forced executor termination".into(),
            ));
        }
        Ok(Self {
            id,
            transport,
            protocol: Protocol::new(id.scope, capacity)?,
            outgoing: VecDeque::new(),
            held: None,
            last_sequence: 0,
            pending_limit: capacity / 2,
            closed: false,
            stop_requested: false,
            stopped: None,
            loss: None,
        })
    }

    pub fn id(&self) -> ExecutionId {
        self.id
    }
    pub fn loss(&self) -> Option<&str> {
        self.loss.as_deref()
    }
    pub fn stopped(&self) -> Option<StopOutcome> {
        self.stopped
    }
    pub fn pending(&self) -> usize {
        self.protocol.pending()
    }
    pub fn poll_event(&mut self) -> Option<Envelope> {
        self.protocol.poll()
    }
    pub fn text_input_state(&self) -> Option<&TextInputState> {
        self.protocol.text_input_state()
    }

    /// Acceptance is local: the host assumes responsibility before any send.
    /// Remote rejection is a terminal result, not retroactive non-acceptance.
    pub fn request(&mut self, id: RequestId, command: Command) -> Result<RequestKey> {
        self.protocol.ensure_live()?;
        if self.protocol.pending() >= self.pending_limit {
            return Err(Error::Backpressure);
        }
        let request = self.protocol.accept(id)?;
        self.outgoing.push_back(ExecutionRequest {
            execution: self.id,
            request,
            command,
        });
        Ok(request)
    }

    /// One bounded host turn. Backpressured observations are held without
    /// reordering; drain the public event queue to make progress.
    pub fn advance(&mut self) -> Result<()> {
        const BUDGET: usize = 64;
        if !self.closed {
            for _ in 0..BUDGET {
                let Some(request) = self.outgoing.front() else {
                    break;
                };
                match self.transport.try_send(request) {
                    Ok(SendStatus::Sent) => {
                        self.outgoing.pop_front();
                    }
                    Ok(SendStatus::Busy) => break,
                    Err(error) => {
                        self.lose(format!("send: {error}"))?;
                        break;
                    }
                }
            }
        }
        if !self.closed {
            for _ in 0..BUDGET {
                let update = match self.held.take() {
                    Some(update) => update,
                    None => match self.transport.try_recv() {
                        Ok(Some(update)) => update,
                        Ok(None) => return self.observe_stop(),
                        Err(error) => {
                            self.lose(format!("receive: {error}"))?;
                            break;
                        }
                    },
                };
                if let Err(error) = self.validate(&update) {
                    self.lose(format!("invalid update: {error}"))?;
                    break;
                }
                let result = match &update.event {
                    Event::Completed { request, result } => {
                        self.protocol.settle(*request, result.clone())
                    }
                    event => self.protocol.emit(event.clone()),
                };
                match result {
                    Ok(()) => self.last_sequence = update.sequence,
                    Err(Error::Backpressure) => {
                        self.held = Some(update);
                        return Ok(());
                    }
                    Err(error) => {
                        self.lose(format!("invalid event: {error}"))?;
                        break;
                    }
                }
            }
            // Do not report exit before buffered updates have been drained.
            if !self.closed {
                return Ok(());
            }
        }
        self.observe_stop()
    }

    fn validate(&self, update: &ExecutionUpdate) -> Result<()> {
        if update.execution != self.id || self.last_sequence.checked_add(1) != Some(update.sequence)
        {
            return Err(Error::Stale);
        }
        let scope = match &update.event {
            Event::Completed { request, .. } => Some(request.scope),
            Event::Semantics { publication, .. } | Event::SemanticsRetired(publication) => {
                Some(publication.scope)
            }
            _ => None,
        };
        if scope.is_some_and(|scope| scope != self.id.scope) {
            return Err(Error::Stale);
        }
        if let Event::Completed { request, .. } = &update.event
            && self
                .outgoing
                .iter()
                .any(|queued| queued.request == *request)
        {
            return Err(Error::Invalid("completion precedes transmission".into()));
        }
        Ok(())
    }

    fn ask_stop(&mut self) -> Result<()> {
        if !self.stop_requested && self.stopped.is_none() {
            self.transport.request_stop()?;
            self.stop_requested = true;
        }
        Ok(())
    }

    fn observe_stop(&mut self) -> Result<()> {
        if self.stopped.is_none() {
            match self.transport.poll_stopped() {
                Ok(Some(outcome)) => {
                    self.stopped = Some(outcome);
                    if !self.closed {
                        self.lose(format!("executor stopped: {outcome:?}"))?;
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    self.lose(format!("supervision: {error}"))?;
                    return Err(error);
                }
            }
        }
        if self.closed {
            self.ask_stop()?;
        }
        Ok(())
    }

    /// Explicit host timeout/loss decision. The first cause is retained.
    pub fn lose(&mut self, reason: String) -> Result<()> {
        if !self.closed {
            self.protocol.execution_lost(reason.clone())?;
            self.loss = Some(reason);
            self.closed = true;
            self.outgoing.clear();
            self.held = None;
        }
        self.ask_stop()
    }

    /// Retires host-facing work immediately. Continue `advance` until the host
    /// observes `stopped()` if actual termination is required. Retired requests
    /// are not evidence that a previously sent external effect was rolled back.
    pub fn retire(&mut self) -> Result<()> {
        self.protocol.retire()?;
        self.closed = true;
        self.outgoing.clear();
        self.held = None;
        self.ask_stop()
    }
}
