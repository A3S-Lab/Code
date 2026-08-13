use super::super::{
    ChunkCatalogLimits, ChunkingConfig, WorkspaceChunkCatalog, WorkspaceHybridFallbackReason,
    WorkspaceHybridSearchRequest, WorkspaceRetrievalChannel, WorkspaceRetrievalOptions,
    WorkspaceRetrievalPhase, WorkspaceRetrievalRuntime,
};
use crate::code_intelligence::{
    CodeDiagnostic, CodeIntelligenceCapabilities, CodeIntelligenceError, CodeIntelligenceState,
    CodeIntelligenceStatus, CodeLocation, CodePosition, CodeQueryResult, CodeRange, CodeSymbolKind,
    DocumentSymbol, NavigationKind, SymbolInformation, WorkspaceCodeIntelligence,
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
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize)]
struct RetrievalFixture {
    documents: Vec<RetrievalDocument>,
    queries: Vec<RetrievalQuery>,
    expected_bm25_summary: RetrievalSummary,
    expected_hybrid_summary: RetrievalSummary,
}

#[derive(Debug, Deserialize)]
struct RetrievalDocument {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct RetrievalQuery {
    id: String,
    category: String,
    query: String,
    relevant_paths: Vec<String>,
    expected_hybrid_paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RetrievalSummary {
    query_count: usize,
    recall_at_10: f64,
    mean_reciprocal_rank: f64,
    category_recall_at_10: BTreeMap<String, f64>,
}

#[tokio::test]
async fn locked_hybrid_fixture_meets_quality_and_identifier_gates() {
    let fixture: RetrievalFixture = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/workspace-retrieval-v1/corpus.json"
    )))
    .expect("workspace retrieval fixture must parse");
    let entries = fixture
        .documents
        .iter()
        .map(|document| (document.path.as_str(), document.content.as_str()))
        .collect::<Vec<_>>();
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(QualityProvider::from_fixture(&fixture));
    let fixture_runtime = HybridFixture::start_with_provider(&entries, provider).await;
    let mut recalled = 0usize;
    let mut reciprocal_rank_sum = 0.0;
    let mut category_counts = BTreeMap::<String, (usize, usize)>::new();

    for query in &fixture.queries {
        let result = fixture_runtime
            .search(WorkspaceHybridSearchRequest::new(&query.query).with_limit(10))
            .await;
        let paths = result
            .hits
            .iter()
            .map(|hit| hit.chunk.path.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            paths, query.expected_hybrid_paths,
            "hybrid result drifted for query '{}'",
            query.id
        );
        let first_relevant_rank = paths
            .iter()
            .position(|path| query.relevant_paths.contains(path));
        let was_recalled = first_relevant_rank.is_some();
        recalled += usize::from(was_recalled);
        reciprocal_rank_sum += first_relevant_rank
            .map(|rank| 1.0 / (rank + 1) as f64)
            .unwrap_or_default();
        let counts = category_counts.entry(query.category.clone()).or_default();
        counts.0 += usize::from(was_recalled);
        counts.1 += 1;

        if query.category == "identifier" && query.query == "LocalWorkspaceAccessPolicy" {
            assert!(result.hits[0].exact_identifier);
            assert_eq!(result.hits[0].chunk.path.as_ref(), "src/path_policy.rs");
        }
    }

    let query_count = fixture.queries.len() as f64;
    let recall = recalled as f64 / query_count;
    let mrr = reciprocal_rank_sum / query_count;
    assert_metric(
        recall,
        fixture.expected_hybrid_summary.recall_at_10,
        "hybrid Recall@10",
    );
    assert_metric(
        mrr,
        fixture.expected_hybrid_summary.mean_reciprocal_rank,
        "hybrid MRR",
    );
    assert!(recall >= 0.85);
    assert!(
        recall - fixture.expected_bm25_summary.recall_at_10 >= 0.15,
        "hybrid Recall@10 improvement was below 15 points"
    );
    for (category, expected) in &fixture.expected_hybrid_summary.category_recall_at_10 {
        let (category_recalled, category_total) =
            category_counts.get(category).copied().unwrap_or_default();
        assert_metric(
            category_recalled as f64 / category_total as f64,
            *expected,
            &format!("{category} Recall@10"),
        );
    }
    assert_eq!(
        fixture.expected_hybrid_summary.query_count,
        fixture.queries.len()
    );
    fixture_runtime.runtime.close().await;
}

#[tokio::test]
async fn hybrid_filters_stale_source_after_fusing_candidates() {
    let fixture =
        HybridFixture::start(&[("src/cache.rs", "release temporary session vector memory\n")])
            .await;
    fixture.files.replace(
        "src/cache.rs",
        "unreconciled content that no longer matches the catalog\n",
    );

    let result = fixture
        .search(WorkspaceHybridSearchRequest::new("session cleanup"))
        .await;

    assert!(result.hits.is_empty());
    assert_eq!(
        result.fallback,
        Some(WorkspaceHybridFallbackReason::FilteredStaleHits)
    );
    assert_eq!(fixture.files.read_paths(), vec!["src/cache.rs"]);
    fixture.runtime.close().await;
}

#[tokio::test]
async fn query_embedding_failure_keeps_lexical_results_and_reports_partial_channel() {
    let entries = &[("src/cache.rs", "session cache invalidation policy\n")];
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(QueryFailingProvider {
        inner: QualityProvider::from_documents(entries),
    });
    let fixture = HybridFixture::start_with_provider(entries, provider).await;

    let result = fixture
        .search(WorkspaceHybridSearchRequest::new(
            "session cache invalidation",
        ))
        .await;

    assert_eq!(result.hits[0].chunk.path.as_ref(), "src/cache.rs");
    assert_eq!(
        result.fallback,
        Some(WorkspaceHybridFallbackReason::QueryEmbeddingFailed)
    );
    let semantic = result
        .channels
        .iter()
        .find(|status| status.channel == WorkspaceRetrievalChannel::Semantic)
        .unwrap();
    assert_eq!(semantic.candidate_count, 0);
    assert_eq!(
        semantic.fallback,
        Some(WorkspaceHybridFallbackReason::QueryEmbeddingFailed)
    );
    fixture.runtime.close().await;
}

#[tokio::test]
async fn caller_cancellation_reaches_an_active_query_embedding_provider() {
    let entries = &[("src/cache.rs", "session cache invalidation policy\n")];
    let provider = Arc::new(BlockingQueryProvider::new(QualityProvider::from_documents(
        entries,
    )));
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let fixture = HybridFixture::start_with_provider(entries, provider_port).await;
    let cancellation = CancellationToken::new();
    let search = fixture.runtime.hybrid_search(
        WorkspaceHybridSearchRequest::new("session cache invalidation"),
        fixture.files.clone(),
        None,
        None,
        cancellation.clone(),
    );
    tokio::pin!(search);

    tokio::select! {
        () = provider.query_started.notified() => {}
        result = &mut search => panic!("query completed before provider blocked: {result:?}"),
    }
    cancellation.cancel();
    let error = search.await.unwrap_err();

    assert!(matches!(
        error,
        super::super::WorkspaceRetrievalError::Cancelled
    ));
    assert!(provider.query_token().is_cancelled());
    fixture.runtime.close().await;
}

#[tokio::test]
async fn structural_channel_maps_symbols_to_catalog_chunks_and_marks_identifier_evidence() {
    let fixture = HybridFixture::start(&[("src/widget.rs", "pub struct Widget;\n")]).await;
    let provider: Arc<dyn WorkspaceCodeIntelligence> =
        Arc::new(SymbolProvider::ready(vec![SymbolInformation {
            name: "Widget".to_owned(),
            kind: CodeSymbolKind::Struct,
            location: CodeLocation {
                path: WorkspacePath::from_normalized("src/widget.rs"),
                range: CodeRange::new(CodePosition::new(0, 11), CodePosition::new(0, 17)),
            },
            container_name: None,
        }]));

    let result = fixture
        .runtime
        .hybrid_search(
            WorkspaceHybridSearchRequest::new("Widget"),
            fixture.files.clone(),
            Some(provider),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(result.hits[0].chunk.path.as_ref(), "src/widget.rs");
    assert!(result.hits[0].exact_identifier);
    assert!(result.hits[0]
        .channels
        .iter()
        .any(|rank| rank.channel == WorkspaceRetrievalChannel::Structural));
    let structural = result
        .channels
        .iter()
        .find(|status| status.channel == WorkspaceRetrievalChannel::Structural)
        .unwrap();
    assert_eq!(structural.candidate_count, 1);
    assert_eq!(structural.fallback, None);
    fixture.runtime.close().await;
}

struct HybridFixture {
    runtime: Arc<WorkspaceRetrievalRuntime>,
    files: Arc<MemoryFiles>,
}

impl HybridFixture {
    async fn start(entries: &[(&str, &str)]) -> Self {
        let provider: Arc<dyn EmbeddingProvider> =
            Arc::new(QualityProvider::from_documents(entries));
        Self::start_with_provider(entries, provider).await
    }

    async fn start_with_provider(
        entries: &[(&str, &str)],
        provider: Arc<dyn EmbeddingProvider>,
    ) -> Self {
        let catalog =
            WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
                .unwrap();
        for (revision, (path, content)) in entries.iter().enumerate() {
            catalog
                .replace_file(
                    &WorkspacePath::from_normalized(*path),
                    path.ends_with(".rs").then_some("rust"),
                    revision as u64 + 1,
                    content,
                )
                .unwrap();
        }
        let files = MemoryFiles::from_entries(entries);
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
        request: WorkspaceHybridSearchRequest,
    ) -> super::super::WorkspaceHybridSearchResult {
        self.runtime
            .hybrid_search(
                request,
                self.files.clone(),
                None,
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap()
    }
}

struct QualityProvider {
    descriptor: EmbeddingProviderDescriptor,
    slots: BTreeMap<String, usize>,
    fallback_slot: usize,
}

impl QualityProvider {
    fn from_fixture(fixture: &RetrievalFixture) -> Self {
        let fallback_slot = fixture.documents.len();
        let mut slots = fixture
            .documents
            .iter()
            .enumerate()
            .map(|(slot, document)| (document.content.clone(), slot))
            .collect::<BTreeMap<_, _>>();
        let path_slots = fixture
            .documents
            .iter()
            .enumerate()
            .map(|(slot, document)| (document.path.as_str(), slot))
            .collect::<BTreeMap<_, _>>();
        for query in &fixture.queries {
            let relevant = query
                .relevant_paths
                .first()
                .and_then(|path| path_slots.get(path.as_str()))
                .copied()
                .expect("every query must reference a fixture document");
            slots.insert(query.query.clone(), relevant);
        }
        Self {
            descriptor: EmbeddingProviderDescriptor::new(
                "fixture",
                "hybrid-quality-v1",
                fallback_slot + 1,
            ),
            slots,
            fallback_slot,
        }
    }

    fn from_documents(entries: &[(&str, &str)]) -> Self {
        let fallback_slot = entries.len();
        Self {
            descriptor: EmbeddingProviderDescriptor::new(
                "fixture",
                "hybrid-documents-v1",
                fallback_slot + 1,
            ),
            slots: entries
                .iter()
                .enumerate()
                .map(|(slot, (_, content))| ((*content).to_owned(), slot))
                .collect(),
            fallback_slot,
        }
    }

    fn vector(&self, text: &str) -> Vec<f32> {
        let mut vector = vec![0.0; self.descriptor.dimension];
        let slot = self.slots.get(text).copied().unwrap_or(self.fallback_slot);
        vector[slot] = 1.0;
        vector
    }
}

#[async_trait]
impl EmbeddingProvider for QualityProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.descriptor.clone()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        _cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        let vectors = request
            .inputs()
            .iter()
            .map(|input| EmbeddingVector::new(input.id(), self.vector(input.text())))
            .collect();
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

struct QueryFailingProvider {
    inner: QualityProvider,
}

struct BlockingQueryProvider {
    inner: QualityProvider,
    query_started: Notify,
    query_cancellation: Mutex<Option<CancellationToken>>,
}

impl BlockingQueryProvider {
    fn new(inner: QualityProvider) -> Self {
        Self {
            inner,
            query_started: Notify::new(),
            query_cancellation: Mutex::new(None),
        }
    }

    fn query_token(&self) -> CancellationToken {
        self.query_cancellation
            .lock()
            .unwrap()
            .clone()
            .expect("query provider token was not captured")
    }
}

#[async_trait]
impl EmbeddingProvider for BlockingQueryProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.inner.descriptor()
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
        {
            *self.query_cancellation.lock().unwrap() = Some(cancellation.clone());
            self.query_started.notify_one();
            cancellation.cancelled().await;
            return Err(EmbeddingProviderError::Cancelled);
        }
        let vectors = request
            .inputs()
            .iter()
            .map(|input| EmbeddingVector::new(input.id(), self.inner.vector(input.text())))
            .collect();
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

#[async_trait]
impl EmbeddingProvider for QueryFailingProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.inner.descriptor()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        _cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if request
            .inputs()
            .iter()
            .any(|input| input.id() == "workspace-query")
        {
            return Err(EmbeddingProviderError::Other);
        }
        let vectors = request
            .inputs()
            .iter()
            .map(|input| EmbeddingVector::new(input.id(), self.inner.vector(input.text())))
            .collect();
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

async fn wait_until_ready(runtime: &WorkspaceRetrievalRuntime) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while runtime.status().phase != WorkspaceRetrievalPhase::Ready {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("hybrid semantic index did not become ready");
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

fn assert_metric(actual: f64, expected: f64, name: &str) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "{name} drifted: actual={actual}, expected={expected}"
    );
}

struct SymbolProvider {
    status: tokio::sync::watch::Sender<CodeIntelligenceStatus>,
    symbols: Vec<SymbolInformation>,
}

impl SymbolProvider {
    fn ready(symbols: Vec<SymbolInformation>) -> Self {
        let (status, _) = tokio::sync::watch::channel(CodeIntelligenceStatus {
            state: CodeIntelligenceState::Ready,
            capabilities: CodeIntelligenceCapabilities {
                workspace_symbols: true,
                ..CodeIntelligenceCapabilities::default()
            },
            languages: Vec::new(),
            message: None,
        });
        Self { status, symbols }
    }

    fn unavailable<T>() -> Result<T, CodeIntelligenceError> {
        Err(CodeIntelligenceError::Unavailable {
            message: "fixture operation is unavailable".to_owned(),
        })
    }
}

#[async_trait]
impl WorkspaceCodeIntelligence for SymbolProvider {
    fn subscribe_status(&self) -> tokio::sync::watch::Receiver<CodeIntelligenceStatus> {
        self.status.subscribe()
    }

    async fn document_symbols(
        &self,
        _path: &WorkspacePath,
        _cancellation: CancellationToken,
    ) -> Result<CodeQueryResult<DocumentSymbol>, CodeIntelligenceError> {
        Self::unavailable()
    }

    async fn search_symbols(
        &self,
        _query: &str,
        limit: usize,
        _cancellation: CancellationToken,
    ) -> Result<CodeQueryResult<SymbolInformation>, CodeIntelligenceError> {
        Ok(CodeQueryResult {
            items: self.symbols.iter().take(limit).cloned().collect(),
            truncated: self.symbols.len() > limit,
            workspace_revision: 1,
            document: None,
        })
    }

    async fn navigate(
        &self,
        _kind: NavigationKind,
        _path: &WorkspacePath,
        _position: CodePosition,
        _cancellation: CancellationToken,
    ) -> Result<CodeQueryResult<CodeLocation>, CodeIntelligenceError> {
        Self::unavailable()
    }

    async fn diagnostics(
        &self,
        _path: Option<&WorkspacePath>,
        _cancellation: CancellationToken,
    ) -> Result<CodeQueryResult<CodeDiagnostic>, CodeIntelligenceError> {
        Self::unavailable()
    }
}
