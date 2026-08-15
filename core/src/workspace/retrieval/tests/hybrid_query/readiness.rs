use super::{
    populated_catalog, wait_until_ready, MemoryFiles, QualityProvider,
    WorkspaceHybridFallbackReason, WorkspaceHybridSearchRequest, WorkspaceRetrievalChannel,
    WorkspaceRetrievalOptions, WorkspaceRetrievalPhase, WorkspaceRetrievalRuntime,
};
use crate::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingProvider, EmbeddingProviderDescriptor,
    EmbeddingProviderError, EmbeddingVector,
};
use crate::workspace::WorkspaceRetrievalError;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn bounded_readiness_barrier_waits_for_the_current_semantic_generation() {
    let entries = &[("src/cache.rs", "pub fn release_ephemeral_projection() {}\n")];
    let mut quality = QualityProvider::from_documents(entries);
    quality
        .slots
        .insert("session projection cleanup".to_owned(), 0);
    let provider = Arc::new(BlockingDocumentProvider::new(quality));
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let runtime = WorkspaceRetrievalRuntime::start(
        populated_catalog(entries),
        WorkspaceRetrievalOptions::new(provider_port)
            .with_semantic_readiness_timeout(Duration::from_secs(1)),
        CancellationToken::new(),
    )
    .unwrap();
    provider.wait_until_document_started().await;

    let search = runtime.hybrid_search(
        WorkspaceHybridSearchRequest::new("session projection cleanup"),
        MemoryFiles::from_entries(entries),
        None,
        None,
        CancellationToken::new(),
    );
    tokio::pin!(search);
    tokio::select! {
        result = &mut search => panic!("readiness barrier returned before publication: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(20)) => {}
    }
    provider.release_document();
    let result = tokio::time::timeout(Duration::from_secs(2), &mut search)
        .await
        .expect("readiness barrier did not observe publication")
        .unwrap();

    assert_eq!(result.fallback, None);
    assert_eq!(result.semantic_status.phase, WorkspaceRetrievalPhase::Ready);
    let semantic = result
        .channels
        .iter()
        .find(|status| status.channel == WorkspaceRetrievalChannel::Semantic)
        .unwrap();
    assert_eq!(semantic.candidate_count, 1);
    assert!(result.hits[0]
        .channels
        .iter()
        .any(|rank| rank.channel == WorkspaceRetrievalChannel::Semantic));
    runtime.close().await;
}

#[tokio::test]
async fn readiness_timeout_preserves_the_existing_partial_fallback() {
    let entries = &[("src/cache.rs", "pub fn release_ephemeral_projection() {}\n")];
    let provider = Arc::new(BlockingDocumentProvider::new(
        QualityProvider::from_documents(entries),
    ));
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let runtime = WorkspaceRetrievalRuntime::start(
        populated_catalog(entries),
        WorkspaceRetrievalOptions::new(provider_port)
            .with_semantic_readiness_timeout(Duration::from_millis(20)),
        CancellationToken::new(),
    )
    .unwrap();
    provider.wait_until_document_started().await;

    let result = runtime
        .hybrid_search(
            WorkspaceHybridSearchRequest::new("unmatched semantic concept"),
            MemoryFiles::from_entries(entries),
            None,
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(result.hits.len(), 0);
    assert_eq!(
        result.fallback,
        Some(WorkspaceHybridFallbackReason::Building)
    );
    let semantic = result
        .channels
        .iter()
        .find(|status| status.channel == WorkspaceRetrievalChannel::Semantic)
        .unwrap();
    assert_eq!(semantic.candidate_count, 0);
    provider.release_document();
    wait_until_ready(&runtime).await;
    runtime.close().await;
}

#[tokio::test]
async fn caller_cancellation_interrupts_the_readiness_barrier() {
    let entries = &[("src/cache.rs", "pub fn release_ephemeral_projection() {}\n")];
    let provider = Arc::new(BlockingDocumentProvider::new(
        QualityProvider::from_documents(entries),
    ));
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let runtime = WorkspaceRetrievalRuntime::start(
        populated_catalog(entries),
        WorkspaceRetrievalOptions::new(provider_port)
            .with_semantic_readiness_timeout(Duration::from_secs(1)),
        CancellationToken::new(),
    )
    .unwrap();
    provider.wait_until_document_started().await;
    let cancellation = CancellationToken::new();
    let search = runtime.hybrid_search(
        WorkspaceHybridSearchRequest::new("unmatched semantic concept"),
        MemoryFiles::from_entries(entries),
        None,
        None,
        cancellation.clone(),
    );
    tokio::pin!(search);
    tokio::select! {
        result = &mut search => panic!("readiness barrier returned before cancellation: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(20)) => {}
    }
    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_millis(250), &mut search)
        .await
        .expect("readiness barrier ignored caller cancellation")
        .unwrap_err();
    assert!(matches!(error, WorkspaceRetrievalError::Cancelled));
    provider.release_document();
    wait_until_ready(&runtime).await;
    runtime.close().await;
}

#[test]
fn semantic_readiness_timeout_has_a_hard_upper_bound() {
    let entries = &[("src/cache.rs", "pub fn cached() {}\n")];
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(QualityProvider::from_documents(entries));
    let error = WorkspaceRetrievalRuntime::start(
        populated_catalog(entries),
        WorkspaceRetrievalOptions::new(provider)
            .with_semantic_readiness_timeout(Duration::from_secs(31)),
        CancellationToken::new(),
    )
    .unwrap_err();

    assert!(matches!(
        error,
        WorkspaceRetrievalError::InvalidConfiguration {
            field: "semantic_readiness_timeout",
            ..
        }
    ));
}

struct BlockingDocumentProvider {
    inner: QualityProvider,
    document_started: AtomicBool,
    started: Notify,
    gate: Semaphore,
}

impl BlockingDocumentProvider {
    fn new(inner: QualityProvider) -> Self {
        Self {
            inner,
            document_started: AtomicBool::new(false),
            started: Notify::new(),
            gate: Semaphore::new(0),
        }
    }

    async fn wait_until_document_started(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !self.document_started.load(Ordering::Acquire) {
                self.started.notified().await;
            }
        })
        .await
        .expect("document embedding did not start");
    }

    fn release_document(&self) {
        self.gate.add_permits(1);
    }
}

#[async_trait]
impl EmbeddingProvider for BlockingDocumentProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.inner.descriptor()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        let is_query = request
            .inputs()
            .iter()
            .any(|input| input.id() == "workspace-query");
        if !is_query {
            self.document_started.store(true, Ordering::Release);
            self.started.notify_waiters();
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => return Err(EmbeddingProviderError::Cancelled),
                permit = self.gate.acquire() => {
                    permit.expect("test semaphore is never closed").forget();
                }
            }
        }
        let vectors = request
            .inputs()
            .iter()
            .map(|input| EmbeddingVector::new(input.id(), self.inner.vector(input.text())))
            .collect();
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}
