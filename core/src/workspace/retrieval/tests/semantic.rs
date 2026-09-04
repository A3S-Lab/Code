use super::super::{
    ChunkCatalogLimits, ChunkingConfig, WorkspaceChunkCatalog, WorkspaceRetrievalOptions,
    WorkspaceRetrievalPhase, WorkspaceRetrievalRuntime, WorkspaceRetrievalStatus,
};
use crate::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingProvider, EmbeddingProviderDescriptor,
    EmbeddingProviderError, EmbeddingVector,
};
use crate::workspace::WorkspacePath;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn builds_file_partitions_asynchronously_and_exposes_partial_readiness() {
    let catalog = populated_catalog(&[
        ("a.rs", "session cache invalidation\n"),
        ("b.rs", "credential expiry guard\n"),
    ]);
    let provider = ControlledProvider::gated(vec![ProviderOutcome::Success; 2]);
    let runtime = start_runtime_with_embedding(
        Arc::clone(&catalog),
        provider.clone(),
        crate::EmbeddingExecutorConfig {
            max_batch_inputs: 1,
            ..Default::default()
        },
    );

    provider.wait_for_calls(1).await;
    let initial = runtime.status();
    assert_eq!(initial.phase, WorkspaceRetrievalPhase::Building);
    assert_eq!(initial.indexed_files, 0);
    assert_eq!(initial.queue_depth, 2);

    provider.release(1);
    wait_for_status(&runtime, |status| status.indexed_files == 1).await;
    let partial = runtime.status();
    assert_eq!(partial.phase, WorkspaceRetrievalPhase::Building);
    assert_eq!(partial.coverage_bps, 5_000);
    assert_eq!(partial.vector_records, 1);
    assert_eq!(partial.queue_depth, 1);

    provider.release(1);
    let ready = wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Ready
    })
    .await;
    assert_eq!(ready.indexed_files, 2);
    assert_eq!(ready.indexed_chunks, 2);
    assert_eq!(ready.coverage_bps, 10_000);
    assert_eq!(ready.vector_records, 2);
    runtime.close().await;
    assert_eq!(runtime.status().phase, WorkspaceRetrievalPhase::Closed);
}

#[tokio::test]
async fn invalidates_a_changed_partition_before_reembedding_it() {
    let catalog = populated_catalog(&[("a.rs", "old token\n"), ("b.rs", "stable token\n")]);
    let provider = ControlledProvider::gated(vec![ProviderOutcome::Success; 3]);
    let runtime = start_runtime_with_embedding(
        Arc::clone(&catalog),
        provider.clone(),
        crate::EmbeddingExecutorConfig {
            max_batch_inputs: 1,
            ..Default::default()
        },
    );
    provider.release(2);
    wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Ready
    })
    .await;

    catalog
        .replace_file(
            &WorkspacePath::from_normalized("a.rs"),
            Some("rust"),
            2,
            "replacement token\n",
        )
        .unwrap();
    provider.wait_for_calls(3).await;
    let invalidated = wait_for_status(&runtime, |status| {
        status.source_revision == 2 && status.indexed_files == 1
    })
    .await;
    assert_eq!(invalidated.vector_records, 1);
    assert_eq!(invalidated.phase, WorkspaceRetrievalPhase::Building);

    provider.release(1);
    let rebuilt = wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Ready && status.source_revision == 2
    })
    .await;
    assert_eq!(rebuilt.vector_records, 2);
    runtime.close().await;
}

#[tokio::test]
async fn update_during_embedding_cannot_publish_the_superseded_vector() {
    let catalog = populated_catalog(&[("a.rs", "generation one\n")]);
    let provider = ControlledProvider::gated(vec![ProviderOutcome::Success; 2]);
    let runtime = start_runtime(Arc::clone(&catalog), provider.clone());
    provider.wait_for_calls(1).await;

    catalog
        .replace_file(
            &WorkspacePath::from_normalized("a.rs"),
            Some("rust"),
            2,
            "generation two\n",
        )
        .unwrap();
    provider.wait_for_calls(2).await;
    assert!(provider.request_was_cancelled(0));
    let during_replacement = runtime.status();
    assert_eq!(during_replacement.source_revision, 2);
    assert_eq!(during_replacement.indexed_files, 0);
    assert_eq!(during_replacement.vector_records, 0);

    provider.release(1);
    let ready = wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Ready && status.source_revision == 2
    })
    .await;
    assert_eq!(ready.vector_records, 1);
    runtime.close().await;
}

#[tokio::test]
async fn a_failed_file_degrades_only_semantic_coverage() {
    let catalog = populated_catalog(&[("a.rs", "first\n"), ("b.rs", "second\n")]);
    let provider =
        ControlledProvider::immediate(vec![ProviderOutcome::Unavailable, ProviderOutcome::Success]);
    let mut options = WorkspaceRetrievalOptions::new(provider.clone());
    let embedding = crate::EmbeddingExecutorConfig {
        max_batch_inputs: 1,
        max_retries: 0,
        ..Default::default()
    };
    options = options.with_embedding_config(embedding);
    let runtime =
        WorkspaceRetrievalRuntime::start(catalog, options, CancellationToken::new()).unwrap();

    let degraded = wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Degraded && status.queue_depth == 0
    })
    .await;
    assert_eq!(degraded.eligible_files, 2);
    assert_eq!(degraded.indexed_files, 1);
    assert_eq!(degraded.failed_files, 1);
    assert_eq!(degraded.total_failures, 1);
    assert_eq!(degraded.coverage_bps, 5_000);
    runtime.close().await;
}

#[tokio::test]
async fn close_cancels_embedding_joins_the_task_and_releases_vector_memory() {
    let catalog = populated_catalog(&[("a.rs", "blocked embedding\n")]);
    let provider = ControlledProvider::gated(vec![ProviderOutcome::Success]);
    let runtime = start_runtime(catalog, provider.clone());
    provider.wait_for_calls(1).await;
    let index = runtime.index().unwrap();
    let weak_index = Arc::downgrade(&index);
    drop(index);

    tokio::time::timeout(Duration::from_secs(1), runtime.close())
        .await
        .expect("semantic close exceeded its bounded cancellation path");
    assert!(provider.request_was_cancelled(0));
    assert!(weak_index.upgrade().is_none());
    assert!(runtime.index().is_none());
    assert_eq!(runtime.status().phase, WorkspaceRetrievalPhase::Closed);
    assert_eq!(runtime.status().vector_bytes, 0);

    runtime.close().await;
    assert_eq!(runtime.status().phase, WorkspaceRetrievalPhase::Closed);
}

fn populated_catalog(files: &[(&str, &str)]) -> Arc<WorkspaceChunkCatalog> {
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .unwrap();
    for (path, content) in files {
        catalog
            .replace_file(
                &WorkspacePath::from_normalized(*path),
                path.ends_with(".rs").then_some("rust"),
                1,
                content,
            )
            .unwrap();
    }
    catalog
}

fn start_runtime(
    catalog: Arc<WorkspaceChunkCatalog>,
    provider: Arc<ControlledProvider>,
) -> Arc<WorkspaceRetrievalRuntime> {
    start_runtime_with_embedding(catalog, provider, crate::EmbeddingExecutorConfig::default())
}

fn start_runtime_with_embedding(
    catalog: Arc<WorkspaceChunkCatalog>,
    provider: Arc<ControlledProvider>,
    embedding: crate::EmbeddingExecutorConfig,
) -> Arc<WorkspaceRetrievalRuntime> {
    let provider: Arc<dyn EmbeddingProvider> = provider;
    WorkspaceRetrievalRuntime::start(
        catalog,
        WorkspaceRetrievalOptions::new(provider).with_embedding_config(embedding),
        CancellationToken::new(),
    )
    .unwrap()
}

async fn wait_for_status(
    runtime: &WorkspaceRetrievalRuntime,
    predicate: impl Fn(&WorkspaceRetrievalStatus) -> bool,
) -> WorkspaceRetrievalStatus {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let status = runtime.status();
            if predicate(&status) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("semantic status did not converge")
}

#[derive(Clone, Copy)]
enum ProviderOutcome {
    Success,
    Unavailable,
}

struct ControlledProvider {
    descriptor: EmbeddingProviderDescriptor,
    outcomes: Mutex<VecDeque<ProviderOutcome>>,
    gate: Option<Semaphore>,
    calls: AtomicUsize,
    cancellation_tokens: Mutex<Vec<CancellationToken>>,
    called: Notify,
}

impl ControlledProvider {
    fn gated(outcomes: Vec<ProviderOutcome>) -> Arc<Self> {
        Arc::new(Self::new(outcomes, Some(Semaphore::new(0))))
    }

    fn immediate(outcomes: Vec<ProviderOutcome>) -> Arc<Self> {
        Arc::new(Self::new(outcomes, None))
    }

    fn new(outcomes: Vec<ProviderOutcome>, gate: Option<Semaphore>) -> Self {
        Self {
            descriptor: EmbeddingProviderDescriptor::new("fixture", "semantic-v1", 2),
            outcomes: Mutex::new(outcomes.into()),
            gate,
            calls: AtomicUsize::new(0),
            cancellation_tokens: Mutex::new(Vec::new()),
            called: Notify::new(),
        }
    }

    fn release(&self, permits: usize) {
        self.gate.as_ref().unwrap().add_permits(permits);
    }

    async fn wait_for_calls(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.calls.load(Ordering::Acquire) < expected {
                self.called.notified().await;
            }
        })
        .await
        .expect("embedding provider was not called");
    }

    fn request_was_cancelled(&self, index: usize) -> bool {
        self.cancellation_tokens
            .lock()
            .unwrap()
            .get(index)
            .is_some_and(CancellationToken::is_cancelled)
    }
}

#[async_trait]
impl EmbeddingProvider for ControlledProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.descriptor.clone()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.cancellation_tokens
            .lock()
            .unwrap()
            .push(cancellation.clone());
        self.called.notify_waiters();
        if let Some(gate) = &self.gate {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    return Err(EmbeddingProviderError::Cancelled);
                }
                permit = gate.acquire() => {
                    permit.expect("test semaphore is never closed").forget();
                }
            }
        }
        let outcome = self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(ProviderOutcome::Success);
        match outcome {
            ProviderOutcome::Success => Ok(EmbeddingBatchResponse::new(
                self.descriptor.clone(),
                request
                    .inputs()
                    .iter()
                    .map(|input| EmbeddingVector::new(input.id(), vec![1.0, 1.0]))
                    .collect(),
            )),
            ProviderOutcome::Unavailable => {
                Err(EmbeddingProviderError::Unavailable { retry_after: None })
            }
        }
    }
}
