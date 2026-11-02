// SPDX-License-Identifier: MPL-2.0
//! Bounded producer leases. Completion and consumer release are independent.
use crate::{Error, Result, Scope};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotLease {
    pub scope: Scope,
    pub slot: u32,
    /// Monotonic across the whole pool, including reuse of another slot.
    pub epoch: u64,
}
#[derive(Clone, Copy, Debug)]
struct Occupied {
    lease: SlotLease,
    producer_done: bool,
    consumer_released: bool,
}
/// Owns reuse policy, not storage or transport. Retiring the pool forbids new
/// leases; it does not infer either GPU completion from an executor's exit.
pub struct FrameSlotPool {
    scope: Scope,
    slots: Vec<Option<Occupied>>,
    epoch: u64,
    retired: bool,
}
impl FrameSlotPool {
    pub fn new(scope: Scope, capacity: u32) -> Result<Self> {
        if !(1..=64).contains(&capacity) {
            return Err(Error::Invalid("frame slot capacity must be 1..=64".into()));
        }
        Ok(Self {
            scope,
            slots: vec![None; capacity as usize],
            epoch: 0,
            retired: false,
        })
    }
    pub fn acquire(&mut self) -> Result<SlotLease> {
        if self.retired {
            return Err(Error::Retired);
        }
        let slot = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(Error::Backpressure)?;
        self.epoch = self.epoch.checked_add(1).ok_or(Error::Exhausted)?;
        let lease = SlotLease {
            scope: self.scope,
            slot: slot as u32,
            epoch: self.epoch,
        };
        self.slots[slot] = Some(Occupied {
            lease,
            producer_done: false,
            consumer_released: false,
        });
        Ok(lease)
    }
    fn complete(&mut self, lease: SlotLease, producer: bool) -> Result<()> {
        if lease.scope != self.scope {
            return Err(Error::Stale);
        }
        let slot = self
            .slots
            .get_mut(lease.slot as usize)
            .ok_or(Error::Stale)?;
        let occupied = slot
            .as_mut()
            .filter(|o| o.lease == lease)
            .ok_or(Error::Stale)?;
        let flag = if producer {
            &mut occupied.producer_done
        } else {
            &mut occupied.consumer_released
        };
        if *flag {
            return Err(Error::NotPending);
        }
        *flag = true;
        if occupied.producer_done && occupied.consumer_released {
            *slot = None;
        }
        Ok(())
    }
    pub fn producer_done(&mut self, lease: SlotLease) -> Result<()> {
        self.complete(lease, true)
    }
    pub fn consumer_released(&mut self, lease: SlotLease) -> Result<()> {
        self.complete(lease, false)
    }
    pub fn occupied(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }
    pub fn retire(&mut self) {
        self.retired = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pool(n: u32) -> FrameSlotPool {
        FrameSlotPool::new(
            Scope {
                owner: 1,
                generation: 2,
            },
            n,
        )
        .unwrap()
    }
    #[test]
    fn early_release_cannot_reuse_unfinished_storage() {
        let mut p = pool(1);
        let a = p.acquire().unwrap();
        p.consumer_released(a).unwrap();
        assert_eq!(p.acquire(), Err(Error::Backpressure));
        assert_eq!(p.consumer_released(a), Err(Error::NotPending));
        p.producer_done(a).unwrap();
        let b = p.acquire().unwrap();
        assert_eq!(a.slot, b.slot);
        assert!(b.epoch > a.epoch);
        assert_eq!(p.producer_done(a), Err(Error::Stale));
    }
    #[test]
    fn two_slots_need_matching_completion_and_release() {
        let mut p = pool(2);
        let a = p.acquire().unwrap();
        let b = p.acquire().unwrap();
        p.producer_done(a).unwrap();
        assert_eq!(p.producer_done(a), Err(Error::NotPending));
        p.producer_done(b).unwrap();
        assert_eq!(p.acquire(), Err(Error::Backpressure));
        let mut stale = a;
        stale.scope.generation += 1;
        assert_eq!(p.consumer_released(stale), Err(Error::Stale));
        assert_eq!(p.occupied(), 2);
        p.consumer_released(b).unwrap();
        let c = p.acquire().unwrap();
        assert!(c.epoch > b.epoch && b.epoch > a.epoch);
        assert_eq!(p.consumer_released(b), Err(Error::Stale));
        assert_eq!(p.occupied(), 2);
    }
    #[test]
    fn bounded_and_terminal_retirement() {
        assert!(
            FrameSlotPool::new(
                Scope {
                    owner: 0,
                    generation: 0
                },
                0
            )
            .is_err()
        );
        assert!(
            FrameSlotPool::new(
                Scope {
                    owner: 0,
                    generation: 0
                },
                65
            )
            .is_err()
        );
        let mut p = pool(1);
        let a = p.acquire().unwrap();
        p.retire();
        assert_eq!(p.occupied(), 1);
        assert_eq!(p.acquire(), Err(Error::Retired));
        p.consumer_released(a).unwrap();
        assert_eq!(p.occupied(), 1);
        p.producer_done(a).unwrap();
        assert_eq!(p.occupied(), 0);
        assert_eq!(p.acquire(), Err(Error::Retired));
    }
}
