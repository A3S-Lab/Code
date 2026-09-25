//! `DurableMemorySession` construction over the injected Redis backend.

use a3s_code_core::embedding::{EmbeddingExecutorConfig, EmbeddingProvider};
use a3s_code_core::memory::{
    AgentMemory, MemoryConfig, MemoryMaintenanceOptions, MemoryMaintenanceRuntime,
    ScheduledSemanticRefresh, SemanticRefreshRunMetrics, SemanticRefreshRunOutcome,
};
use a3s_code_core::{
    DurableMemoryRecallPolicy, DurableMemorySemanticRecall, DurableMemorySemanticRecallPolicy,
    DurableMemorySession,
};
use a3s_memory::repository::{MemoryNamespace, MemoryRepository};
use a3s_memory::vector::VectorIndex;
use a3s_memory::InMemoryStore;
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;

/// Recall breadth for the qualification pack.
pub const CANDIDATE_LIMIT: usize = 8;
/// Provider batch size; keeps one HTTP request per 32 inputs.
pub const EMBEDDING_BATCH_INPUTS: usize = 32;
const MIN_SCORE: f32 = 0.10;
const RUN_TIMEOUT: Duration = Duration::from_secs(180);

/// Non-secret authority digest naming the host that owns this generation.
pub fn authority_digest(revision: &str) -> String {
    format!(
        "sha256:{:x}",
        Sha256::digest(format!("a3s.code.dm-prod1-host:{revision}").as_bytes())
    )
}

/// Bind one Active-recall session to a host-owned semantic generation.
pub fn durable_session(
    repository: Arc<dyn MemoryRepository>,
    namespace: MemoryNamespace,
    provider: Arc<dyn EmbeddingProvider>,
    index: Arc<dyn VectorIndex>,
    authority_digest: &str,
    max_request_inputs: usize,
) -> Result<DurableMemorySession> {
    let semantic = DurableMemorySemanticRecall::new(
        authority_digest.to_string(),
        provider,
        EmbeddingExecutorConfig {
            max_batch_inputs: EMBEDDING_BATCH_INPUTS,
            max_request_inputs,
            ..EmbeddingExecutorConfig::default()
        },
        index,
        DurableMemorySemanticRecallPolicy::try_new(CANDIDATE_LIMIT, MIN_SCORE)?,
    )?;
    Ok(DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(CANDIDATE_LIMIT, 1.0)?,
    )
    .with_semantic_recall(semantic)?)
}

/// Start an owned maintenance runtime driving `schedule` for one session.
pub fn start_runtime(
    owner_id: &str,
    durable: DurableMemorySession,
    schedule: ScheduledSemanticRefresh,
) -> Result<Arc<MemoryMaintenanceRuntime>> {
    let memory = Arc::new(AgentMemory::with_config_observers_and_durable(
        Arc::new(InMemoryStore::new()),
        MemoryConfig::default(),
        Vec::new(),
        Some(durable),
    ));
    Ok(MemoryMaintenanceRuntime::start(
        owner_id,
        memory,
        MemoryMaintenanceOptions::new().with_semantic_refresh(schedule),
    )?)
}

/// Block until the schedule settles the attempt numbered `sequence`.
///
/// The retained run window is searched rather than only the newest run, so a
/// caller that polls slightly late still reads the exact attempt it asked for.
pub async fn wait_for_run(
    schedule: &ScheduledSemanticRefresh,
    sequence: u64,
) -> Result<SemanticRefreshRunMetrics> {
    tokio::time::timeout(RUN_TIMEOUT, async {
        loop {
            let metrics = schedule.metrics();
            if metrics.attempted_runs() >= sequence {
                return metrics
                    .recent_runs()
                    .iter()
                    .find(|run| run.sequence() == sequence)
                    .cloned()
                    .with_context(|| {
                        format!("refresh sequence {sequence} fell out of the retained window")
                    });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| format!("refresh sequence {sequence} exceeded {RUN_TIMEOUT:?}"))?
}

/// Attempts the schedule has started so far, captured before a mutation lands.
pub fn attempted_runs(schedule: &ScheduledSemanticRefresh) -> Result<u64> {
    let attempted = schedule.metrics().attempted_runs();
    if attempted >= u64::MAX - 2 {
        bail!("refresh attempt counter is exhausted");
    }
    Ok(attempted)
}

/// Wait for the attempt that published the caller's mutation.
///
/// An attempt may already be in flight when the mutation lands, and that
/// attempt settles as `Unchanged` because it read the pre-mutation source. Any
/// source change forces a republication, so the first `Published` attempt after
/// `baseline` is exactly the run that observed the mutation.
pub async fn wait_for_published_run(
    schedule: &ScheduledSemanticRefresh,
    baseline: u64,
) -> Result<SemanticRefreshRunMetrics> {
    wait_for_outcome(
        schedule,
        baseline,
        SemanticRefreshRunOutcome::Published,
        "published",
    )
    .await
}

/// Wait for an attempt that started after `baseline` and found nothing to do.
///
/// The in-flight attempt is skipped so the settled `Unchanged` verdict describes
/// state the caller can reason about.
pub async fn wait_for_unchanged_run(
    schedule: &ScheduledSemanticRefresh,
    baseline: u64,
) -> Result<SemanticRefreshRunMetrics> {
    wait_for_outcome(
        schedule,
        baseline.saturating_add(1),
        SemanticRefreshRunOutcome::Unchanged,
        "unchanged",
    )
    .await
}

async fn wait_for_outcome(
    schedule: &ScheduledSemanticRefresh,
    after: u64,
    outcome: SemanticRefreshRunOutcome,
    description: &str,
) -> Result<SemanticRefreshRunMetrics> {
    tokio::time::timeout(RUN_TIMEOUT, async {
        loop {
            if let Some(run) = schedule
                .metrics()
                .recent_runs()
                .iter()
                .find(|run| run.sequence() > after && run.outcome() == outcome)
            {
                return run.clone();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .with_context(|| {
        format!("no {description} refresh attempt after sequence {after} within {RUN_TIMEOUT:?}")
    })
}
