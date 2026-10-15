//! Ephemeral proof projection of the existing validated original fact fold.

use super::{CacheProofOperation, ReceiptDebt, ReceiptProjection};
use std::collections::BTreeSet;

pub(in crate::fact_control) struct CacheProofCursor<'a> {
    debt: &'a ReceiptDebt,
    next_event: usize,
    last_boundary: Option<usize>,
    stopped: bool,
    claimed_uncertainty: BTreeSet<String>,
    missing_uncertainty: BTreeSet<String>,
    missing_background: BTreeSet<String>,
    pending: BTreeSet<String>,
    asserted: BTreeSet<String>,
    settled_children: BTreeSet<String>,
    proven_uncertainty: BTreeSet<String>,
}

impl<'a> CacheProofCursor<'a> {
    pub(super) fn new(
        debt: &'a ReceiptDebt,
        claimed_uncertainty: &BTreeSet<String>,
        claimed_background: &BTreeSet<String>,
    ) -> Self {
        Self {
            debt,
            next_event: 0,
            last_boundary: None,
            stopped: false,
            claimed_uncertainty: claimed_uncertainty.clone(),
            missing_uncertainty: claimed_uncertainty.clone(),
            missing_background: claimed_background.clone(),
            pending: BTreeSet::new(),
            asserted: BTreeSet::new(),
            settled_children: BTreeSet::new(),
            proven_uncertainty: BTreeSet::new(),
        }
    }

    /// Call once alongside each original receipt projection. Only newly
    /// reached original proof events are visited. Readiness is constant-time;
    /// it cannot be obtained by scanning or trusting cached claims.
    pub(in crate::fact_control) fn advance_to(&mut self, projection: &ReceiptProjection) -> bool {
        let Some(boundary) = self.debt.projection_fact_index(projection) else {
            self.stopped = true;
            return false;
        };
        self.advance_until(boundary)
    }

    /// This boundary is the actual end of the already-read original log,
    /// including preparation/negative inputs after its last receipt.
    pub(in crate::fact_control) fn finish(&mut self) -> bool {
        self.advance_until(usize::MAX)
    }

    /// Consume only at the selected candidate. The set contains original
    /// proven claim keys, not arbitrary cached or generic ledger debt.
    pub(in crate::fact_control) fn into_proven_uncertainty(self) -> BTreeSet<String> {
        self.proven_uncertainty
    }

    #[cfg(test)]
    pub(in crate::fact_control) fn processed_events(&self) -> usize {
        self.next_event
    }

    fn advance_until(&mut self, boundary: usize) -> bool {
        if self.stopped
            || self
                .last_boundary
                .is_some_and(|previous| boundary < previous)
        {
            self.stopped = true;
            return false;
        }
        // Copy the original read-only reference so applying an event mutates
        // only the cursor, never the shared fold or its event order.
        let debt = self.debt;
        while let Some(event) = debt.cache_proof_events.get(self.next_event) {
            if event.fact_index > boundary {
                break;
            }
            self.apply(&event.operation);
            self.next_event += 1;
        }
        self.last_boundary = Some(boundary);
        !self.debt.integrity_invalid
            && self.missing_uncertainty.is_empty()
            && self.missing_background.is_empty()
    }

    fn apply(&mut self, operation: &CacheProofOperation) {
        match operation {
            CacheProofOperation::Prepared(key) => {
                if self.claimed_uncertainty.contains(key) && !self.settled_children.contains(key) {
                    self.pending.insert(key.clone());
                    self.activate(key);
                }
            }
            CacheProofOperation::Terminal(key) => {
                if self.pending.remove(key) && !self.asserted.contains(key) {
                    self.deactivate(key);
                }
            }
            CacheProofOperation::AssertedUncertainty(key) => {
                if self.claimed_uncertainty.contains(key) && !self.settled_children.contains(key) {
                    self.asserted.insert(key.clone());
                    self.activate(key);
                }
            }
            CacheProofOperation::BackgroundReady(key) => {
                self.missing_background.remove(key);
            }
            CacheProofOperation::ChildSettled(key) => {
                if self.claimed_uncertainty.contains(key) {
                    self.settled_children.insert(key.clone());
                    self.pending.remove(key);
                    self.asserted.remove(key);
                    self.deactivate(key);
                }
            }
        }
    }

    fn activate(&mut self, key: &str) {
        self.proven_uncertainty.insert(key.into());
        self.missing_uncertainty.remove(key);
    }

    fn deactivate(&mut self, key: &str) {
        self.proven_uncertainty.remove(key);
        self.missing_uncertainty.insert(key.into());
    }
}
