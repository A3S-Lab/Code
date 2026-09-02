//! Locked release qualification for session-ephemeral workspace retrieval.
//!
//! Run from the Code repository root:
//!
//! `cargo run -p a3s-code-core --example workspace_retrieval_benchmark --release`

use a3s_code_core::embedding::{
    EmbeddingBatchRequest, EmbeddingBatchResponse, EmbeddingProvider, EmbeddingProviderDescriptor,
    EmbeddingProviderError, EmbeddingVector,
};
use a3s_code_core::{
    Agent, CodeConfig, SessionOptions, WorkspaceHybridSearchRequest, WorkspaceRerankMode,
    WorkspaceRerankOptions, WorkspaceRetrievalOptions, WorkspaceRetrievalPhase,
    WorkspaceSemanticIndexLimits, WorkspaceVecShadowPhase, WorkspaceVecShadowStatus,
    WorkspaceVectorEngine,
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

const RECORD_COUNT: usize = 25_000;
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
const MAX_COMBINED_BYTES: usize = 256 * 1024 * 1024;
const CHUNKS_PER_FILE: usize = 128;
const LINES_PER_CHUNK: usize = 80;

#[tokio::main]
async fn main() -> Result<()> {
    if cfg!(debug_assertions) {
        bail!("workspace retrieval qualification must run with --release");
    }

    let exact = benchmark_exact_vector_search().await?;
    let hybrid_rrf = benchmark_hybrid_search(
        WorkspaceRerankOptions::default(),
        "workspace-retrieval-qualification-rrf",
    )
    .await?;
    let hybrid = benchmark_hybrid_search(
        WorkspaceRerankOptions::deterministic(),
        "workspace-retrieval-qualification-rerank",
    )
    .await?;
    let exact_passed = exact.latency.p95_ms <= EXACT_P95_BUDGET_MS;
    let hybrid_rrf_passed =
        hybrid_rrf.latency.p95_ms <= HYBRID_P95_BUDGET_MS && hybrid_rrf.batching_passed();
    let hybrid_deterministic_passed =
        hybrid.latency.p95_ms <= HYBRID_P95_BUDGET_MS && hybrid.batching_passed();
    let hybrid_passed = hybrid_rrf_passed && hybrid_deterministic_passed;
    let rerank_p95_signed_delta_ms = hybrid.latency.p95_ms - hybrid_rrf.latency.p95_ms;
    let rerank_p95_added_ms = rerank_p95_signed_delta_ms.max(0.0);
    let rerank_passed = rerank_p95_added_ms <= RERANK_P95_DELTA_BUDGET_MS
        && hybrid.max_evaluated_candidates <= RERANK_CANDIDATE_BUDGET
        && hybrid.max_accounted_scratch_bytes <= RERANK_SCRATCH_BUDGET_BYTES
        && hybrid.rerank_fallbacks == 0;
    let report = json!({
        "schemaVersion": 4,
        "profile": "workspace-retrieval-v3",
        "build": "release",
        "machine": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "logicalCpus": std::thread::available_parallelism().map(|value| value.get()).unwrap_or(1),
            "processor": std::env::var("PROCESSOR_IDENTIFIER").ok(),
        },
        "parameters": {
            "records": RECORD_COUNT,
            "dimension": DIMENSION,
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
        "hybridRrfOnly": hybrid_json(&hybrid_rrf, hybrid_rrf_passed),
        "hybridDeterministic": hybrid_json(&hybrid, hybrid_deterministic_passed),
        "rerankComparison": {
            "rrfOnlyP95Ms": hybrid_rrf.latency.p95_ms,
            "deterministicP95Ms": hybrid.latency.p95_ms,
            "p95SignedDeltaMs": rerank_p95_signed_delta_ms,
            "p95AddedMs": rerank_p95_added_ms,
            "budgetP95DeltaMs": RERANK_P95_DELTA_BUDGET_MS,
            "maxInputCandidates": hybrid.max_input_candidates,
            "maxEvaluatedCandidates": hybrid.max_evaluated_candidates,
            "candidateBudget": RERANK_CANDIDATE_BUDGET,
            "maxFeatureBytes": hybrid.max_feature_bytes,
            "maxAccountedScratchBytes": hybrid.max_accounted_scratch_bytes,
            "scratchBudgetBytes": RERANK_SCRATCH_BUDGET_BYTES,
            "fallbacks": hybrid.rerank_fallbacks,
            "passed": rerank_passed,
        },
        "hybrid": {
            "p50Ms": hybrid.latency.p50_ms,
            "p95Ms": hybrid.latency.p95_ms,
            "maxMs": hybrid.latency.max_ms,
            "workspaceBuildMs": hybrid.workspace_build_ms,
            "sessionConstructionMs": hybrid.session_construction_ms,
            "sourceBytes": hybrid.source_bytes,
            "vectorBytes": hybrid.vector_bytes,
            "documentEmbeddingInputs": hybrid.document_embedding_inputs,
            "documentEmbeddingRequests": hybrid.document_embedding_requests,
            "documentBatchLimitLowerBound": hybrid.document_batch_limit_lower_bound,
            "documentRequestAmplification": hybrid.document_request_amplification,
            "queryEmbeddingInputs": hybrid.query_embedding_inputs,
            "providerNetworkIncluded": false,
            "authoritativeSourceReads": "included from the warm OS cache",
            "budgetP95Ms": HYBRID_P95_BUDGET_MS,
            "passed": hybrid_passed,
        },
        "resourceCeilings": {
            "vectorBytes": MAX_VECTOR_BYTES,
            "catalogLexicalAndVectorBytes": MAX_COMBINED_BYTES,
        },
        "passed": exact_passed && hybrid_passed && rerank_passed,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    if !exact_passed || !hybrid_passed || !rerank_passed {
        bail!(
            "workspace retrieval qualification failed: exact p95 {:.3} ms, RRF p95 {:.3} ms, deterministic p95 {:.3} ms, rerank delta {:.3} ms",
            exact.latency.p95_ms,
            hybrid_rrf.latency.p95_ms,
            hybrid.latency.p95_ms,
            rerank_p95_added_ms,
        );
    }
    Ok(())
}

async fn benchmark_exact_vector_search() -> Result<ExactMeasurement> {
    let descriptor = VectorIndexDescriptor::new(DIMENSION)
        .with_max_records(RECORD_COUNT)
        .with_max_bytes(MAX_VECTOR_BYTES);
    let index = InMemoryVectorIndex::new(descriptor)?;
    let records = (0..RECORD_COUNT)
        .map(|index| VectorRecord::new(format!("record-{index:05}"), basis_vector(index)))
        .collect();
    let build_started = Instant::now();
    let status = index.replace_partition("qualification", records).await?;
    let build_ms = elapsed_ms(build_started.elapsed());
    if status.record_count != RECORD_COUNT {
        bail!(
            "exact index admitted {} records instead of {RECORD_COUNT}",
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

fn hybrid_json(measurement: &HybridMeasurement, passed: bool) -> serde_json::Value {
    json!({
        "p50Ms": measurement.latency.p50_ms,
        "p95Ms": measurement.latency.p95_ms,
        "maxMs": measurement.latency.max_ms,
        "workspaceBuildMs": measurement.workspace_build_ms,
        "sessionConstructionMs": measurement.session_construction_ms,
        "sourceBytes": measurement.source_bytes,
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
        "activeVectorEngine": measurement.active_vector_engine,
        "vecShadow": {
            "phase": measurement.vec_shadow.phase,
            "revision": measurement.vec_shadow.revision,
            "recordCount": measurement.vec_shadow.record_count,
            "accountedBytes": measurement.vec_shadow.accounted_bytes,
            "successfulMutations": measurement.vec_shadow.successful_mutations,
            "comparedQueries": measurement.vec_shadow.compared_queries,
            "matchingQueries": measurement.vec_shadow.matching_queries,
            "initializationFailures": measurement.vec_shadow.initialization_failures,
            "failedMutations": measurement.vec_shadow.failed_mutations,
            "mismatchedQueries": measurement.vec_shadow.mismatched_queries,
            "failedQueries": measurement.vec_shadow.failed_queries,
            "passed": measurement.migration_passed(),
        },
        "providerNetworkIncluded": false,
        "authoritativeSourceReads": "included from the warm OS cache",
        "budgetP95Ms": HYBRID_P95_BUDGET_MS,
        "passed": passed,
    })
}

async fn benchmark_hybrid_search(
    rerank: WorkspaceRerankOptions,
    session_id: &str,
) -> Result<HybridMeasurement> {
    let workspace = tempfile::tempdir()?;
    let source_bytes = write_workspace(workspace.path())?;
    let provider = Arc::new(BenchmarkProvider::new());
    let provider_port: Arc<dyn EmbeddingProvider> = provider.clone();
    let retrieval = WorkspaceRetrievalOptions::new(provider_port)
        .with_index_limits(WorkspaceSemanticIndexLimits {
            max_records: RECORD_COUNT,
            max_bytes: MAX_VECTOR_BYTES,
            shutdown_timeout: Duration::from_secs(5),
        })
        .with_rerank_options(rerank);
    let config = benchmark_code_config()?;
    let agent = Agent::from_config(config).await?;
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
            if status.phase == WorkspaceRetrievalPhase::Ready
                && status.vector_records == RECORD_COUNT
            {
                return Ok(status);
            }
            if status.phase == WorkspaceRetrievalPhase::Degraded {
                bail!("semantic workspace build degraded: {status:?}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("semantic workspace build exceeded 120 seconds")??;
    let workspace_build_ms = elapsed_ms(session_started.elapsed());
    if provider.document_inputs.load(Ordering::Acquire) != RECORD_COUNT {
        bail!(
            "provider received {} document inputs instead of {RECORD_COUNT}",
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

    let request = WorkspaceHybridSearchRequest::new("x").with_limit(TOP_K);
    let rerank_observation = RerankObservation::default();
    for _ in 0..WARMUP_SAMPLES {
        let result = session.hybrid_search(request.clone()).await?;
        if result.hits.is_empty() {
            bail!("hybrid warmup returned no verified hits");
        }
        rerank_observation.observe(&result, rerank.mode)?;
    }
    let document_inputs_before_measurement = provider.document_inputs.load(Ordering::Acquire);
    let latency = measure_async(MEASURED_SAMPLES, || async {
        let result = session.hybrid_search(request.clone()).await?;
        if result.hits.is_empty() {
            bail!("hybrid sample returned no verified hits");
        }
        rerank_observation.observe(&result, rerank.mode)?;
        Ok(())
    })
    .await?;
    let document_embedding_inputs = provider.document_inputs.load(Ordering::Acquire);
    if document_embedding_inputs != document_inputs_before_measurement {
        bail!("repeated hybrid queries re-embedded unchanged source chunks");
    }
    let query_embedding_inputs = provider.query_inputs.load(Ordering::Acquire);
    if query_embedding_inputs != WARMUP_SAMPLES + MEASURED_SAMPLES {
        bail!(
            "provider received {query_embedding_inputs} query inputs instead of {}",
            WARMUP_SAMPLES + MEASURED_SAMPLES
        );
    }
    let query_embedding_requests = provider.query_requests.load(Ordering::Acquire);
    if query_embedding_requests != WARMUP_SAMPLES + MEASURED_SAMPLES {
        bail!(
            "provider received {query_embedding_requests} query requests instead of {}",
            WARMUP_SAMPLES + MEASURED_SAMPLES
        );
    }
    let migration = session.workspace_retrieval_status();
    let expected_queries = (WARMUP_SAMPLES + MEASURED_SAMPLES) as u64;
    if migration.active_vector_engine != Some(WorkspaceVectorEngine::A3sMemory)
        || migration.vec_shadow.phase != WorkspaceVecShadowPhase::Ready
        || migration.vec_shadow.record_count != RECORD_COUNT
        || migration.vec_shadow.compared_queries != expected_queries
        || migration.vec_shadow.matching_queries != expected_queries
        || migration.vec_shadow.initialization_failures != 0
        || migration.vec_shadow.failed_mutations != 0
        || migration.vec_shadow.mismatched_queries != 0
        || migration.vec_shadow.failed_queries != 0
    {
        bail!("A3S Vec migration shadow failed the Memory-oracle qualification: {migration:?}");
    }
    let vector_bytes = status.vector_bytes;
    session.close().await;
    let closed = session.workspace_retrieval_status();
    if closed.phase != WorkspaceRetrievalPhase::Closed
        || closed.vector_records != 0
        || closed.vector_bytes != 0
        || closed.vec_shadow.phase != WorkspaceVecShadowPhase::Closed
        || closed.vec_shadow.record_count != 0
        || closed.vec_shadow.accounted_bytes != 0
    {
        bail!("session retained semantic vector state after close");
    }

    Ok(HybridMeasurement {
        latency,
        workspace_build_ms,
        session_construction_ms,
        source_bytes,
        vector_bytes,
        document_embedding_inputs,
        document_embedding_requests,
        document_batches: status.batching.document_batches,
        document_batch_limit_lower_bound,
        document_request_amplification,
        generation_complete_flushes: status.batching.generation_complete_flushes,
        time_to_first_ready_ms: status.batching.time_to_first_ready_ms,
        non_text_inputs: status.batching.non_text_inputs,
        query_embedding_inputs,
        active_vector_engine: migration.active_vector_engine,
        vec_shadow: migration.vec_shadow,
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

#[derive(Default)]
struct RerankObservation {
    input_candidates: AtomicUsize,
    evaluated_candidates: AtomicUsize,
    feature_bytes: AtomicUsize,
    accounted_scratch_bytes: AtomicUsize,
    fallbacks: AtomicUsize,
}

impl RerankObservation {
    fn observe(
        &self,
        result: &a3s_code_core::WorkspaceHybridSearchResult,
        requested_mode: WorkspaceRerankMode,
    ) -> Result<()> {
        if result.rerank.requested_mode != requested_mode {
            bail!("hybrid result reported an unexpected requested rerank mode");
        }
        if result.rerank.applied_mode != requested_mode {
            bail!("hybrid result did not apply the requested bounded reranker");
        }
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

fn benchmark_code_config() -> Result<CodeConfig> {
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

fn write_workspace(root: &std::path::Path) -> Result<usize> {
    let mut remaining = RECORD_COUNT;
    let mut file_index = 0usize;
    let mut source_bytes = 0usize;
    while remaining > 0 {
        let chunks = remaining.min(CHUNKS_PER_FILE);
        let content = "x\n".repeat(chunks * LINES_PER_CHUNK);
        source_bytes = source_bytes
            .checked_add(content.len())
            .context("workspace source byte count overflowed")?;
        std::fs::write(root.join(format!("bench-{file_index:04}.rs")), content)?;
        remaining -= chunks;
        file_index += 1;
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
        .saturating_sub(1);
    elapsed_ms(durations[rank.min(durations.len() - 1)])
}

fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
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
                "workspace-retrieval-benchmark-v1",
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
                let mut values = vec![0.0; DIMENSION];
                values[stable_slot(input.id())] = 1.0;
                EmbeddingVector::new(input.id(), values)
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
    workspace_build_ms: f64,
    session_construction_ms: f64,
    source_bytes: usize,
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
    active_vector_engine: Option<WorkspaceVectorEngine>,
    vec_shadow: WorkspaceVecShadowStatus,
    max_input_candidates: usize,
    max_evaluated_candidates: usize,
    max_feature_bytes: usize,
    max_accounted_scratch_bytes: usize,
    rerank_fallbacks: usize,
}

impl HybridMeasurement {
    fn batching_passed(&self) -> bool {
        self.document_batch_limit_lower_bound > 0
            && self.document_embedding_requests.saturating_mul(10)
                <= self.document_batch_limit_lower_bound.saturating_mul(11)
            && self.document_embedding_requests == self.document_batches
            && self.non_text_inputs == 0
            && self.time_to_first_ready_ms.is_some()
            && self.migration_passed()
    }

    fn migration_passed(&self) -> bool {
        let expected_queries = (WARMUP_SAMPLES + MEASURED_SAMPLES) as u64;
        self.active_vector_engine == Some(WorkspaceVectorEngine::A3sMemory)
            && self.vec_shadow.phase == WorkspaceVecShadowPhase::Ready
            && self.vec_shadow.record_count == RECORD_COUNT
            && self.vec_shadow.successful_mutations > 0
            && self.vec_shadow.compared_queries == expected_queries
            && self.vec_shadow.matching_queries == expected_queries
            && self.vec_shadow.initialization_failures == 0
            && self.vec_shadow.failed_mutations == 0
            && self.vec_shadow.mismatched_queries == 0
            && self.vec_shadow.failed_queries == 0
    }
}
