//! CRDT op identity + Lamport clock.
//!
//! Phase 0 only ships the primitives — no op log, no sync, no plugin
//! trait yet. Phase 1 rebuilds `Doc` on top of these so every mutation
//! stamps an `OpId` even before Phase 10 wires transport.
//!
//! Lamport rules:
//! - `local_tick()`  → increment + return the new value.
//! - `observe_remote(remote)` → `self = max(self, remote) + 1`.
//! Comparison on `OpId` is `(lamport, actor)` lexicographic — total,
//! deterministic, resolves concurrent writes reproducibly.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::identity::ActorId;

/// Globally orderable op identifier. Travels with every CRDT op.
#[istmo::message]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct OpId {
    pub lamport: u64,
    pub actor: ActorId,
}

impl Ord for OpId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.lamport
            .cmp(&other.lamport)
            .then_with(|| self.actor.cmp(&other.actor))
    }
}

impl PartialOrd for OpId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// In-memory Lamport counter. One per workspace in the eventual model;
/// a single global instance is enough until Phase 4 brings workspaces.
#[derive(Debug, Default)]
pub struct LamportClock {
    value: AtomicU64,
}

impl LamportClock {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            value: AtomicU64::new(0),
        }
    }

    /// Return the current logical time without advancing it.
    #[must_use]
    pub fn peek(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }

    /// Local emit: advance by one and return the new value.
    pub fn local_tick(&self) -> u64 {
        self.value.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Remote receive: fast-forward past `remote` and reserve one tick
    /// for the local apply that follows.
    pub fn observe_remote(&self, remote: u64) -> u64 {
        let mut cur = self.value.load(Ordering::Relaxed);
        loop {
            let next = cur.max(remote) + 1;
            match self
                .value
                .compare_exchange_weak(cur, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return next,
                Err(now) => cur = now,
            }
        }
    }

    /// Mint an `OpId` for a locally-emitted op.
    #[must_use]
    pub fn emit(&self, actor: ActorId) -> OpId {
        OpId {
            lamport: self.local_tick(),
            actor,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{DeviceId, UserId};

    fn actor() -> ActorId {
        ActorId {
            user: UserId::new_v4(),
            device: DeviceId::new_v4(),
        }
    }

    #[test]
    fn local_tick_monotonic() {
        let c = LamportClock::new();
        assert_eq!(c.local_tick(), 1);
        assert_eq!(c.local_tick(), 2);
        assert_eq!(c.peek(), 2);
    }

    #[test]
    fn observe_remote_fast_forwards() {
        let c = LamportClock::new();
        c.local_tick(); // 1
        assert_eq!(c.observe_remote(10), 11);
        assert_eq!(c.local_tick(), 12);
    }

    #[test]
    fn opid_order_lexicographic() {
        let a = actor();
        let b = actor();
        let lo = OpId { lamport: 5, actor: a };
        let hi_lamport = OpId { lamport: 6, actor: a };
        assert!(hi_lamport > lo);
        let same_lamport_b = OpId { lamport: 5, actor: b };
        // tie-break deterministic — one of (a,b) is strictly greater.
        assert_ne!(lo.cmp(&same_lamport_b), std::cmp::Ordering::Equal);
    }
}
