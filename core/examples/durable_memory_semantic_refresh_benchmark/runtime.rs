use super::fixture::{QualificationProvider, TARGET_INDEX, TARGET_QUERY};
use super::measurement::{ExpectedBytes, ExpectedWork};
use super::{CANDIDATE_LIMIT, CORPUS_RECORDS, EMBEDDING_BATCH_INPUTS, RUN_TIMEOUT};
use a3s_code_core::embedding::{EmbeddingExecutorConfig, EmbeddingProvider};
use a3s_code_core::memory::{
    AgentMemory, MemoryConfig, MemoryMaintenanceOptions, MemoryMaintenanceRuntime,
    ScheduledSemanticRefresh, SemanticRefreshMetrics, SemanticRefreshRunMetrics,
    SemanticRefreshRunOutcome,
};
use a3s_code_core::{
    DurableMemoryRecallChannel, DurableMemoryRecallPolicy, DurableMemorySemanticRecall,
    DurableMemorySemanticRecallPolicy, DurableMemorySession,
};
use a3s_memory::repository::{FileMemoryRepository, MemoryNamespace, MemoryRepository};
use a3s_memory::vector::{SqliteVectorIndex, VectorIndex};
use a3s_memory::InMemoryStore;
use anyhow::{bail, Context, Result};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

pub fn durable_session(
    repository: Arc<FileMemoryRepository>,
    namespace: MemoryNamespace,
    provider: Arc<QualificationProvider>,
    index: Arc<SqliteVectorIndex>,
) -> Result<DurableMemorySession> {
    let repository: Arc<dyn MemoryRepository> = repository;
    let provider: Arc<dyn EmbeddingProvider> = provider;
    let index: Arc<dyn VectorIndex> = index;
    let semantic = DurableMemorySemanticRecall::new(
        format!("sha256:{}", "a".repeat(64)),
        provider,
        EmbeddingExecutorConfig {
            max_batch_inputs: EMBEDDING_BATCH_INPUTS,
            max_request_inputs: CORPUS_RECORDS,
            ..EmbeddingExecutorConfig::default()
        },
        index,
        DurableMemorySemanticRecallPolicy::try_new(CANDIDATE_LIMIT, 0.7)?,
    )?;
    Ok(DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(CANDIDATE_LIMIT, 1.0)?,
    )
    .with_semantic_recall(semantic)?)
}

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

pub async fn wait_for_run(
    schedule: &ScheduledSemanticRefresh,
    sequence: u64,
) -> Result<SemanticRefreshRunMetrics> {
    tokio::time::timeout(RUN_TIMEOUT, async {
        loop {
            let metrics = schedule.metrics();
            if metrics.attempted_runs() >= sequence {
                let run = metrics
                    .last_run()
                    .cloned()
                    .context("settled refresh attempt had no retained run metrics")?;
                if run.sequence() != sequence {
                    bail!(
                        "refresh advanced to sequence {} before sequence {sequence} was observed",
                        run.sequence()
                    );
                }
                return Ok(run);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .with_context(|| format!("refresh sequence {sequence} exceeded {RUN_TIMEOUT:?}"))?
}

pub async fn validate_recall(durable: &DurableMemorySession) -> Result<()> {
    let preview = durable.preview_recall(TARGET_QUERY).await?;
    let hit = preview
        .hits
        .first()
        .context("semantic recall returned no target")?;
    if hit.node_id != format!("memory-{TARGET_INDEX:05}")
        || hit.channel != DurableMemoryRecallChannel::Semantic
        || preview.hits.len() > CANDIDATE_LIMIT
    {
        bail!("semantic recall did not return the independently queried target first");
    }
    Ok(())
}

pub async fn persist_checkpoint(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .await
        .context("could not create checkpoint file")?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    Ok(())
}

pub fn epoch_summary_matches(
    metrics: &SemanticRefreshMetrics,
    attempted: u64,
    published: u64,
    unchanged: u64,
) -> bool {
    metrics.ownership_epoch() == 1
        && metrics.attempted_runs() == attempted
        && metrics.published_runs() == published
        && metrics.unchanged_runs() == unchanged
        && metrics.failed_runs() == 0
        && metrics.recent_runs().len() == usize::try_from(attempted).unwrap_or(usize::MAX)
}

#[allow(clippy::too_many_arguments)]
pub const fn expected_work(
    sequence: u64,
    outcome: SemanticRefreshRunOutcome,
    source_change_token_requests: u64,
    source_snapshot_requests: u64,
    source_snapshot_node_reads: u64,
    source_snapshot_bytes: ExpectedBytes,
    embedding_cache_hits: u64,
    embedding_inputs: u64,
    embedding_input_bytes: u64,
    provider_requests: u64,
    provider_inputs: u64,
    provider_input_bytes: u64,
    publication_attempts: u64,
    publication_records: u64,
) -> ExpectedWork {
    ExpectedWork {
        sequence,
        outcome,
        source_change_token_requests,
        source_snapshot_requests,
        source_snapshot_node_reads,
        source_snapshot_bytes,
        embedding_cache_hits,
        embedding_inputs,
        embedding_input_bytes,
        provider_requests,
        provider_inputs,
        provider_input_bytes,
        publication_attempts,
        publication_records,
    }
}

pub fn file_tree_stats(path: &Path) -> Result<(usize, u64)> {
    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current)
            .with_context(|| format!("could not inspect {}", current.display()))?
        {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                files = files.saturating_add(1);
                bytes = bytes.saturating_add(metadata.len());
            }
        }
    }
    Ok((files, bytes))
}
