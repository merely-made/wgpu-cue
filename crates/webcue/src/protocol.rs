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
        })
    }
    pub fn scope(&self) -> Scope {
        self.scope
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
        let used = self.events.len() + self.pending.len();
        if used.saturating_add(extra) > self.capacity {
            return Err(Error::Backpressure);
        }
        // Reserve sequence numbers as well as storage for all completions.
        if self
            .sequence
            .checked_add((self.pending.len() + extra) as u64)
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
    pub fn emit(&mut self, event: Event) -> Result<()> {
        self.ensure_live()?;
        if matches!(event, Event::Completed { .. }) {
            return Err(Error::Invalid("complete requests through settle".into()));
        }
        self.reserve(1)?;
        self.push(event);
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
