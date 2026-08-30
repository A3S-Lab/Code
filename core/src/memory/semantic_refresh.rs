#[path = "semantic_refresh/metrics.rs"]
mod metrics;

pub use metrics::{
    SemanticRefreshMetrics, SemanticRefreshRunMetrics, SemanticRefreshRunOutcome,
    SEMANTIC_REFRESH_RECENT_RUN_LIMIT,
};

use super::maintenance::{
    validate_interval, MemoryMaintenanceContext, MemoryMaintenanceError, MemoryMaintenanceJob,
    MemoryMaintenanceOutcome, ScheduledMemoryMaintenance,
};
use super::AgentMemory;
use crate::durable_memory::{
    DurableMemorySemanticRefreshCheckpoint, DurableMemorySemanticRefreshReceipt,
    DurableMemorySemanticRefreshRun, SemanticRefreshEmbeddingCache,
};
use a3s_memory::vector::VectorMutationConsistency;
use async_trait::async_trait;
use metrics::SemanticRefreshRunObservation;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Reserved health name of Code's verified semantic refresh worker.
pub const SEMANTIC_REFRESH_JOB_NAME: &str = "v2_semantic_refresh";

/// Explicit periodic schedule for verified semantic memory refresh.
///
/// The schedule always requires atomic global index-revision CAS. This prevents
/// independently constructed session runtimes from silently falling back to a
/// process-local ordering guarantee. Construction is inert; a worker starts
/// only after the schedule is installed in [`super::MemoryMaintenanceOptions`]
/// and an asynchronous session is built. Clones form one ownership family, so
/// only one active maintenance runtime can publish its shared receipt and
/// metrics epoch at once.
#[derive(Clone)]
#[must_use = "a semantic refresh schedule does nothing until installed in maintenance options"]
pub struct ScheduledSemanticRefresh {
    interval: Duration,
    state: Arc<RwLock<ScheduledSemanticRefreshState>>,
    claimed: Arc<AtomicBool>,
}

#[derive(Default)]
struct ScheduledSemanticRefreshState {
    last_receipt: Option<DurableMemorySemanticRefreshReceipt>,
    recovery_receipt: Option<DurableMemorySemanticRefreshReceipt>,
    embedding_cache: Option<Arc<SemanticRefreshEmbeddingCache>>,
    metrics: SemanticRefreshMetrics,
}

impl ScheduledSemanticRefresh {
    pub fn try_new(interval: Duration) -> Result<Self, MemoryMaintenanceError> {
        Self::try_new_inner(interval, None)
    }

    /// Construct a schedule that verifies and adopts persisted refresh evidence.
    ///
    /// The checkpoint never authorizes the repository-token fast path. Its
    /// first run reads a complete bounded Active snapshot and checks the current
    /// index before it can become this ownership epoch's successful receipt.
    pub fn try_new_with_checkpoint(
        interval: Duration,
        checkpoint: DurableMemorySemanticRefreshCheckpoint,
    ) -> Result<Self, MemoryMaintenanceError> {
        checkpoint.verify().map_err(|error| {
            invalid(
                "semanticRefresh.checkpoint",
                format!("failed validation: {error}"),
            )
        })?;
        Self::try_new_inner(interval, Some(checkpoint.into_recovery_receipt()))
    }

    fn try_new_inner(
        interval: Duration,
        recovery_receipt: Option<DurableMemorySemanticRefreshReceipt>,
    ) -> Result<Self, MemoryMaintenanceError> {
        validate_interval(interval)?;
        Ok(Self {
            interval,
            state: Arc::new(RwLock::new(ScheduledSemanticRefreshState {
                recovery_receipt,
                ..ScheduledSemanticRefreshState::default()
            })),
            claimed: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn required_consistency(&self) -> VectorMutationConsistency {
        VectorMutationConsistency::IndexRevisionCas
    }

    /// Return the most recent successful, secret-free refresh receipt.
    ///
    /// Clones share this observation state for one active ownership epoch. A
    /// failed later run leaves the last successful receipt intact while generic
    /// maintenance health records the failure. A replacement owner starts a new
    /// epoch and clears the process-local receipt before its first run, so an
    /// optional source change token is never reused across repository owners.
    pub fn last_receipt(&self) -> Option<DurableMemorySemanticRefreshReceipt> {
        read_unpoisoned(&self.state).last_receipt.clone()
    }

    /// Return bounded, non-sensitive evidence for the current ownership epoch.
    ///
    /// A never-owned schedule reports epoch zero. Clean close retains the last
    /// epoch for inspection; a successful replacement claim increments the
    /// epoch and clears all prior counters and recent runs.
    pub fn metrics(&self) -> SemanticRefreshMetrics {
        read_unpoisoned(&self.state).metrics.clone()
    }

    pub(super) fn validate_for(&self, memory: &AgentMemory) -> Result<(), MemoryMaintenanceError> {
        let durable = memory.durable_memory().ok_or_else(|| {
            invalid(
                "semanticRefresh",
                "requires an exact durable-memory binding",
            )
        })?;
        let semantic = durable.semantic_recall().ok_or_else(|| {
            invalid(
                "semanticRefresh",
                "requires an attached semantic recall generation",
            )
        })?;
        let actual = semantic.mutation_consistency();
        if actual != self.required_consistency() {
            return Err(invalid(
                "semanticRefresh.mutationConsistency",
                format!(
                    "requires {:?}, but the backend provides {actual:?}",
                    self.required_consistency()
                ),
            ));
        }
        Ok(())
    }

    pub(super) fn try_claim(
        &self,
    ) -> Result<ScheduledSemanticRefreshClaim, MemoryMaintenanceError> {
        let claim = self
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ScheduledSemanticRefreshClaim {
                _lease: Arc::new(ScheduledSemanticRefreshClaimLease {
                    claimed: Arc::clone(&self.claimed),
                    state: Arc::clone(&self.state),
                }),
            })
            .map_err(|_| MemoryMaintenanceError::SemanticRefreshAlreadyOwned)?;
        let mut state = write_unpoisoned(&self.state);
        let ownership_epoch = state.metrics.ownership_epoch().saturating_add(1);
        let recovery_receipt = state.recovery_receipt.take();
        *state = ScheduledSemanticRefreshState {
            recovery_receipt,
            metrics: SemanticRefreshMetrics::for_epoch(ownership_epoch),
            ..ScheduledSemanticRefreshState::default()
        };
        Ok(claim)
    }

    pub(super) fn as_maintenance(
        &self,
    ) -> Result<ScheduledMemoryMaintenance, MemoryMaintenanceError> {
        ScheduledMemoryMaintenance::try_new(
            SEMANTIC_REFRESH_JOB_NAME,
            self.interval,
            Arc::new(SemanticRefreshJob {
                state: Arc::clone(&self.state),
            }),
        )
    }
}

#[derive(Clone)]
pub(super) struct ScheduledSemanticRefreshClaim {
    _lease: Arc<ScheduledSemanticRefreshClaimLease>,
}

struct ScheduledSemanticRefreshClaimLease {
    claimed: Arc<AtomicBool>,
    state: Arc<RwLock<ScheduledSemanticRefreshState>>,
}

impl Drop for ScheduledSemanticRefreshClaimLease {
    fn drop(&mut self) {
        // The host-held schedule keeps the receipt observable after close, but
        // vectors are useful only while this ownership epoch can run again.
        // Clear them before making the next claim visible.
        write_unpoisoned(&self.state).embedding_cache = None;
        self.claimed.store(false, Ordering::Release);
    }
}

impl std::fmt::Debug for ScheduledSemanticRefresh {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = read_unpoisoned(&self.state);
        formatter
            .debug_struct("ScheduledSemanticRefresh")
            .field("interval", &self.interval)
            .field("required_consistency", &self.required_consistency())
            .field("has_receipt", &state.last_receipt.is_some())
            .field("has_recovery_checkpoint", &state.recovery_receipt.is_some())
            .field("ownership_epoch", &state.metrics.ownership_epoch())
            .field("attempted_runs", &state.metrics.attempted_runs())
            .finish()
    }
}

struct SemanticRefreshJob {
    state: Arc<RwLock<ScheduledSemanticRefreshState>>,
}

#[async_trait]
impl MemoryMaintenanceJob for SemanticRefreshJob {
    async fn run(
        &self,
        context: &MemoryMaintenanceContext,
        cancellation: CancellationToken,
    ) -> anyhow::Result<MemoryMaintenanceOutcome> {
        let durable = context
            .durable_memory()
            .ok_or_else(|| anyhow::anyhow!("scheduled semantic refresh binding is unavailable"))?;
        let (previous, previous_cache, previous_requires_index_continuity) = {
            let state = read_unpoisoned(&self.state);
            let (previous, requires_continuity) = match state.last_receipt.as_ref() {
                Some(receipt) => (Some(receipt.clone()), false),
                None => (
                    state.recovery_receipt.clone(),
                    state.recovery_receipt.is_some(),
                ),
            };
            (previous, state.embedding_cache.clone(), requires_continuity)
        };
        let attempt = durable
            .refresh_semantic_recall_scheduled(
                previous.as_ref(),
                previous_cache.as_deref(),
                previous_requires_index_continuity,
                cancellation,
            )
            .await;
        let outcome = match &attempt.result {
            Ok(DurableMemorySemanticRefreshRun::Published { .. }) => {
                SemanticRefreshRunOutcome::Published
            }
            Ok(DurableMemorySemanticRefreshRun::Unchanged(_)) => {
                SemanticRefreshRunOutcome::Unchanged
            }
            Err(_) => SemanticRefreshRunOutcome::Failed,
        };
        let observation = SemanticRefreshRunObservation {
            outcome,
            elapsed: attempt.elapsed,
            source_change_token_requests: attempt.work.source_change_token_requests,
            source_change_token_observations: attempt.work.source_change_token_observations,
            source_snapshot_requests: attempt.work.source_snapshot_requests,
            source_snapshot_node_reads: attempt.work.source_snapshot_node_reads,
            source_snapshot_bytes: attempt.work.source_snapshot_bytes,
            embedding_cache_hits: attempt.work.embedding_cache_hits,
            embedding_inputs: attempt.work.embedding_inputs,
            embedding_input_bytes: attempt.work.embedding_input_bytes,
            provider_requests: attempt.work.provider_requests,
            provider_inputs: attempt.work.provider_inputs,
            provider_input_bytes: attempt.work.provider_input_bytes,
            publication_attempts: attempt.work.publication_attempts,
            publication_records: attempt.work.publication_records,
        };
        let mut state = write_unpoisoned(&self.state);
        state.metrics.record(observation);
        let affected_items = match attempt.result {
            Ok(DurableMemorySemanticRefreshRun::Published {
                receipt,
                embedding_cache,
            }) => {
                let affected_items = receipt.active_node_count();
                state.last_receipt = Some(receipt);
                state.recovery_receipt = None;
                state.embedding_cache = embedding_cache;
                affected_items
            }
            Ok(DurableMemorySemanticRefreshRun::Unchanged(receipt)) => {
                state.last_receipt = Some(receipt);
                state.recovery_receipt = None;
                0
            }
            Err(error) => {
                return Err(anyhow::anyhow!(error.redacted_message()));
            }
        };
        Ok(MemoryMaintenanceOutcome::new(affected_items))
    }
}

fn invalid(field: &'static str, reason: impl Into<String>) -> MemoryMaintenanceError {
    MemoryMaintenanceError::InvalidConfiguration {
        field,
        reason: reason.into(),
    }
}

fn read_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_unpoisoned<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
