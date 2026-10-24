// SPDX-License-Identifier: MPL-2.0
use crate::*;
use std::collections::VecDeque;
/// Bounded ordered mailbox. Each accepted request reserves its completion slot,
/// so observation backpressure cannot prevent retirement from settling work.
/// IDs increase strictly within a scope; a completed ID cannot be reused.
pub struct Protocol {
    scope: Scope,
    sequence: u64,
    capacity: usize,
    last_id: Option<RequestId>,
    pending: VecDeque<RequestId>,
    events: VecDeque<Envelope>,
    retired: bool,
    text_input: Option<TextInputState>,
}
impl Protocol {
    pub fn new(scope: Scope, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::Invalid("mailbox capacity must be positive".into()));
        }
        Ok(Self {
            scope,
            sequence: 0,
            capacity,
            last_id: None,
            pending: VecDeque::new(),
            events: VecDeque::new(),
            retired: false,
            text_input: None,
        })
    }
    pub fn scope(&self) -> Scope {
        self.scope
    }
    /// Latest successfully published editor observation. This is the same
    /// authority as TextInput events, including their clearing reservation.
    pub fn text_input_state(&self) -> Option<&TextInputState> {
        self.text_input.as_ref()
    }
    pub fn ensure_live(&self) -> Result<()> {
        if self.retired {
            Err(Error::Retired)
        } else {
            Ok(())
        }
    }
    pub fn accept(&mut self, id: RequestId) -> Result<RequestKey> {
        self.ensure_live()?;
        if self.last_id.is_some_and(|last| id <= last) {
            return Err(Error::RequestOrder);
        }
        self.reserve(1)?;
        self.last_id = Some(id);
        self.pending.push_back(id);
        Ok(RequestKey {
            scope: self.scope,
            id,
        })
    }
    fn reserve(&self, extra: usize) -> Result<()> {
        let used = self.events.len() + self.pending.len() + usize::from(self.text_input.is_some());
        if used.saturating_add(extra) > self.capacity {
            return Err(Error::Backpressure);
        }
        // Reserve sequence numbers as well as storage for all completions.
        if self
            .sequence
            .checked_add(
                (self.pending.len() + usize::from(self.text_input.is_some()) + extra) as u64,
            )
            .is_none()
        {
            return Err(Error::Exhausted);
        }
        Ok(())
    }
    fn push(&mut self, event: Event) {
        self.sequence += 1;
        self.events.push_back(Envelope {
            scope: self.scope,
            sequence: self.sequence,
            event,
        });
    }
    /// Check room for an atomic batch of observations before changing producer state.
    /// The single owner must emit that batch before accepting other work.
    pub fn ensure_observations(&self, count: usize) -> Result<()> {
        self.ensure_live()?;
        self.reserve(count)
    }
    pub fn emit(&mut self, event: Event) -> Result<()> {
        self.ensure_live()?;
        if let Event::TextInput(state) = event {
            return self.publish_text_input(state);
        }
        if matches!(event, Event::Completed { .. }) {
            return Err(Error::Invalid("complete requests through settle".into()));
        }
        self.reserve(1)?;
        self.push(event);
        Ok(())
    }
    /// Publish a changed editor observation. A live editor reserves one extra
    /// slot for its eventual clearing event, so hide/retirement cannot strand
    /// host geometry behind a full mailbox or pending request completions.
    pub fn publish_text_input(&mut self, state: Option<TextInputState>) -> Result<()> {
        self.ensure_live()?;
        if state == self.text_input {
            return Ok(());
        }
        if let Some(value) = &state {
            let r = value.caret;
            if ![r.x, r.y, r.width, r.height]
                .into_iter()
                .all(f64::is_finite)
                || r.width < 0.0
                || r.height < 0.0
            {
                return Err(Error::Invalid(
                    "text-input geometry must be finite and nonnegative in extent".into(),
                ));
            }
            self.reserve(if self.text_input.is_some() { 1 } else { 2 })?;
        }
        // None consumes the clearing slot already reserved by Some. Updating
        // only after reservation preserves the old observation on backpressure.
        self.text_input = state.clone();
        self.push(Event::TextInput(state));
        Ok(())
    }

    pub fn settle(&mut self, request: RequestKey, result: Result<Reply>) -> Result<()> {
        if request.scope != self.scope {
            return Err(Error::Stale);
        }
        let Some(index) = self.pending.iter().position(|id| *id == request.id) else {
            return Err(Error::NotPending);
        };
        self.pending.remove(index);
        self.push(Event::Completed { request, result });
        Ok(())
    }
    pub fn retire(&mut self) -> Result<()> {
        if self.retired {
            return Ok(());
        }
        if self.text_input.take().is_some() {
            self.push(Event::TextInput(None));
        }
        self.retired = true;
        while let Some(id) = self.pending.pop_front() {
            let request = RequestKey {
                scope: self.scope,
                id,
            };
            self.push(Event::Completed {
                request,
                result: Err(Error::Retired),
            });
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Option<Envelope> {
        self.events.pop_front()
    }
    /// Exact queued observations plus reserved terminal events on retirement.
    pub fn retirement_len(&self) -> usize {
        self.events.len() + self.pending.len() + usize::from(self.text_input.is_some())
    }
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// Tracks a publication life independently of pixels. Reusing a numeric node
/// key cannot revive an action from an older publication or surface generation.
pub struct PublicationTracker {
    scope: Scope,
    current: Option<PublicationKey>,
    last_revision: Option<u64>,
}
impl PublicationTracker {
    pub fn new(scope: Scope) -> Self {
        Self {
            scope,
            current: None,
            last_revision: None,
        }
    }
    pub fn publish(&mut self, revision: u64) -> Result<PublicationKey> {
        if self.last_revision.is_some_and(|old| revision <= old) {
            return Err(Error::Stale);
        }
        let key = PublicationKey {
            scope: self.scope,
            revision,
        };
        self.current = Some(key);
        self.last_revision = Some(revision);
        Ok(key)
    }
    pub fn validate(&self, key: PublicationKey) -> Result<()> {
        if self.current == Some(key) {
            Ok(())
        } else {
            Err(Error::Stale)
        }
    }
    pub fn retire(&mut self) -> Option<PublicationKey> {
        self.current.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn observation_batch_preflight_preserves_completion_reservations() {
        let mut protocol = Protocol::new(
            Scope {
                owner: 1,
                generation: 1,
            },
            3,
        )
        .unwrap();
        let request = protocol.accept(RequestId(1)).unwrap();
        assert!(protocol.ensure_observations(2).is_ok());
        protocol.emit(Event::Message("first".into())).unwrap();
        assert_eq!(protocol.ensure_observations(2), Err(Error::Backpressure));
        assert_eq!(protocol.retirement_len(), 2);
        protocol.retire().unwrap();
        assert!(matches!(protocol.poll().unwrap().event, Event::Message(_)));
        assert!(
            matches!(protocol.poll().unwrap().event,Event::Completed{request:key,result:Err(Error::Retired)} if key==request)
        );
    }
    #[test]
    fn sequence_exhaustion_cannot_steal_a_reserved_terminal_event() {
        let mut protocol = Protocol::new(
            Scope {
                owner: 1,
                generation: 1,
            },
            8,
        )
        .unwrap();
        protocol.sequence = u64::MAX - 2;
        let request = protocol.accept(RequestId(1)).unwrap();
        protocol
            .emit(Event::TitleChanged("last observation".into()))
            .unwrap();
        assert_eq!(
            protocol.emit(Event::Diagnostic("would exhaust".into())),
            Err(Error::Exhausted)
        );
        assert_eq!(protocol.accept(RequestId(2)), Err(Error::Exhausted));
        protocol.retire().unwrap();
        assert_eq!(protocol.poll().unwrap().sequence, u64::MAX - 1);
        let final_event = protocol.poll().unwrap();
        assert_eq!(final_event.sequence, u64::MAX);
        assert!(
            matches!(final_event.event,Event::Completed {request:actual,result:Err(Error::Retired)} if actual==request)
        );
    }
}

#[cfg(test)]
mod text_input_tests {
    use super::*;
    fn scope() -> Scope {
        Scope {
            owner: 91,
            generation: 1,
        }
    }
    fn state(x: f64) -> TextInputState {
        TextInputState {
            element_kind: "input".into(),
            input_type: "text".into(),
            input_mode: String::new(),
            autocomplete: String::new(),
            multiline: false,
            password: false,
            selection_utf16: (3, 3),
            caret: LogicalRect {
                x,
                y: 12.0,
                width: 1.0,
                height: 18.0,
            },
        }
    }
    #[test]
    fn text_input_clear_and_completion_survive_a_full_mailbox() {
        let mut protocol = Protocol::new(scope(), 3).unwrap();
        protocol.emit(Event::TextInput(Some(state(4.0)))).unwrap();
        let request = protocol.accept(RequestId(1)).unwrap();
        assert!(matches!(
            protocol.emit(Event::Message("full".into())),
            Err(Error::Backpressure)
        ));
        assert_eq!(protocol.retirement_len(), 3);
        protocol.retire().unwrap();
        assert!(matches!(
            protocol.poll().unwrap().event,
            Event::TextInput(Some(_))
        ));
        assert!(matches!(
            protocol.poll().unwrap().event,
            Event::TextInput(None)
        ));
        assert!(matches!(protocol.poll().unwrap().event,
            Event::Completed { request: key, result: Err(Error::Retired) } if key == request));
        assert!(protocol.poll().is_none());
    }
    #[test]
    fn text_input_backpressure_preserves_the_last_publication_and_sequence() {
        let mut protocol = Protocol::new(scope(), 4).unwrap();
        protocol.emit(Event::TextInput(Some(state(4.0)))).unwrap();
        protocol.emit(Event::Message("queued".into())).unwrap();
        protocol.accept(RequestId(1)).unwrap();
        protocol.emit(Event::TextInput(Some(state(4.0)))).unwrap(); // unchanged; no slot
        assert!(matches!(
            protocol.emit(Event::TextInput(Some(state(8.0)))),
            Err(Error::Backpressure)
        ));
        assert_eq!(protocol.poll().unwrap().sequence, 1);
        protocol.emit(Event::TextInput(Some(state(8.0)))).unwrap();
        assert_eq!(protocol.poll().unwrap().sequence, 2);
        let update = protocol.poll().unwrap();
        assert_eq!(update.sequence, 3);
        assert!(matches!(update.event, Event::TextInput(Some(value)) if value.caret.x == 8.0));
        protocol.retire().unwrap();
        assert!(matches!(
            protocol.poll().unwrap().event,
            Event::TextInput(None)
        ));
    }
    #[test]
    fn text_input_rejects_nonfinite_geometry_without_changing_admission() {
        let mut protocol = Protocol::new(scope(), 2).unwrap();
        assert!(matches!(
            protocol.emit(Event::TextInput(Some(state(f64::NAN)))),
            Err(Error::Invalid(_))
        ));
        protocol.emit(Event::TextInput(Some(state(4.0)))).unwrap();
        protocol.emit(Event::TextInput(None)).unwrap();
        assert!(matches!(
            protocol.poll().unwrap().event,
            Event::TextInput(Some(_))
        ));
        assert!(matches!(
            protocol.poll().unwrap().event,
            Event::TextInput(None)
        ));
    }
}
