//! Release qualification for session-local workspace retrieval.
//!
//! Run from the Code repository root with:
//!
//! `cargo run --locked --release -p a3s-code-core --example workspace_retrieval_benchmark`
//!
//! The benchmark exercises the production architecture: A3S Memory owns the
//! exact semantic vectors, the selected lexical engine owns FTS/BM25, hybrid
//! channels are fused with RRF, and the optional deterministic reranker is
//! bounded by its public resource contract. The embedding provider is
//! deterministic so the measurements exclude network variability.

use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingProvider, EmbeddingProviderDescriptor,
    EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::{
    Agent, ChunkCatalogLimits, ChunkingConfig, CodeConfig, SessionOptions,
    WorkspaceHybridSearchRequest, WorkspaceLexicalEngine, WorkspaceRerankMode,
    WorkspaceRerankOptions, WorkspaceRetrievalOptions, WorkspaceRetrievalPhase,
    WorkspaceSemanticIndexLimits,
};
use a3s_memory::vector::{
    InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor, VectorRecord, VectorSearchRequest,
};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

// Keep the exact-vector gate compatible with the historical release profile.
const EXACT_RECORD_COUNT: usize = 25_000;
const DIMENSION: usize = 384;
const TOP_K: usize = 20;
const WARMUP_SAMPLES: usize = 20;
const MEASURED_SAMPLES: usize = 100;
const EXACT_P95_BUDGET_MS: f64 = 30.0;
const HYBRID_P95_BUDGET_MS: f64 = 100.0;
const RERANK_P95_DELTA_BUDGET_MS: f64 = 10.0;
const RERANK_SCRATCH_BUDGET_BYTES: usize = 4 * 1024 * 1024;
const RERANK_CANDIDATE_BUDGET: usize = 100;
const MAX_VECTOR_BYTES: usize = 128 * 1024 * 1024;
const MAX_CATALOG_BYTES: usize = 256 * 1024 * 1024;

// The hybrid fixture is intentionally multi-chunk per file. Four sizeable
// partitions exercise the native zvec path while avoiding an artificial
// descriptor storm: zvec/RocksDB owns several descriptors per collection and
// Code's catalog deliberately keeps that resource bounded.
const HYBRID_FILE_COUNT: usize = 4;
const HYBRID_CHUNKS_PER_FILE: usize = 128;
const HYBRID_LINES_PER_CHUNK: usize = 16;
const HYBRID_RECORD_COUNT: usize = HYBRID_FILE_COUNT * HYBRID_CHUNKS_PER_FILE;
const LEXICAL_ENGINE_ENV: &str = "A3S_WORKSPACE_LEXICAL_ENGINE";

#[tokio::main]
async fn main() -> Result<()> {
    if cfg!(debug_assertions) {
        bail!("workspace retrieval qualification must run with --release");
    }

    let exact = benchmark_exact_vector_search().await?;
    let lexical_engine = configured_lexical_engine()?;
    let hybrid_rrf = benchmark_hybrid_search(
        WorkspaceRerankOptions::default(),
        "workspace-retrieval-qualification-rrf",
        lexical_engine,
    )
    .await?;
    let hybrid_deterministic = benchmark_hybrid_search(
        WorkspaceRerankOptions::deterministic(),
        "workspace-retrieval-qualification-rerank",
        lexical_engine,
    )
    .await?;

    let exact_passed = exact.latency.p95_ms <= EXACT_P95_BUDGET_MS;
    let rrf_passed = hybrid_rrf.latency.p95_ms <= HYBRID_P95_BUDGET_MS
        && hybrid_rrf.batching_passed()
        && hybrid_rrf.semantic_passed();
    let deterministic_passed = hybrid_deterministic.latency.p95_ms <= HYBRID_P95_BUDGET_MS
        && hybrid_deterministic.batching_passed()
        && hybrid_deterministic.semantic_passed();
    let signed_rerank_delta = hybrid_deterministic.latency.p95_ms - hybrid_rrf.latency.p95_ms;
    let added_rerank_delta = signed_rerank_delta.max(0.0);
    let rerank_passed = added_rerank_delta <= RERANK_P95_DELTA_BUDGET_MS
        && hybrid_deterministic.max_input_candidates <= RERANK_CANDIDATE_BUDGET
        && hybrid_deterministic.max_evaluated_candidates <= RERANK_CANDIDATE_BUDGET
        && hybrid_deterministic.max_accounted_scratch_bytes <= RERANK_SCRATCH_BUDGET_BYTES
        && hybrid_deterministic.rerank_fallbacks == 0;
    let passed = exact_passed && rrf_passed && deterministic_passed && rerank_passed;

    let report = json!({
        "schemaVersion": 5,
        "profile": "workspace-retrieval-v5",
        "build": "release",
        "lexicalEngine": lexical_engine.stable_id(),
        "machine": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "logicalCpus": std::thread::available_parallelism().map(|value| value.get()).unwrap_or(1),
            "processor": std::env::var("PROCESSOR_IDENTIFIER").ok(),
        },
        "parameters": {
            "exactRecords": EXACT_RECORD_COUNT,
            "dimension": DIMENSION,
            "hybridFiles": HYBRID_FILE_COUNT,
            "hybridChunksPerFile": HYBRID_CHUNKS_PER_FILE,
            "hybridRecords": HYBRID_RECORD_COUNT,
            "linesPerChunk": HYBRID_LINES_PER_CHUNK,
            "topK": TOP_K,
            "warmupSamples": WARMUP_SAMPLES,
            "measuredSamples": MEASURED_SAMPLES,
        },
        "exactVector": {
            "p50Ms": exact.latency.p50_ms,
            "p95Ms": exact.latency.p95_ms,
            "maxMs": exact.latency.max_ms,
            "buildMs": exact.build_ms,
            "accountedBytes": exact.accounted_bytes,
            "providerNetworkIncluded": false,
            "sourceReadsIncluded": false,
            "budgetP95Ms": EXACT_P95_BUDGET_MS,
            "passed": exact_passed,
        },
        "hybridRrfOnly": hybrid_json(&hybrid_rrf, rrf_passed),
        "hybridDeterministic": hybrid_json(&hybrid_deterministic, deterministic_passed),
        "rerankComparison": {
            "rrfOnlyP95Ms": hybrid_rrf.latency.p95_ms,
            "deterministicP95Ms": hybrid_deterministic.latency.p95_ms,
            "p95SignedDeltaMs": signed_rerank_delta,
            "p95AddedMs": added_rerank_delta,
            "budgetP95DeltaMs": RERANK_P95_DELTA_BUDGET_MS,
            "maxInputCandidates": hybrid_deterministic.max_input_candidates,
            "maxEvaluatedCandidates": hybrid_deterministic.max_evaluated_candidates,
            "candidateBudget": RERANK_CANDIDATE_BUDGET,
            "maxFeatureBytes": hybrid_deterministic.max_feature_bytes,
            "maxAccountedScratchBytes": hybrid_deterministic.max_accounted_scratch_bytes,
            "scratchBudgetBytes": RERANK_SCRATCH_BUDGET_BYTES,
            "fallbacks": hybrid_deterministic.rerank_fallbacks,
            "passed": rerank_passed,
        },
        "resourceCeilings": {
            "semanticVectorBytes": MAX_VECTOR_BYTES,
            "catalogLexicalAndTextBytes": MAX_CATALOG_BYTES,
        },
        "passed": passed,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    if !passed {
        bail!(
            "workspace retrieval qualification failed: exact p95 {:.3} ms, RRF p95 {:.3} ms, deterministic p95 {:.3} ms, rerank delta {:.3} ms",
            exact.latency.p95_ms,
            hybrid_rrf.latency.p95_ms,
            hybrid_deterministic.latency.p95_ms,
            added_rerank_delta,
        );
    }
    Ok(())
}

async fn benchmark_exact_vector_search() -> Result<ExactMeasurement> {
    let descriptor = VectorIndexDescriptor::new(DIMENSION)
        .with_max_records(EXACT_RECORD_COUNT)
        .with_max_bytes(MAX_VECTOR_BYTES);
    let index = InMemoryVectorIndex::new(descriptor)?;
    let records = (0..EXACT_RECORD_COUNT)
        .map(|index| VectorRecord::new(format!("record-{index:05}"), basis_vector(index)))
        .collect();
    let build_started = Instant::now();
    let status = index.replace_partition("qualification", records).await?;
    let build_ms = elapsed_ms(build_started.elapsed());
    if status.record_count != EXACT_RECORD_COUNT {
        bail!(
            "exact index admitted {} records instead of {EXACT_RECORD_COUNT}",
            status.record_count
        );
    }

    let query = basis_vector(17);
    for _ in 0..WARMUP_SAMPLES {
        let result = index
            .search(VectorSearchRequest::new(query.clone(), TOP_K))
            .await?;
        if result.hits.len() != TOP_K {
            bail!("exact vector warmup returned an incomplete top-k");
        }
    }
    let latency = measure_async(MEASURED_SAMPLES, || async {
        let result = index
            .search(VectorSearchRequest::new(query.clone(), TOP_K))
            .await?;
        if result.hits.len() != TOP_K {
            bail!("exact vector sample returned an incomplete top-k");
        }
        Ok(())
    })
    .await?;
    let accounted_bytes = index.status().byte_count;
    index.clear().await?;
    if index.status().record_count != 0 || index.status().byte_count != 0 {
        bail!("exact vector index retained memory after clear");
    }

    Ok(ExactMeasurement {
        latency,
        build_ms,
        accounted_bytes,
    })
}

async fn benchmark_hybrid_search(
    rerank: WorkspaceRerankOptions,
    session_id: &str,
    lexical_engine: WorkspaceLexicalEngine,
) -> Result<HybridMeasurement> {
    let workspace = tempfile::tempdir()?;
    let source_bytes = write_hybrid_workspace(workspace.path())?;
    let provider = Arc::new(BenchmarkProvider::new());
    let retrieval = WorkspaceRetrievalOptions::new(provider.clone())
        .with_index_limits(WorkspaceSemanticIndexLimits {
            max_records: HYBRID_RECORD_COUNT,
            max_bytes: MAX_VECTOR_BYTES,
            shutdown_timeout: Duration::from_secs(5),
        })
        .with_lexical_engine(lexical_engine)
        .with_chunking_config(ChunkingConfig {
            max_lines: HYBRID_LINES_PER_CHUNK,
            max_bytes: 16 * 1024,
            max_chunks_per_file: HYBRID_CHUNKS_PER_FILE,
        })
        .with_catalog_limits(ChunkCatalogLimits {
            max_files: HYBRID_FILE_COUNT,
            max_chunks: HYBRID_RECORD_COUNT,
            max_text_bytes: MAX_CATALOG_BYTES,
            max_index_bytes: MAX_CATALOG_BYTES,
        })
        .with_rerank_options(rerank);
    let agent = Agent::from_config(benchmark_config()?).await?;
    let session_started = Instant::now();
    let session = agent
        .session_async(
            workspace.path().to_string_lossy(),
            Some(
                SessionOptions::new()
                    .with_session_id(session_id)
                    .with_workspace_retrieval(retrieval),
            ),
        )
        .await?;
    let session_construction_ms = elapsed_ms(session_started.elapsed());

    let status = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let status = session.workspace_retrieval_status();
            match status.phase {
                WorkspaceRetrievalPhase::Ready
                    if status.indexed_files == HYBRID_FILE_COUNT
                        && status.indexed_chunks == HYBRID_RECORD_COUNT
                        && status.vector_records == HYBRID_RECORD_COUNT =>
                {
                    return Ok(status);
                }
                WorkspaceRetrievalPhase::Degraded => {
                    bail!("semantic workspace build degraded: {status:?}");
                }
                WorkspaceRetrievalPhase::Closed => {
                    bail!("semantic workspace closed while building");
                }
                _ => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .context("semantic workspace build exceeded 120 seconds")??;

    if status.lexical_engine != lexical_engine {
        bail!(
            "workspace status selected {:?}, expected {:?}",
            status.lexical_engine,
            lexical_engine
        );
    }
    if provider.document_inputs.load(Ordering::Acquire) != HYBRID_RECORD_COUNT {
        bail!(
            "provider received {} document inputs instead of {HYBRID_RECORD_COUNT}",
            provider.document_inputs.load(Ordering::Acquire)
        );
    }
    let document_embedding_requests = provider.document_requests.load(Ordering::Acquire);
    if document_embedding_requests != status.batching.document_provider_requests {
        bail!(
            "provider observed {document_embedding_requests} document requests while status reported {}",
            status.batching.document_provider_requests
        );
    }
    let document_batch_limit_lower_bound = status.batching.batch_limit_lower_bound;
    let document_request_amplification =
        document_embedding_requests as f64 / document_batch_limit_lower_bound.max(1) as f64;

    let request = WorkspaceHybridSearchRequest::new("cache invalidation").with_limit(TOP_K);
    let rerank_observation = RerankObservation::default();
    for _ in 0..WARMUP_SAMPLES {
        let result = session.hybrid_search(request.clone()).await?;
        validate_hybrid(&result, rerank.mode, lexical_engine)?;
        rerank_observation.observe(&result)?;
    }
    let document_inputs_before_measurement = provider.document_inputs.load(Ordering::Acquire);
    let latency = measure_async(MEASURED_SAMPLES, || async {
        let result = session.hybrid_search(request.clone()).await?;
        validate_hybrid(&result, rerank.mode, lexical_engine)?;
        rerank_observation.observe(&result)?;
        Ok(())
    })
    .await?;
    if provider.document_inputs.load(Ordering::Acquire) != document_inputs_before_measurement {
        bail!("repeated hybrid queries re-embedded unchanged source chunks");
    }
    let query_embedding_inputs = provider.query_inputs.load(Ordering::Acquire);
    let expected_queries = WARMUP_SAMPLES + MEASURED_SAMPLES;
    if query_embedding_inputs != expected_queries {
        bail!(
            "provider received {query_embedding_inputs} query inputs instead of {expected_queries}"
        );
    }
    if provider.query_requests.load(Ordering::Acquire) != expected_queries {
        bail!(
            "provider received {} query requests instead of {expected_queries}",
            provider.query_requests.load(Ordering::Acquire)
        );
    }

    let final_status = session.workspace_retrieval_status();
    if !matches!(final_status.phase, WorkspaceRetrievalPhase::Ready) {
        bail!("workspace became unavailable during qualification: {final_status:?}");
    }
    let vector_bytes = final_status.vector_bytes;
    session.close().await;
    let closed = session.workspace_retrieval_status();
    if closed.phase != WorkspaceRetrievalPhase::Closed
        || closed.vector_records != 0
        || closed.vector_bytes != 0
    {
        bail!("session retained semantic vector state after close: {closed:?}");
    }

    Ok(HybridMeasurement {
        latency,
        lexical_engine,
        workspace_build_ms: elapsed_ms(session_started.elapsed()),
        session_construction_ms,
        source_bytes,
        catalog_files: status.catalog_files,
        catalog_chunks: status.catalog_chunks,
        vector_bytes,
        document_embedding_inputs: status.batching.document_inputs,
        document_embedding_requests,
        document_batches: status.batching.document_batches,
        document_batch_limit_lower_bound,
        document_request_amplification,
        generation_complete_flushes: status.batching.generation_complete_flushes,
        time_to_first_ready_ms: status.batching.time_to_first_ready_ms,
        non_text_inputs: status.batching.non_text_inputs,
        query_embedding_inputs,
        max_input_candidates: rerank_observation.input_candidates.load(Ordering::Acquire),
        max_evaluated_candidates: rerank_observation
            .evaluated_candidates
            .load(Ordering::Acquire),
        max_feature_bytes: rerank_observation.feature_bytes.load(Ordering::Acquire),
        max_accounted_scratch_bytes: rerank_observation
            .accounted_scratch_bytes
            .load(Ordering::Acquire),
        rerank_fallbacks: rerank_observation.fallbacks.load(Ordering::Acquire),
    })
}

fn validate_hybrid(
    result: &a3s_code_core::WorkspaceHybridSearchResult,
    requested_mode: WorkspaceRerankMode,
    lexical_engine: WorkspaceLexicalEngine,
) -> Result<()> {
    if result.hits.is_empty() {
        bail!("hybrid query returned no verified hits");
    }
    if result.rerank.requested_mode != requested_mode
        || result.rerank.applied_mode != requested_mode
        || result.rerank.fallback.is_some()
    {
        bail!("unexpected reranker status: {:?}", result.rerank);
    }
    if result.semantic_status.lexical_engine != lexical_engine {
        bail!("hybrid result reported an unexpected lexical engine");
    }
    let lexical = result
        .channels
        .iter()
        .find(|channel| channel.channel == a3s_code_core::WorkspaceRetrievalChannel::Lexical)
        .context("hybrid result omitted the lexical channel")?;
    if lexical.candidate_count == 0 {
        bail!("hybrid query did not exercise the lexical channel");
    }
    if result.hits.iter().any(|hit| hit.chunk.source_revision == 0) {
        bail!("hybrid result contained an unverified source chunk");
    }
    Ok(())
}

fn hybrid_json(measurement: &HybridMeasurement, passed: bool) -> serde_json::Value {
    json!({
        "lexicalEngine": measurement.lexical_engine.stable_id(),
        "p50Ms": measurement.latency.p50_ms,
        "p95Ms": measurement.latency.p95_ms,
        "maxMs": measurement.latency.max_ms,
        "workspaceBuildMs": measurement.workspace_build_ms,
        "sessionConstructionMs": measurement.session_construction_ms,
        "sourceBytes": measurement.source_bytes,
        "catalogFiles": measurement.catalog_files,
        "catalogChunks": measurement.catalog_chunks,
        "vectorBytes": measurement.vector_bytes,
        "documentEmbeddingInputs": measurement.document_embedding_inputs,
        "documentEmbeddingRequests": measurement.document_embedding_requests,
        "documentBatches": measurement.document_batches,
        "documentBatchLimitLowerBound": measurement.document_batch_limit_lower_bound,
        "documentRequestAmplification": measurement.document_request_amplification,
        "generationCompleteFlushes": measurement.generation_complete_flushes,
        "timeToFirstReadyMs": measurement.time_to_first_ready_ms,
        "nonTextInputs": measurement.non_text_inputs,
        "queryEmbeddingInputs": measurement.query_embedding_inputs,
        "maxInputCandidates": measurement.max_input_candidates,
        "maxEvaluatedCandidates": measurement.max_evaluated_candidates,
        "maxFeatureBytes": measurement.max_feature_bytes,
        "maxAccountedScratchBytes": measurement.max_accounted_scratch_bytes,
        "rerankFallbacks": measurement.rerank_fallbacks,
        "providerNetworkIncluded": false,
        "authoritativeSourceReads": "included from the warm OS cache",
        "budgetP95Ms": HYBRID_P95_BUDGET_MS,
        "passed": passed,
    })
}

fn benchmark_config() -> Result<CodeConfig> {
    CodeConfig::from_acl(
        r#"
default_model = "qualification/unused"
providers "qualification" {
  apiKey = "not-used"
  baseUrl = "http://127.0.0.1:1/v1"
  models "unused" {}
}
"#,
    )
    .map_err(anyhow::Error::msg)
}

fn write_hybrid_workspace(root: &std::path::Path) -> Result<usize> {
    let mut source_bytes = 0usize;
    for file in 0..HYBRID_FILE_COUNT {
        let mut content = String::new();
        for chunk in 0..HYBRID_CHUNKS_PER_FILE {
            for line in 0..HYBRID_LINES_PER_CHUNK {
                content.push_str(&format!(
                    "pub fn cache_invalidation_{file}_{chunk}_{line}() {{ /* session cache policy */ }}\n"
                ));
            }
        }
        source_bytes = source_bytes
            .checked_add(content.len())
            .context("workspace source byte count overflowed")?;
        std::fs::write(root.join(format!("module_{file:04}.rs")), content)?;
    }
    Ok(source_bytes)
}

fn basis_vector(seed: usize) -> Vec<f32> {
    let mut vector = vec![0.0; DIMENSION];
    vector[seed % DIMENSION] = 1.0;
    vector
}

fn stable_slot(value: &str) -> usize {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash as usize % DIMENSION
}

fn configured_lexical_engine() -> Result<WorkspaceLexicalEngine> {
    match std::env::var(LEXICAL_ENGINE_ENV).as_deref() {
        Ok("zvec_rust") => {
            if !cfg!(feature = "zvec-rust-fts") {
                bail!(
                    "{LEXICAL_ENGINE_ENV}=zvec_rust requires the zvec-rust-fts feature; use a native build or select portable"
                );
            }
            Ok(WorkspaceLexicalEngine::ZvecRust)
        }
        Ok("portable") => Ok(WorkspaceLexicalEngine::Portable),
        Ok(value) => bail!("{LEXICAL_ENGINE_ENV} must be 'zvec_rust' or 'portable', got '{value}'"),
        Err(std::env::VarError::NotPresent) => Ok(WorkspaceLexicalEngine::default()),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("{LEXICAL_ENGINE_ENV} must contain valid UTF-8")
        }
    }
}

async fn measure_async<F, Fut>(samples: usize, mut operation: F) -> Result<Latency>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let mut durations = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        operation().await?;
        durations.push(started.elapsed());
    }
    durations.sort_unstable();
    Ok(Latency {
        p50_ms: percentile_ms(&durations, 50),
        p95_ms: percentile_ms(&durations, 95),
        max_ms: elapsed_ms(*durations.last().context("no benchmark samples")?),
    })
}

fn percentile_ms(durations: &[Duration], percentile: usize) -> f64 {
    let rank = (durations.len() * percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(durations.len().saturating_sub(1));
    elapsed_ms(durations[rank])
}

fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

#[derive(Default)]
struct RerankObservation {
    input_candidates: AtomicUsize,
    evaluated_candidates: AtomicUsize,
    feature_bytes: AtomicUsize,
    accounted_scratch_bytes: AtomicUsize,
    fallbacks: AtomicUsize,
}

impl RerankObservation {
    fn observe(&self, result: &a3s_code_core::WorkspaceHybridSearchResult) -> Result<()> {
        if result.rerank.fallback.is_some() {
            self.fallbacks.fetch_add(1, Ordering::AcqRel);
        }
        self.input_candidates
            .fetch_max(result.rerank.input_candidates, Ordering::AcqRel);
        self.evaluated_candidates
            .fetch_max(result.rerank.evaluated_candidates, Ordering::AcqRel);
        self.feature_bytes
            .fetch_max(result.rerank.feature_bytes, Ordering::AcqRel);
        self.accounted_scratch_bytes
            .fetch_max(result.rerank.accounted_scratch_bytes, Ordering::AcqRel);
        Ok(())
    }
}

struct BenchmarkProvider {
    descriptor: EmbeddingProviderDescriptor,
    document_inputs: AtomicUsize,
    document_requests: AtomicUsize,
    query_inputs: AtomicUsize,
    query_requests: AtomicUsize,
}

impl BenchmarkProvider {
    fn new() -> Self {
        Self {
            descriptor: EmbeddingProviderDescriptor::new(
                "deterministic",
                "workspace-retrieval-benchmark-v5",
                DIMENSION,
            ),
            document_inputs: AtomicUsize::new(0),
            document_requests: AtomicUsize::new(0),
            query_inputs: AtomicUsize::new(0),
            query_requests: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for BenchmarkProvider {
    fn descriptor(&self) -> EmbeddingProviderDescriptor {
        self.descriptor.clone()
    }

    async fn embed(
        &self,
        request: EmbeddingBatchRequest,
        cancellation: CancellationToken,
    ) -> Result<EmbeddingBatchResponse, EmbeddingProviderError> {
        if cancellation.is_cancelled() {
            return Err(EmbeddingProviderError::Cancelled);
        }
        let query_request = request
            .inputs()
            .iter()
            .all(|input| input.id() == "workspace-query");
        let document_request = request
            .inputs()
            .iter()
            .all(|input| input.id() != "workspace-query");
        if !query_request && !document_request {
            return Err(EmbeddingProviderError::InvalidRequest);
        }
        if query_request {
            self.query_requests.fetch_add(1, Ordering::AcqRel);
        } else {
            self.document_requests.fetch_add(1, Ordering::AcqRel);
        }
        let vectors = request
            .inputs()
            .iter()
            .map(|input| {
                if input.id() == "workspace-query" {
                    self.query_inputs.fetch_add(1, Ordering::AcqRel);
                } else {
                    self.document_inputs.fetch_add(1, Ordering::AcqRel);
                }
                EmbeddingVector::new(input.id(), basis_vector(stable_slot(input.id())))
            })
            .collect();
        Ok(EmbeddingBatchResponse::new(self.descriptor(), vectors))
    }
}

struct Latency {
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

struct ExactMeasurement {
    latency: Latency,
    build_ms: f64,
    accounted_bytes: usize,
}

struct HybridMeasurement {
    latency: Latency,
    lexical_engine: WorkspaceLexicalEngine,
    workspace_build_ms: f64,
    session_construction_ms: f64,
    source_bytes: usize,
    catalog_files: usize,
    catalog_chunks: usize,
    vector_bytes: usize,
    document_embedding_inputs: usize,
    document_embedding_requests: usize,
    document_batches: usize,
    document_batch_limit_lower_bound: usize,
    document_request_amplification: f64,
    generation_complete_flushes: usize,
    time_to_first_ready_ms: Option<u64>,
    non_text_inputs: usize,
    query_embedding_inputs: usize,
    max_input_candidates: usize,
    max_evaluated_candidates: usize,
    max_feature_bytes: usize,
    max_accounted_scratch_bytes: usize,
    rerank_fallbacks: usize,
}

impl HybridMeasurement {
    fn batching_passed(&self) -> bool {
        self.document_embedding_inputs == HYBRID_RECORD_COUNT
            && self.document_batch_limit_lower_bound > 0
            && self.document_embedding_requests == self.document_batches
            && self.document_embedding_requests.saturating_mul(10)
                <= self.document_batch_limit_lower_bound.saturating_mul(11)
            && self.non_text_inputs == 0
            && self.time_to_first_ready_ms.is_some()
    }

    fn semantic_passed(&self) -> bool {
        self.catalog_files == HYBRID_FILE_COUNT
            && self.catalog_chunks == HYBRID_RECORD_COUNT
            && self.vector_bytes <= MAX_VECTOR_BYTES
    }
}
