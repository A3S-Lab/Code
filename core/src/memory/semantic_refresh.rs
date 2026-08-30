use super::maintenance::{
    validate_interval, MemoryMaintenanceContext, MemoryMaintenanceError, MemoryMaintenanceJob,
    MemoryMaintenanceOutcome, ScheduledMemoryMaintenance,
};
use super::AgentMemory;
use crate::durable_memory::DurableMemorySemanticRefreshReceipt;
use a3s_memory::vector::VectorMutationConsistency;
use async_trait::async_trait;
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
/// only one active maintenance runtime can publish its shared receipt at once.
#[derive(Clone)]
#[must_use = "a semantic refresh schedule does nothing until installed in maintenance options"]
pub struct ScheduledSemanticRefresh {
    interval: Duration,
    last_receipt: Arc<RwLock<Option<DurableMemorySemanticRefreshReceipt>>>,
    claimed: Arc<AtomicBool>,
}

impl ScheduledSemanticRefresh {
    pub fn try_new(interval: Duration) -> Result<Self, MemoryMaintenanceError> {
        validate_interval(interval)?;
        Ok(Self {
            interval,
            last_receipt: Arc::new(RwLock::new(None)),
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
    /// Clones share this observation state. A failed later run leaves the last
    /// successful receipt intact while generic maintenance health records the
    /// failure.
    pub fn last_receipt(&self) -> Option<DurableMemorySemanticRefreshReceipt> {
        read_unpoisoned(&self.last_receipt).clone()
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
        self.claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ScheduledSemanticRefreshClaim {
                _lease: Arc::new(ScheduledSemanticRefreshClaimLease {
                    claimed: Arc::clone(&self.claimed),
                }),
            })
            .map_err(|_| MemoryMaintenanceError::SemanticRefreshAlreadyOwned)
    }

    pub(super) fn as_maintenance(
        &self,
    ) -> Result<ScheduledMemoryMaintenance, MemoryMaintenanceError> {
        ScheduledMemoryMaintenance::try_new(
            SEMANTIC_REFRESH_JOB_NAME,
            self.interval,
            Arc::new(SemanticRefreshJob {
                last_receipt: Arc::clone(&self.last_receipt),
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
}

impl Drop for ScheduledSemanticRefreshClaimLease {
    fn drop(&mut self) {
        self.claimed.store(false, Ordering::Release);
    }
}

impl std::fmt::Debug for ScheduledSemanticRefresh {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ScheduledSemanticRefresh")
            .field("interval", &self.interval)
            .field("required_consistency", &self.required_consistency())
            .field("has_receipt", &self.last_receipt().is_some())
            .finish()
    }
}

struct SemanticRefreshJob {
    last_receipt: Arc<RwLock<Option<DurableMemorySemanticRefreshReceipt>>>,
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
        let receipt = durable
            .refresh_semantic_recall_requiring(
                VectorMutationConsistency::IndexRevisionCas,
                cancellation,
            )
            .await
            .map_err(|error| anyhow::anyhow!(error.redacted_message()))?;
        let affected_items = receipt.active_node_count();
        *write_unpoisoned(&self.last_receipt) = Some(receipt);
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
