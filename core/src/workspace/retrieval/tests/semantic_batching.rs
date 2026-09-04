use super::super::{
    ChunkCatalogLimits, ChunkingConfig, WorkspaceChunkCatalog, WorkspaceRetrievalOptions,
    WorkspaceRetrievalPhase, WorkspaceRetrievalRuntime, WorkspaceRetrievalStatus,
};
use crate::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingExecutorConfig, EmbeddingProvider,
    EmbeddingProviderDescriptor, EmbeddingProviderError, EmbeddingVector,
};
use crate::workspace::WorkspacePath;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

mod adversarial;

#[test]
fn additive_batching_metrics_preserve_status_json_compatibility() {
    let mut legacy = serde_json::to_value(WorkspaceRetrievalStatus::disabled()).unwrap();
    let legacy = legacy.as_object_mut().unwrap();
    legacy.remove("batching");

    let status: WorkspaceRetrievalStatus =
        serde_json::from_value(serde_json::Value::Object(legacy.clone())).unwrap();

    assert_eq!(status.batching, Default::default());
}

#[tokio::test]
async fn coalesces_small_files_to_the_batch_limit_lower_bound() {
    let files = (0..30)
        .map(|index| {
            (
                format!("src/file_{index:02}.rs"),
                format!("pub fn file_{index:02}() {{}}\n"),
            )
        })
        .collect::<Vec<_>>();
    let catalog = populated_catalog(&files, ChunkingConfig::default());
    let provider = RecordingProvider::immediate();
    let runtime = start_runtime(
        catalog,
        provider.clone(),
        EmbeddingExecutorConfig::default(),
    );

    let ready = wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Ready
    })
    .await;

    assert_eq!(ready.indexed_files, 30);
    assert_eq!(ready.indexed_chunks, 30);
    assert_eq!(provider.call_count(), 1);
    assert_eq!(provider.batch_sizes(), vec![30]);
    assert_eq!(ready.batching.document_inputs, 30);
    assert_eq!(ready.batching.document_batches, 1);
    assert_eq!(ready.batching.document_provider_requests, 1);
    assert_eq!(ready.batching.batch_limit_lower_bound, 1);
    assert_eq!(ready.batching.generation_complete_flushes, 1);
    assert_eq!(ready.batching.non_text_inputs, 0);
    assert!(ready.batching.time_to_first_ready_ms.is_some());
    runtime.close().await;
}

#[tokio::test]
async fn flushes_on_the_input_limit_and_keeps_provider_batches_bounded() {
    let files = (0..5)
        .map(|index| {
            (
                format!("src/file_{index}.rs"),
                format!("pub fn file_{index}() {{}}\n"),
            )
        })
        .collect::<Vec<_>>();
    let catalog = populated_catalog(&files, ChunkingConfig::default());
    let provider = RecordingProvider::immediate();
    let embedding = EmbeddingExecutorConfig {
        max_batch_inputs: 2,
        ..EmbeddingExecutorConfig::default()
    };
    let runtime = start_runtime(catalog, provider.clone(), embedding);

    let ready = wait_for_status(&runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Ready
    })
    .await;

    assert_eq!(provider.call_count(), 3);
    assert_eq!(provider.batch_sizes(), vec![2, 2, 1]);
    assert_eq!(ready.batching.document_inputs, 5);
    assert_eq!(ready.batching.document_batches, 3);
    assert_eq!(ready.batching.document_provider_requests, 3);
    assert_eq!(ready.batching.batch_limit_lower_bound, 3);
    assert_eq!(ready.batching.input_limit_flushes, 2);
    assert_eq!(ready.batching.generation_complete_flushes, 1);
    runtime.close().await;
}

#[tokio::test]
async fn flush_metrics_distinguish_text_and_vector_limits() {
    let files = (0..5)
        .map(|index| (format!("{index}.txt"), "abcd\n".to_owned()))
        .collect::<Vec<_>>();

    let text_provider = RecordingProvider::immediate();
    let text_runtime = start_runtime(
        populated_catalog(&files, ChunkingConfig::default()),
        text_provider.clone(),
        EmbeddingExecutorConfig {
            max_batch_text_bytes: 10,
            max_input_text_bytes: 10,
            ..EmbeddingExecutorConfig::default()
        },
    );
    let text_ready = wait_for_status(&text_runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Ready
    })
    .await;
    assert_eq!(text_provider.batch_sizes(), vec![2, 2, 1]);
    assert_eq!(text_ready.batching.text_byte_limit_flushes, 2);
    assert_eq!(text_ready.batching.batch_limit_lower_bound, 3);
    text_runtime.close().await;

    let vector_provider = RecordingProvider::immediate();
    let vector_runtime = start_runtime(
        populated_catalog(&files, ChunkingConfig::default()),
        vector_provider.clone(),
        EmbeddingExecutorConfig {
            max_batch_vector_bytes: 16,
            ..EmbeddingExecutorConfig::default()
        },
    );
    let vector_ready = wait_for_status(&vector_runtime, |status| {
        status.phase == WorkspaceRetrievalPhase::Ready
    })
    .await;
    assert_eq!(vector_provider.batch_sizes(), vec![2, 2, 1]);
    assert_eq!(vector_ready.batching.vector_byte_limit_flushes, 2);
    assert_eq!(vector_ready.batching.batch_limit_lower_bound, 3);
    vector_runtime.close().await;
}

fn populated_catalog(
    files: &[(String, String)],
    chunking: ChunkingConfig,
) -> Arc<WorkspaceChunkCatalog> {
    let catalog = WorkspaceChunkCatalog::new(chunking, ChunkCatalogLimits::default()).unwrap();
    for (path, content) in files {
        catalog
            .replace_file(
                &WorkspacePath::from_normalized(path),
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
    provider: Arc<RecordingProvider>,
    embedding: EmbeddingExecutorConfig,
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

struct RecordingProvider {
    descriptor: EmbeddingProviderDescriptor,
    calls: AtomicUsize,
    batch_sizes: Mutex<Vec<usize>>,
    request_ids: Mutex<Vec<Vec<String>>>,
    cancellation_tokens: Mutex<Vec<CancellationToken>>,
    failures: Vec<usize>,
    malformed_responses: Vec<usize>,
    gate: Option<Semaphore>,
    called: Notify,
}

impl RecordingProvider {
    fn immediate() -> Arc<Self> {
        Self::with_failures(&[])
    }

    fn with_failures(failures: &[usize]) -> Arc<Self> {
        Self::with_outcomes(failures, &[])
    }

    fn with_malformed_responses(malformed_responses: &[usize]) -> Arc<Self> {
        Self::with_outcomes(&[], malformed_responses)
    }

    fn with_outcomes(failures: &[usize], malformed_responses: &[usize]) -> Arc<Self> {
        Arc::new(Self {
            descriptor: EmbeddingProviderDescriptor::new("fixture", "batch-v1", 2),
            calls: AtomicUsize::new(0),
            batch_sizes: Mutex::new(Vec::new()),
            request_ids: Mutex::new(Vec::new()),
            cancellation_tokens: Mutex::new(Vec::new()),
            failures: failures.to_vec(),
            malformed_responses: malformed_responses.to_vec(),
            gate: None,
            called: Notify::new(),
        })
    }

    fn gated() -> Arc<Self> {
        Arc::new(Self {
            descriptor: EmbeddingProviderDescriptor::new("fixture", "batch-v1", 2),
            calls: AtomicUsize::new(0),
            batch_sizes: Mutex::new(Vec::new()),
            request_ids: Mutex::new(Vec::new()),
            cancellation_tokens: Mutex::new(Vec::new()),
            failures: Vec::new(),
            malformed_responses: Vec::new(),
            gate: Some(Semaphore::new(0)),
            called: Notify::new(),
        })
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }

    fn batch_sizes(&self) -> Vec<usize> {
        self.batch_sizes.lock().unwrap().clone()
    }

    fn request_ids(&self) -> Vec<Vec<String>> {
        self.request_ids.lock().unwrap().clone()
    }

    fn request_was_cancelled(&self, index: usize) -> bool {
        self.cancellation_tokens
            .lock()
            .unwrap()
            .get(index)
            .is_some_and(CancellationToken::is_cancelled)
    }

    fn release(&self, permits: usize) {
        self.gate.as_ref().unwrap().add_permits(permits);
    }

    async fn wait_for_calls(&self, expected: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.call_count() < expected {
                self.called.notified().await;
            }
        })
        .await
        .expect("embedding provider was not called");
    }
}

#[async_trait]
impl EmbeddingProvider for RecordingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.descriptor.clone()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        let call = self.calls.fetch_add(1, Ordering::AcqRel);
        self.batch_sizes
            .lock()
            .unwrap()
            .push(request.inputs().len());
        self.request_ids.lock().unwrap().push(
            request
                .inputs()
                .iter()
                .map(|input| input.id().to_owned())
                .collect(),
        );
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
        if self.failures.contains(&call) {
            return Err(EmbeddingProviderError::Unavailable { retry_after: None });
        }
        let mut vectors = request
            .inputs()
            .iter()
            .map(|input| EmbeddingVector::new(input.id(), vec![1.0, 1.0]))
            .collect::<Vec<_>>();
        if self.malformed_responses.contains(&call) {
            vectors.pop();
        }
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}
