use super::*;

/// Typed lexical FTS implementation for the session-owned workspace catalog.
#[napi(string_enum = "snake_case")]
pub enum WorkspaceLexicalEngineOption {
    #[napi(value = "portable")]
    Portable,
    #[napi(value = "zvec_rust")]
    ZvecRust,
}

fn default_lexical_engine() -> WorkspaceLexicalEngineOption {
    if cfg!(feature = "zvec-rust-fts") {
        WorkspaceLexicalEngineOption::ZvecRust
    } else {
        WorkspaceLexicalEngineOption::Portable
    }
}

#[napi(object)]
#[derive(Clone, Default)]
pub struct WorkspaceRetrievalOptionsObject {
    /// Opaque live provider identity. Pass a `WorkspaceRetrievalOptions` instance.
    pub instance_id: String,
    pub max_records: Option<f64>,
    pub max_bytes: Option<f64>,
    pub shutdown_timeout_ms: Option<f64>,
    /// Typed lexical FTS engine; omission selects the product default.
    pub lexical_engine: Option<WorkspaceLexicalEngineOption>,
    /// Opaque validated reranker snapshot; empty preserves RRF-only.
    pub reranker_instance_id: String,
    /// Opaque validated chunking snapshot; empty preserves line chunking.
    pub chunking_strategy_instance_id: String,
}

type NodeEmbeddingProviderRegistry = std::collections::HashMap<String, Weak<NodeEmbeddingProvider>>;

pub(super) fn embedding_provider_registry() -> &'static Mutex<NodeEmbeddingProviderRegistry> {
    static REGISTRY: OnceLock<Mutex<NodeEmbeddingProviderRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn resolve_embedding_provider(instance_id: &str) -> napi::Result<Arc<NodeEmbeddingProvider>> {
    if instance_id.trim().is_empty() {
        return Err(napi::Error::from_reason(
            "workspaceRetrieval requires the original WorkspaceRetrievalOptions instance",
        ));
    }
    let weak = {
        let mut registry = embedding_provider_registry()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        registry.retain(|_, provider| provider.strong_count() > 0);
        registry.get(instance_id).cloned()
    };
    weak.and_then(|provider| provider.upgrade()).ok_or_else(|| {
        napi::Error::from_reason(
            "WorkspaceRetrievalOptions provider identity is invalid or expired; pass the original instance",
        )
    })
}

/// Typed options that enable ephemeral semantic retrieval for one session.
#[napi]
pub struct WorkspaceRetrievalOptions {
    instance_id: String,
    max_records: f64,
    max_bytes: f64,
    shutdown_timeout_ms: f64,
    lexical_engine: WorkspaceLexicalEngineOption,
    reranker_instance_id: String,
    _reranker: Option<Arc<NodeDeterministicRerankerConfiguration>>,
    chunking_strategy_instance_id: String,
    _chunking_strategy: Option<Arc<NodeWorkspaceChunkingConfiguration>>,
    _provider: Arc<NodeEmbeddingProvider>,
}

#[napi]
impl WorkspaceRetrievalOptions {
    #[napi(
        constructor,
        ts_args_type = "provider: CallbackEmbeddingProvider, reranker?: DeterministicWorkspaceReranker | null, chunkingStrategy?: LineWorkspaceChunkingStrategy | FixedWindowWorkspaceChunkingStrategy | RecursiveWorkspaceChunkingStrategy | null, lexicalEngine?: WorkspaceLexicalEngineOption | null"
    )]
    pub fn new(
        provider: napi::bindgen_prelude::ClassInstance<CallbackEmbeddingProvider>,
        reranker: Option<napi::bindgen_prelude::ClassInstance<DeterministicWorkspaceReranker>>,
        chunking_strategy: Option<WorkspaceChunkingStrategyInput>,
        lexical_engine: Option<WorkspaceLexicalEngineOption>,
    ) -> napi::Result<Self> {
        let (reranker_instance_id, reranker) = match reranker.as_ref() {
            Some(reranker) => {
                let (instance_id, configuration) = bind_deterministic_reranker(reranker)?;
                (instance_id, Some(configuration))
            }
            None => (String::new(), None),
        };
        let (chunking_strategy_instance_id, chunking_strategy) = match chunking_strategy.as_ref() {
            Some(strategy) => {
                let (instance_id, configuration) = bind_workspace_chunking_strategy(strategy)?;
                (instance_id, Some(configuration))
            }
            None => (String::new(), None),
        };
        Ok(Self {
            instance_id: provider.instance_id.clone(),
            max_records: 100_000.0,
            max_bytes: (128 * 1024 * 1024) as f64,
            shutdown_timeout_ms: 5_000.0,
            lexical_engine: lexical_engine.unwrap_or_else(default_lexical_engine),
            reranker_instance_id,
            _reranker: reranker,
            chunking_strategy_instance_id,
            _chunking_strategy: chunking_strategy,
            _provider: Arc::clone(&provider.inner),
        })
    }

    /// Return the opaque provider identity used by structural SessionOptions conversion.
    #[napi(getter)]
    pub fn instance_id(&self) -> String {
        self.instance_id.clone()
    }

    #[napi(getter)]
    pub fn max_records(&self) -> f64 {
        self.max_records
    }

    #[napi(setter)]
    pub fn set_max_records(&mut self, value: f64) {
        self.max_records = value;
    }

    #[napi(getter)]
    pub fn max_bytes(&self) -> f64 {
        self.max_bytes
    }

    #[napi(setter)]
    pub fn set_max_bytes(&mut self, value: f64) {
        self.max_bytes = value;
    }

    #[napi(getter)]
    pub fn shutdown_timeout_ms(&self) -> f64 {
        self.shutdown_timeout_ms
    }

    #[napi(setter)]
    pub fn set_shutdown_timeout_ms(&mut self, value: f64) {
        self.shutdown_timeout_ms = value;
    }

    #[napi(getter)]
    pub fn lexical_engine(&self) -> WorkspaceLexicalEngineOption {
        self.lexical_engine
    }

    #[napi(setter)]
    pub fn set_lexical_engine(&mut self, value: WorkspaceLexicalEngineOption) {
        self.lexical_engine = value;
    }

    /// Return the opaque reranker snapshot used by structural conversion.
    #[napi(getter)]
    pub fn reranker_instance_id(&self) -> String {
        self.reranker_instance_id.clone()
    }

    /// Return the opaque chunking snapshot used by structural conversion.
    #[napi(getter)]
    pub fn chunking_strategy_instance_id(&self) -> String {
        self.chunking_strategy_instance_id.clone()
    }
}

impl Drop for WorkspaceRetrievalOptions {
    fn drop(&mut self) {
        if let Some(reranker) = &self._reranker {
            unregister_deterministic_reranker(&self.reranker_instance_id, reranker);
        }
        if let Some(chunking_strategy) = &self._chunking_strategy {
            unregister_workspace_chunking_strategy(
                &self.chunking_strategy_instance_id,
                chunking_strategy,
            );
        }
    }
}

pub(crate) fn js_workspace_retrieval_to_rust(
    options: &WorkspaceRetrievalOptionsObject,
) -> napi::Result<a3s_code_core::WorkspaceRetrievalOptions> {
    let chunking_strategy =
        resolve_workspace_chunking_strategy(&options.chunking_strategy_instance_id)?;
    let reranker = resolve_deterministic_reranker(&options.reranker_instance_id)?;
    let provider = resolve_embedding_provider(&options.instance_id)?;
    let provider: Arc<dyn EmbeddingProvider> = provider;
    let mut retrieval = a3s_code_core::WorkspaceRetrievalOptions::new(provider);
    let defaults = a3s_code_core::WorkspaceSemanticIndexLimits::default();
    let max_records = js_optional_usize(
        options.max_records,
        "workspaceRetrieval.maxRecords",
        defaults.max_records,
    )?;
    let max_bytes = js_optional_usize(
        options.max_bytes,
        "workspaceRetrieval.maxBytes",
        defaults.max_bytes,
    )?;
    let shutdown_timeout_ms = js_optional_usize(
        options.shutdown_timeout_ms,
        "workspaceRetrieval.shutdownTimeoutMs",
        defaults.shutdown_timeout.as_millis() as usize,
    )?;
    retrieval = retrieval.with_index_limits(a3s_code_core::WorkspaceSemanticIndexLimits {
        max_records,
        max_bytes,
        shutdown_timeout: Duration::from_millis(shutdown_timeout_ms as u64),
    });
    retrieval = retrieval.with_lexical_engine(match options
        .lexical_engine
        .unwrap_or_else(default_lexical_engine)
    {
        WorkspaceLexicalEngineOption::Portable => a3s_code_core::WorkspaceLexicalEngine::Portable,
        WorkspaceLexicalEngineOption::ZvecRust => {
            a3s_code_core::WorkspaceLexicalEngine::ZvecRust
        }
    });
    if let Some(reranker) = reranker {
        retrieval = retrieval.with_rerank_options(reranker);
    }
    if let Some(chunking_strategy) = chunking_strategy {
        retrieval = retrieval.with_chunking_strategy(chunking_strategy);
    }
    Ok(retrieval)
}
