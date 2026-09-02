use super::super::{
    ChunkCatalogLimits, ChunkingConfig, WorkspaceChunkCatalog, WorkspaceRetrievalOptions,
    WorkspaceRetrievalPhase, WorkspaceRetrievalRuntime, WorkspaceSemanticFallbackReason,
    WorkspaceSemanticSearchRequest, WorkspaceVecShadowPhase,
};
use crate::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingProvider, EmbeddingProviderDescriptor,
    EmbeddingProviderError, EmbeddingVector,
};
use crate::workspace::{
    WorkspaceDirEntry, WorkspaceError, WorkspaceFileSystem, WorkspacePath, WorkspaceResult,
    WorkspaceWriteOutcome,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn semantic_query_ranks_filters_and_verifies_current_source() {
    let fixture = QueryFixture::start(&[
        ("src/cache.rs", "release temporary session vector memory\n"),
        ("docs/guide.md", "authentication token rotation guide\n"),
    ])
    .await;

    let result = fixture
        .search(
            WorkspaceSemanticSearchRequest::new("session cleanup")
                .with_path("src")
                .with_include("*.rs")
                .with_limit(1),
        )
        .await;

    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].chunk.path.as_ref(), "src/cache.rs");
    assert!(result.hits[0].chunk.text.contains("temporary session"));
    assert_eq!(result.fallback, None);
    assert_eq!(result.status.phase, WorkspaceRetrievalPhase::Ready);
    assert_eq!(
        result.status.vec_shadow.phase,
        WorkspaceVecShadowPhase::Ready
    );
    assert_eq!(result.status.vec_shadow.compared_queries, 1);
    assert_eq!(result.status.vec_shadow.matching_queries, 1);
    assert_eq!(fixture.files.read_paths(), vec!["src/cache.rs"]);
    fixture.runtime.close().await;
}

#[tokio::test]
async fn stale_source_is_never_returned_before_manifest_reconciliation() {
    let fixture =
        QueryFixture::start(&[("src/cache.rs", "release temporary session vector memory\n")]).await;
    fixture.files.replace(
        "src/cache.rs",
        "new unrelated content that the catalog has not observed\n",
    );

    let result = fixture
        .search(WorkspaceSemanticSearchRequest::new("session cleanup"))
        .await;

    assert!(result.hits.is_empty());
    assert_eq!(
        result.fallback,
        Some(WorkspaceSemanticFallbackReason::FilteredStaleHits)
    );
    fixture.runtime.close().await;
}

#[tokio::test]
async fn deleted_source_is_never_returned_before_manifest_reconciliation() {
    let fixture =
        QueryFixture::start(&[("src/cache.rs", "release temporary session vector memory\n")]).await;
    fixture.files.remove("src/cache.rs");

    let result = fixture
        .search(WorkspaceSemanticSearchRequest::new("session cleanup"))
        .await;

    assert!(result.hits.is_empty());
    assert_eq!(
        result.fallback,
        Some(WorkspaceSemanticFallbackReason::FilteredStaleHits)
    );
    fixture.runtime.close().await;
}

#[tokio::test]
async fn query_is_cancelled_by_the_caller_without_waiting_for_the_provider() {
    let catalog = populated_catalog(&[("src/cache.rs", "session cleanup\n")]);
    let files = MemoryFiles::from_entries(&[("src/cache.rs", "session cleanup\n")]);
    let provider = Arc::new(QueryProvider::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let runtime = WorkspaceRetrievalRuntime::start(
        Arc::clone(&catalog),
        WorkspaceRetrievalOptions::new(provider_port),
        CancellationToken::new(),
    )
    .unwrap();
    wait_until_ready(&runtime).await;
    provider.block_queries();
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = runtime
        .search(
            WorkspaceSemanticSearchRequest::new("session cleanup"),
            files,
            None,
            cancellation,
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        super::super::WorkspaceRetrievalError::Cancelled
    ));
    runtime.close().await;
}

#[tokio::test]
async fn a_ready_empty_corpus_is_complete_without_a_building_fallback() {
    let catalog = populated_catalog(&[("empty.rs", "")]);
    let files = MemoryFiles::from_entries(&[("empty.rs", "")]);
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(QueryProvider::new());
    let runtime = WorkspaceRetrievalRuntime::start(
        catalog,
        WorkspaceRetrievalOptions::new(provider),
        CancellationToken::new(),
    )
    .unwrap();
    wait_until_ready(&runtime).await;

    let result = runtime
        .search(
            WorkspaceSemanticSearchRequest::new("anything"),
            files,
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert!(result.hits.is_empty());
    assert_eq!(result.status.phase, WorkspaceRetrievalPhase::Ready);
    assert_eq!(result.fallback, None);
    runtime.close().await;
}

#[tokio::test]
async fn public_semantic_diagnostics_redact_query_and_source_text() {
    let query_sentinel = "private-query-sentinel";
    let source_sentinel = "private-source-sentinel\n";
    let fixture = QueryFixture::start(&[("private.rs", source_sentinel)]).await;
    let request = WorkspaceSemanticSearchRequest::new(query_sentinel);

    let request_debug = format!("{request:?}");
    assert!(!request_debug.contains(query_sentinel));
    let result = fixture.search(request).await;
    assert_eq!(result.hits.len(), 1);
    let result_debug = format!("{result:?}");
    assert!(!result_debug.contains(query_sentinel));
    assert!(!result_debug.contains(source_sentinel.trim()));
    assert!(!format!("{:?}", result.hits[0].chunk).contains(source_sentinel.trim()));
    fixture.runtime.close().await;
}

struct QueryFixture {
    runtime: Arc<WorkspaceRetrievalRuntime>,
    files: Arc<MemoryFiles>,
}

impl QueryFixture {
    async fn start(entries: &[(&str, &str)]) -> Self {
        let catalog = populated_catalog(entries);
        let files = MemoryFiles::from_entries(entries);
        let provider: Arc<dyn EmbeddingProvider> = Arc::new(QueryProvider::new());
        let runtime = WorkspaceRetrievalRuntime::start(
            catalog,
            WorkspaceRetrievalOptions::new(provider),
            CancellationToken::new(),
        )
        .unwrap();
        wait_until_ready(&runtime).await;
        Self { runtime, files }
    }

    async fn search(
        &self,
        request: WorkspaceSemanticSearchRequest,
    ) -> super::super::WorkspaceSemanticSearchResult {
        self.runtime
            .search(request, self.files.clone(), None, CancellationToken::new())
            .await
            .unwrap()
    }
}

fn populated_catalog(entries: &[(&str, &str)]) -> Arc<WorkspaceChunkCatalog> {
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .unwrap();
    for (path, content) in entries {
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

async fn wait_until_ready(runtime: &WorkspaceRetrievalRuntime) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.status().phase != WorkspaceRetrievalPhase::Ready {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("semantic index did not become ready");
}

struct QueryProvider {
    descriptor: EmbeddingProviderDescriptor,
    block_queries: std::sync::atomic::AtomicBool,
}

impl QueryProvider {
    fn new() -> Self {
        Self {
            descriptor: EmbeddingProviderDescriptor::new("fixture", "query-v1", 2),
            block_queries: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn block_queries(&self) {
        self.block_queries
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

#[async_trait]
impl EmbeddingProvider for QueryProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.descriptor.clone()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if request
            .inputs()
            .iter()
            .any(|input| input.id() == "workspace-query")
            && self
                .block_queries
                .load(std::sync::atomic::Ordering::Acquire)
        {
            cancellation.cancelled().await;
            return Err(EmbeddingProviderError::Cancelled);
        }
        let vectors = request
            .inputs()
            .iter()
            .map(|input| {
                let text = input.text().to_ascii_lowercase();
                let values = if text.contains("session")
                    || text.contains("temporary")
                    || text.contains("cleanup")
                {
                    vec![1.0, 0.0]
                } else {
                    vec![0.0, 1.0]
                };
                EmbeddingVector::new(input.id(), values)
            })
            .collect();
        Ok(EmbeddingBatchResponse::new(
            self.descriptor.clone(),
            vectors,
        ))
    }
}

#[derive(Default)]
struct MemoryFiles {
    files: Mutex<BTreeMap<String, String>>,
    reads: Mutex<Vec<String>>,
}

impl MemoryFiles {
    fn from_entries(entries: &[(&str, &str)]) -> Arc<Self> {
        Arc::new(Self {
            files: Mutex::new(
                entries
                    .iter()
                    .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
                    .collect(),
            ),
            reads: Mutex::new(Vec::new()),
        })
    }

    fn replace(&self, path: &str, content: &str) {
        self.files
            .lock()
            .unwrap()
            .insert(path.to_owned(), content.to_owned());
    }

    fn remove(&self, path: &str) {
        self.files.lock().unwrap().remove(path);
    }

    fn read_paths(&self) -> Vec<String> {
        self.reads.lock().unwrap().clone()
    }
}

#[async_trait]
impl WorkspaceFileSystem for MemoryFiles {
    async fn read_text(&self, path: &WorkspacePath) -> WorkspaceResult<String> {
        self.reads.lock().unwrap().push(path.as_str().to_owned());
        self.files
            .lock()
            .unwrap()
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| WorkspaceError::NotFound {
                path: path.as_str().to_owned(),
            })
    }

    async fn write_text(
        &self,
        path: &WorkspacePath,
        content: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome> {
        self.replace(path.as_str(), content);
        Ok(WorkspaceWriteOutcome {
            bytes: content.len(),
            lines: content.lines().count(),
        })
    }

    async fn list_dir(&self, _path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
        Ok(Vec::new())
    }
}
