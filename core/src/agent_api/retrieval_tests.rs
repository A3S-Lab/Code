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
    let search = session
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "search")
        .unwrap();
    assert_eq!(
        search.parameters["properties"]["mode"]["enum"],
        serde_json::json!(["grep", "glob", "bm25"])
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
async fn explicit_disable_clears_preconfigured_retrieval_without_calling_the_provider() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("source.rs"), "eligible source\n").unwrap();
    let provider = Arc::new(BlockingProvider::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let options = SessionOptions::new()
        .with_workspace_retrieval(WorkspaceRetrievalOptions::new(provider_port))
        .without_workspace_retrieval();
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();
    let session = agent
        .session_async(workspace.path().to_string_lossy(), Some(options))
        .await
        .unwrap();

    assert_eq!(
        session.workspace_retrieval_status().phase,
        WorkspaceRetrievalPhase::Disabled
    );
    let search = session
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "search")
        .unwrap();
    assert_eq!(
        search.parameters["properties"]["mode"]["enum"],
        serde_json::json!(["grep", "glob", "bm25"])
    );
    assert_eq!(provider.calls.load(Ordering::Acquire), 0);

    session.close().await;
    assert_eq!(provider.calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn enabled_session_exposes_and_executes_semantic_search() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("cache.rs"),
        "release temporary session vector memory\n",
    )
    .unwrap();
    // ASCII container headers are intentionally used here: extension admission,
    // not a coincidental UTF-8 decode failure, must keep these assets out.
    std::fs::write(workspace.path().join("architecture.pdf"), "%PDF-1.7\n").unwrap();
    std::fs::write(
        workspace.path().join("recording.mp3"),
        [b'I', b'D', b'3', 0xff, 0xfb, 0x90, 0x64],
    )
    .unwrap();
    let embedded_inputs = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(ImmediateProvider {
        inputs: Arc::clone(&embedded_inputs),
    });
    let options =
        SessionOptions::new().with_workspace_retrieval(WorkspaceRetrievalOptions::new(provider));
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();
    let session = agent
        .session_async(workspace.path().to_string_lossy(), Some(options))
        .await
        .unwrap();
    wait_for_ready(&session).await;
    let status = session.workspace_retrieval_status();
    assert_eq!(status.eligible_files, 1);
    assert_eq!(status.indexed_files, 1);
    assert_eq!(status.indexed_chunks, 1);
    assert_eq!(status.vector_records, 1);
    assert_eq!(embedded_inputs.load(Ordering::Acquire), 1);

    let search = session
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "search")
        .unwrap();
    assert_eq!(
        search.parameters["properties"]["mode"]["enum"],
        serde_json::json!(["grep", "glob", "bm25", "semantic", "hybrid"])
    );
    let result = session
        .tool(
            "search",
            serde_json::json!({
                "mode": "semantic",
                "query": "session cleanup",
                "include": "*.rs",
                "limit": 1
            }),
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("cache.rs:1-1"), "{}", result.output);
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["mode"], "semantic");
    assert_eq!(metadata["status"]["phase"], "ready");
    assert_eq!(metadata["results"][0]["digest_verified"], true);

    let structured = session
        .semantic_search(
            crate::WorkspaceSemanticSearchRequest::new("session cleanup").with_limit(1),
        )
        .await
        .unwrap();
    assert_eq!(structured.hits.len(), 1);
    assert_eq!(structured.hits[0].chunk.path.as_ref(), "cache.rs");

    let hybrid = session
        .tool(
            "search",
            serde_json::json!({
                "mode": "hybrid",
                "query": "session cleanup",
                "include": "*.rs",
                "limit": 1
            }),
        )
        .await
        .unwrap();
    assert_eq!(hybrid.exit_code, 0, "{}", hybrid.output);
    assert!(hybrid.output.contains("cache.rs:1-1"), "{}", hybrid.output);
    let metadata = hybrid.metadata.unwrap();
    assert_eq!(metadata["mode"], "hybrid");
    assert_eq!(metadata["algorithm"], "rrf_k60");
    assert_eq!(metadata["results"][0]["digest_verified"], true);

    let structured_hybrid = session
        .hybrid_search(crate::WorkspaceHybridSearchRequest::new("session cleanup").with_limit(1))
        .await
        .unwrap();
    assert_eq!(structured_hybrid.hits.len(), 1);
    assert_eq!(structured_hybrid.hits[0].chunk.path.as_ref(), "cache.rs");
    assert_eq!(structured_hybrid.channels.len(), 4);

    let invalid = session
        .semantic_search(
            crate::WorkspaceSemanticSearchRequest::new("session cleanup").with_path("../escape"),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        invalid,
        crate::WorkspaceRetrievalError::InvalidQuery(_)
    ));

    session.close().await;
    let closed = session
        .semantic_search(crate::WorkspaceSemanticSearchRequest::new(
            "session cleanup",
        ))
        .await
        .unwrap_err();
    assert!(matches!(
        closed,
        crate::WorkspaceRetrievalError::Unavailable
    ));
    let closed = session
        .hybrid_search(crate::WorkspaceHybridSearchRequest::new("session cleanup"))
        .await
        .unwrap_err();
    assert!(matches!(
        closed,
        crate::WorkspaceRetrievalError::Unavailable
    ));
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
async fn session_chunking_options_cannot_override_a_host_owned_catalog() {
    let backend = Arc::new(EmptyWorkspace);
    let catalog = crate::WorkspaceChunkCatalog::new(
        crate::ChunkingConfig::default(),
        crate::ChunkCatalogLimits::default(),
    )
    .unwrap();
    let services =
        WorkspaceServices::builder(WorkspaceRef::new("remote", "remote://workspace"), backend)
            .chunk_catalog(catalog)
            .build();
    let provider = Arc::new(BlockingProvider::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let strategy = crate::WorkspaceChunkingStrategy::FixedWindow(
        crate::FixedWindowChunkingOptions::new(512, 64).unwrap(),
    );
    let options = SessionOptions::new()
        .with_workspace_backend(services)
        .with_workspace_retrieval(
            WorkspaceRetrievalOptions::new(provider_port).with_chunking_strategy(strategy),
        );
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
async fn custom_workspace_without_read_capability_is_rejected() {
    let backend = Arc::new(EmptyWorkspace);
    let catalog = crate::WorkspaceChunkCatalog::new(
        crate::ChunkingConfig::default(),
        crate::ChunkCatalogLimits::default(),
    )
    .unwrap();
    let services = WorkspaceServices::builder(
        WorkspaceRef::new("write-only", "remote://write-only"),
        backend,
    )
    .capabilities(crate::WorkspaceCapabilities {
        read: false,
        write: true,
        exec: false,
        search: false,
        git: false,
        code_intelligence: false,
    })
    .chunk_catalog(catalog)
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

struct ImmediateProvider {
    inputs: Arc<AtomicUsize>,
}

#[tokio::test]
async fn session_owned_workspace_uses_the_explicit_chunking_strategy() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("window.txt"), "abcdefghij").unwrap();
    let embedded_inputs = Arc::new(AtomicUsize::new(0));
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(ImmediateProvider {
        inputs: Arc::clone(&embedded_inputs),
    });
    let strategy = crate::WorkspaceChunkingStrategy::FixedWindow(
        crate::FixedWindowChunkingOptions::new(4, 1).unwrap(),
    );
    let options = SessionOptions::new().with_workspace_retrieval(
        WorkspaceRetrievalOptions::new(provider).with_chunking_strategy(strategy),
    );
    let agent = Agent::from_config(super::tests::test_config())
        .await
        .unwrap();
    let session = agent
        .session_async(workspace.path().to_string_lossy(), Some(options))
        .await
        .unwrap();
    wait_for_ready(&session).await;

    let status = session.workspace_retrieval_status();
    assert_eq!(status.indexed_files, 1);
    assert_eq!(status.indexed_chunks, 3);
    assert_eq!(status.vector_records, 3);
    assert_eq!(embedded_inputs.load(Ordering::Acquire), 3);
    session.close().await;
}

#[async_trait]
impl EmbeddingProvider for ImmediateProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        EmbeddingProviderDescriptor::new("fixture", "immediate-v1", 2)
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        _cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        self.inputs
            .fetch_add(request.inputs().len(), Ordering::AcqRel);
        let vectors = request
            .inputs()
            .iter()
            .map(|input| EmbeddingVector::new(input.id(), vec![1.0, 0.0]))
            .collect();
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

async fn wait_for_ready(session: &super::AgentSession) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while session.workspace_retrieval_status().phase != WorkspaceRetrievalPhase::Ready {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("semantic session did not become ready");
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
