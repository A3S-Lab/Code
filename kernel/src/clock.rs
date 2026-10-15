//! Injectable logical time for identity adapters and deterministic tests.

use crate::identity::CoreIdentityError;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Injectable logical time source for event adapters and deterministic tests.
pub trait LogicalClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLogicalClock;

impl LogicalClock for SystemLogicalClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
            .unwrap_or(0)
    }
}

#[derive(Debug)]
pub struct ManualLogicalClock {
    now_ms: AtomicU64,
}

impl ManualLogicalClock {
    pub const fn new(initial_ms: u64) -> Self {
        Self {
            now_ms: AtomicU64::new(initial_ms),
        }
    }

    pub fn set(&self, value: u64) {
        self.now_ms.store(value, Ordering::SeqCst);
    }

    pub fn advance(&self, delta_ms: u64) -> Result<u64, CoreIdentityError> {
        let mut current = self.now_ms.load(Ordering::SeqCst);
        loop {
            let next = current
                .checked_add(delta_ms)
                .ok_or(CoreIdentityError::SequenceOverflow)?;
            match self
                .now_ms
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Ok(next),
                Err(actual) => current = actual,
            }
        }
    }
}

impl LogicalClock for ManualLogicalClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_is_injectable_and_overflow_is_explicit() {
        let clock = ManualLogicalClock::new(10);
        assert_eq!(clock.now_ms(), 10);
        assert_eq!(clock.advance(5).unwrap(), 15);
        clock.set(u64::MAX);
        assert_eq!(clock.advance(1), Err(CoreIdentityError::SequenceOverflow));
    }

    #[test]
    fn system_clock_reads_time_without_starting_a_runtime() {
        let clock = SystemLogicalClock;
        assert!(clock.now_ms() > 1_700_000_000_000);
    }
}
