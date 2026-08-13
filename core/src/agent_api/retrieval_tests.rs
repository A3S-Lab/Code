use super::{Agent, SessionOptions};
use crate::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingProvider, EmbeddingProviderDescriptor,
    EmbeddingProviderError, EmbeddingVector,
};
use crate::workspace::{
    WorkspaceDirEntry, WorkspaceFileSystem, WorkspacePath, WorkspaceRef, WorkspaceResult,
    WorkspaceRetrievalOptions, WorkspaceRetrievalPhase, WorkspaceServices, WorkspaceWriteOutcome,
};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn retrieval_is_disabled_without_explicit_typed_options() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();
    let session = agent
        .session_async(workspace.path().to_string_lossy(), None)
        .await
        .unwrap();

    assert_eq!(
        session.workspace_retrieval_status().phase,
        WorkspaceRetrievalPhase::Disabled
    );
    session.close().await;
    // Disabled means there is no runtime to transition; it remains a stable
    // capability observation rather than pretending an index existed.
    assert_eq!(
        session.workspace_retrieval_status().phase,
        WorkspaceRetrievalPhase::Disabled
    );
}

#[tokio::test]
async fn async_session_creation_does_not_wait_for_workspace_embeddings() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("lib.rs"),
        "pub fn semantic_probe() {}\n",
    )
    .unwrap();
    let provider = Arc::new(BlockingProvider::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let options = SessionOptions::new()
        .with_workspace_retrieval(WorkspaceRetrievalOptions::new(provider_port));
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();

    let started = Instant::now();
    let session = tokio::time::timeout(
        Duration::from_secs(1),
        agent.session_async(workspace.path().to_string_lossy(), Some(options)),
    )
    .await
    .expect("session construction waited for embedding")
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        session.workspace_retrieval_status().phase,
        WorkspaceRetrievalPhase::Building
    );

    provider.wait_for_calls(1).await;
    session.close().await;
    assert_eq!(
        session.workspace_retrieval_status().phase,
        WorkspaceRetrievalPhase::Closed
    );
    assert!(provider.request_was_cancelled());
}

#[tokio::test]
async fn sync_session_rejects_semantic_retrieval_as_an_async_resource() {
    let workspace = tempfile::tempdir().unwrap();
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(BlockingProvider::new());
    let options = SessionOptions::new()
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_workspace_retrieval(WorkspaceRetrievalOptions::new(provider));
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();

    let error = agent
        .session(workspace.path().to_string_lossy(), Some(options))
        .unwrap_err();
    assert!(matches!(
        error,
        crate::CodeError::AsyncSessionBuildRequired {
            resource: crate::SessionBuildResource::WorkspaceRetrieval
        }
    ));
}

#[tokio::test]
async fn custom_workspace_without_a_catalog_is_rejected_before_provider_calls() {
    let backend = Arc::new(EmptyWorkspace);
    let services =
        WorkspaceServices::builder(WorkspaceRef::new("remote", "remote://workspace"), backend)
            .build();
    let provider = Arc::new(BlockingProvider::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let options = SessionOptions::new()
        .with_workspace_backend(services)
        .with_workspace_retrieval(WorkspaceRetrievalOptions::new(provider_port));
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();

    let error = agent
        .session_async("remote-placeholder", Some(options))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::CodeError::SessionConfiguration {
            field: "workspace_retrieval",
            ..
        }
    ));
    assert_eq!(provider.calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn a_later_session_build_failure_cleans_up_started_retrieval_work() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("lib.rs"),
        "pub fn cleanup_probe() {}\n",
    )
    .unwrap();
    let provider = Arc::new(BlockingProvider::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let queue = crate::queue::SessionQueueConfig {
        query_max_concurrency: 0,
        ..Default::default()
    };
    let options = SessionOptions::new()
        .with_workspace_retrieval(WorkspaceRetrievalOptions::new(provider_port))
        .with_queue_config(queue);
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();

    let error = agent
        .session_async(workspace.path().to_string_lossy(), Some(options))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::CodeError::SessionInitialization {
            resource: crate::SessionBuildResource::Queue,
            ..
        }
    ));
    if provider.calls.load(Ordering::Acquire) > 0 {
        assert!(provider.request_was_cancelled());
    }
}

struct BlockingProvider {
    calls: AtomicUsize,
    called: Notify,
    cancellation: std::sync::Mutex<Option<CancellationToken>>,
}

impl BlockingProvider {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            called: Notify::new(),
            cancellation: std::sync::Mutex::new(None),
        }
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

    fn request_was_cancelled(&self) -> bool {
        self.cancellation
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }
}

#[async_trait]
impl EmbeddingProvider for BlockingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new("fixture", "blocked-v1", 2)
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        *self.cancellation.lock().unwrap() = Some(cancellation.clone());
        self.called.notify_waiters();
        cancellation.cancelled().await;
        let vectors = request
            .inputs()
            .iter()
            .map(|input| EmbeddingVector::new(input.id(), vec![1.0, 1.0]))
            .collect();
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

struct EmptyWorkspace;

#[async_trait]
impl WorkspaceFileSystem for EmptyWorkspace {
    async fn read_text(&self, path: &WorkspacePath) -> WorkspaceResult<String> {
        Err(crate::workspace::WorkspaceError::NotFound {
            path: path.as_str().to_owned(),
        })
    }

    async fn write_text(
        &self,
        _path: &WorkspacePath,
        content: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome> {
        Ok(WorkspaceWriteOutcome {
            bytes: content.len(),
            lines: content.lines().count(),
        })
    }

    async fn list_dir(&self, _path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
        Ok(Vec::new())
    }
}
