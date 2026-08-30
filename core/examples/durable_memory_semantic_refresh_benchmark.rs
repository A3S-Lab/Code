//! Release qualification for durable semantic refresh and local SQLite recovery.
//!
//! The profile uses a deterministic in-process embedding adapter so provider
//! network and billing are deliberately excluded. It gates Code-owned work
//! amplification over durable source and vector backends, then measures warm
//! semantic recall after a real close/reopen boundary.

#[path = "durable_memory_semantic_refresh_benchmark/fixture.rs"]
mod fixture;
#[path = "durable_memory_semantic_refresh_benchmark/measurement.rs"]
mod measurement;
#[path = "durable_memory_semantic_refresh_benchmark/runtime.rs"]
mod runtime;

use a3s_code_core::memory::{ScheduledSemanticRefresh, SemanticRefreshRunOutcome};
use a3s_code_core::DurableMemorySemanticRefreshCheckpoint;
use a3s_memory::repository::{FileMemoryRepository, MemoryNamespace};
use a3s_memory::vector::{SqliteVectorIndex, VectorIndex, VectorIndexDescriptor};
use anyhow::{bail, Context, Result};
use fixture::{revise_source_node, seed_repository, QualificationProvider};
use measurement::{
    duration_ms, latency, machine_metadata, max_rss, resident_set_bytes, rss_delta,
    rss_within_budget, ExpectedBytes, PhaseEvidence,
};
use runtime::{
    durable_session, epoch_summary_matches, expected_work, file_tree_stats, persist_checkpoint,
    start_runtime, validate_recall, wait_for_run,
};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

const CORPUS_RECORDS: usize = 10_000;
const DIMENSION: usize = 384;
const CANDIDATE_LIMIT: usize = 8;
const EMBEDDING_BATCH_INPUTS: usize = 64;
const VECTOR_MAX_BYTES: usize = 64 * 1024 * 1024;
const SOURCE_DISK_BUDGET_BYTES: u64 = 64 * 1024 * 1024;
const VECTOR_DISK_BUDGET_BYTES: u64 = 128 * 1024 * 1024;
const TOTAL_DURABLE_DISK_BUDGET_BYTES: u64 = 193 * 1024 * 1024;
const ACTIVE_RSS_DELTA_BUDGET_BYTES: u64 = 768 * 1024 * 1024;
const RETAINED_RSS_DELTA_BUDGET_BYTES: u64 = 384 * 1024 * 1024;
const QUERY_WARMUP_SAMPLES: usize = 3;
const QUERY_MEASURED_SAMPLES: usize = 20;
const QUERY_P95_BUDGET_MS: f64 = 1_000.0;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const RUN_TIMEOUT: Duration = Duration::from_secs(120);

#[tokio::main]
async fn main() -> Result<()> {
    if cfg!(debug_assertions) {
        bail!("durable semantic refresh qualification must run with --release");
    }

    let rss_before = resident_set_bytes();
    let root = tempfile::tempdir().context("could not create qualification directory")?;
    let source_root = root.path().join("source");
    let vector_root = root.path().join("vector");
    let vector_path = vector_root.join("semantic.sqlite3");
    let checkpoint_path = root.path().join("semantic-refresh-checkpoint.json");
    let namespace = MemoryNamespace::try_new(
        "qualification-tenant",
        "qualification-principal",
        "semantic-refresh-release-v1",
    )?;

    let repository = Arc::new(FileMemoryRepository::open(&source_root).await?);
    let seed_started = Instant::now();
    let corpus = seed_repository(repository.as_ref(), &namespace, CORPUS_RECORDS).await?;
    let seed_ms = duration_ms(seed_started.elapsed());
    let seed_change_sets = corpus.change_sets;
    let provider = Arc::new(QualificationProvider::new(
        DIMENSION,
        corpus.target_content.clone(),
    ));
    let descriptor = VectorIndexDescriptor::new(DIMENSION)
        .with_max_records(CORPUS_RECORDS)
        .with_max_bytes(VECTOR_MAX_BYTES);
    let index = Arc::new(SqliteVectorIndex::open(&vector_path, descriptor.clone()).await?);
    let durable = durable_session(
        repository.clone(),
        namespace.clone(),
        provider.clone(),
        index.clone(),
    )?;
    let first_schedule = ScheduledSemanticRefresh::try_new(REFRESH_INTERVAL)?;
    let first_runtime = start_runtime(
        "semantic-refresh-release-first",
        durable.clone(),
        first_schedule.clone(),
    )?;

    let records = u64::try_from(CORPUS_RECORDS)?;
    let corpus_bytes = u64::try_from(corpus.content_bytes)?;
    let initial_provider_requests = u64::try_from(CORPUS_RECORDS.div_ceil(EMBEDDING_BATCH_INPUTS))?;
    let initial = wait_for_run(&first_schedule, 1).await?;
    let mut phases = vec![PhaseEvidence::evaluate(
        "initial_publication",
        &initial,
        expected_work(
            1,
            SemanticRefreshRunOutcome::Published,
            3,
            1,
            records,
            ExpectedBytes::Positive,
            0,
            records,
            corpus_bytes,
            initial_provider_requests,
            records,
            corpus_bytes,
            1,
            records,
        ),
    )];

    let stable = wait_for_run(&first_schedule, 2).await?;
    phases.push(PhaseEvidence::evaluate(
        "stable_token_fast_path",
        &stable,
        expected_work(
            2,
            SemanticRefreshRunOutcome::Unchanged,
            1,
            0,
            0,
            ExpectedBytes::Zero,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ),
    ));

    let revised_content = revise_source_node(repository.as_ref(), &namespace).await?;
    let revised_bytes = u64::try_from(revised_content.len())?;
    let source_drift = wait_for_run(&first_schedule, 3).await?;
    phases.push(PhaseEvidence::evaluate(
        "single_node_source_drift",
        &source_drift,
        expected_work(
            3,
            SemanticRefreshRunOutcome::Published,
            3,
            1,
            records,
            ExpectedBytes::Positive,
            records - 1,
            1,
            revised_bytes,
            1,
            1,
            revised_bytes,
            1,
            records,
        ),
    ));

    let cleared = index.clear().await?;
    if cleared.record_count != 0 || cleared.byte_count != 0 {
        bail!("SQLite index clear retained records or accounted bytes");
    }
    let index_drift = wait_for_run(&first_schedule, 4).await?;
    phases.push(PhaseEvidence::evaluate(
        "index_only_drift",
        &index_drift,
        expected_work(
            4,
            SemanticRefreshRunOutcome::Published,
            3,
            1,
            records,
            ExpectedBytes::Positive,
            records,
            0,
            0,
            0,
            0,
            0,
            1,
            records,
        ),
    ));

    let first_epoch_metrics = first_schedule.metrics();
    let first_observation = index.observe().await?;
    let receipt = first_schedule
        .last_receipt()
        .context("first ownership epoch did not retain a receipt")?;
    let checkpoint = receipt.checkpoint();
    let checkpoint_json = serde_json::to_vec_pretty(&checkpoint)?;
    let checkpoint_text = std::str::from_utf8(&checkpoint_json)?;
    let checkpoint_secret_free = !checkpoint_text.contains(&corpus.target_content)
        && !checkpoint_text.contains(&corpus.target_id)
        && !checkpoint_text.contains("sourceChangeToken");
    persist_checkpoint(&checkpoint_path, &checkpoint_json).await?;
    let rss_first_epoch = resident_set_bytes();
    let first_close = first_runtime.close().await;
    drop(first_runtime);
    drop(durable);
    drop(index);
    drop(repository);

    let reopen_started = Instant::now();
    let repository = Arc::new(FileMemoryRepository::open(&source_root).await?);
    let index = Arc::new(SqliteVectorIndex::open(&vector_path, descriptor.clone()).await?);
    let reopened_observation = index.observe().await?;
    let decoded_checkpoint: DurableMemorySemanticRefreshCheckpoint =
        serde_json::from_slice(&tokio::fs::read(&checkpoint_path).await?)?;
    let reopen_ms = duration_ms(reopen_started.elapsed());
    let reopen_continuity_passed = decoded_checkpoint == checkpoint
        && reopened_observation == first_observation
        && reopened_observation.status.record_count == CORPUS_RECORDS;

    let durable = durable_session(
        repository.clone(),
        namespace,
        provider.clone(),
        index.clone(),
    )?;
    let recovered_schedule =
        ScheduledSemanticRefresh::try_new_with_checkpoint(REFRESH_INTERVAL, decoded_checkpoint)?;
    let recovered_runtime = start_runtime(
        "semantic-refresh-release-recovered",
        durable.clone(),
        recovered_schedule.clone(),
    )?;

    let recovery = wait_for_run(&recovered_schedule, 1).await?;
    phases.push(PhaseEvidence::evaluate(
        "checkpoint_recovery",
        &recovery,
        expected_work(
            1,
            SemanticRefreshRunOutcome::Unchanged,
            2,
            1,
            records,
            ExpectedBytes::Positive,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ),
    ));
    let recovered_stable = wait_for_run(&recovered_schedule, 2).await?;
    phases.push(PhaseEvidence::evaluate(
        "recovered_stable_token_fast_path",
        &recovered_stable,
        expected_work(
            2,
            SemanticRefreshRunOutcome::Unchanged,
            1,
            0,
            0,
            ExpectedBytes::Zero,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ),
    ));
    let recovered_metrics = recovered_schedule.metrics();
    let second_close = recovered_runtime.close().await;
    drop(recovered_runtime);

    let provider_calls_before_query = provider.calls();
    let provider_inputs_before_query = provider.inputs();
    for _ in 0..QUERY_WARMUP_SAMPLES {
        validate_recall(&durable).await?;
    }
    let mut query_samples = Vec::with_capacity(QUERY_MEASURED_SAMPLES);
    for _ in 0..QUERY_MEASURED_SAMPLES {
        let started = Instant::now();
        validate_recall(&durable).await?;
        query_samples.push(started.elapsed());
    }
    let query_latency = latency(query_samples)?;
    let provider_calls_after_query = provider.calls();
    let provider_inputs_after_query = provider.inputs();
    let final_observation = index.observe().await?;
    let rss_recovered = resident_set_bytes();
    let rss_active = max_rss(rss_first_epoch, rss_recovered);

    let expected_refresh_provider_calls = usize::try_from(initial_provider_requests)? + 1;
    let expected_query_calls = QUERY_WARMUP_SAMPLES + QUERY_MEASURED_SAMPLES;
    let provider_boundary_passed = provider_calls_before_query == expected_refresh_provider_calls
        && provider_inputs_before_query == CORPUS_RECORDS + 1
        && provider_calls_after_query == provider_calls_before_query + expected_query_calls
        && provider_inputs_after_query == provider_inputs_before_query + expected_query_calls;
    let first_epoch_passed = epoch_summary_matches(&first_epoch_metrics, 4, 3, 1);
    let recovered_epoch_passed = epoch_summary_matches(&recovered_metrics, 2, 0, 2);
    let phases_passed = phases.iter().all(|phase| phase.passed);
    let query_passed = query_latency.p95_ms <= QUERY_P95_BUDGET_MS;
    let close_passed = first_close.is_clean() && second_close.is_clean();
    let index_passed = final_observation == first_observation
        && final_observation.status.record_count == CORPUS_RECORDS
        && final_observation.status.byte_count <= VECTOR_MAX_BYTES;

    drop(durable);
    drop(index);
    drop(repository);
    drop(provider);
    drop(corpus);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let rss_after_drop = resident_set_bytes();
    let active_rss_delta = rss_delta(rss_before, rss_active);
    let retained_rss_delta = rss_delta(rss_before, rss_after_drop);
    let rss_passed = rss_within_budget(active_rss_delta, ACTIVE_RSS_DELTA_BUDGET_BYTES)
        && rss_within_budget(retained_rss_delta, RETAINED_RSS_DELTA_BUDGET_BYTES);
    let (source_file_count, source_disk_bytes) = file_tree_stats(&source_root)?;
    let (vector_file_count, vector_disk_bytes) = file_tree_stats(&vector_root)?;
    let (total_file_count, total_disk_bytes) = file_tree_stats(root.path())?;
    let disk_passed = source_disk_bytes <= SOURCE_DISK_BUDGET_BYTES
        && vector_disk_bytes <= VECTOR_DISK_BUDGET_BYTES
        && total_disk_bytes <= TOTAL_DURABLE_DISK_BUDGET_BYTES;
    let passed = phases_passed
        && first_epoch_passed
        && recovered_epoch_passed
        && checkpoint_secret_free
        && reopen_continuity_passed
        && provider_boundary_passed
        && close_passed
        && index_passed
        && query_passed
        && rss_passed
        && disk_passed;

    let report = json!({
        "schemaVersion": 1,
        "profile": "durable-memory-semantic-refresh-sqlite-v1",
        "build": "release",
        "machine": machine_metadata(),
        "parameters": {
            "activeRecords": CORPUS_RECORDS,
            "embeddingDimension": DIMENSION,
            "embeddingBatchInputs": EMBEDDING_BATCH_INPUTS,
            "candidateLimit": CANDIDATE_LIMIT,
            "refreshIntervalMs": REFRESH_INTERVAL.as_millis(),
            "queryWarmupSamples": QUERY_WARMUP_SAMPLES,
            "queryMeasuredSamples": QUERY_MEASURED_SAMPLES,
        },
        "sourceRepository": {
            "backend": "a3s-memory/FileMemoryRepository",
            "seedChangeSets": seed_change_sets,
            "contentBytes": corpus_bytes,
            "seedObservedMs": seed_ms,
            "reopenObservedMs": reopen_ms,
            "regularFiles": source_file_count,
            "diskBytes": source_disk_bytes,
            "diskBudgetBytes": SOURCE_DISK_BUDGET_BYTES,
            "filesystemAndFsyncIncluded": true,
            "passed": source_disk_bytes <= SOURCE_DISK_BUDGET_BYTES,
        },
        "semanticRefresh": {
            "scheduler": "MemoryMaintenanceRuntime/ScheduledSemanticRefresh",
            "phases": phases,
            "firstEpochMetrics": first_epoch_metrics,
            "recoveredEpochMetrics": recovered_metrics,
            "firstEpochSummaryPassed": first_epoch_passed,
            "recoveredEpochSummaryPassed": recovered_epoch_passed,
            "workAmplificationPassed": phases_passed,
            "passed": phases_passed && first_epoch_passed && recovered_epoch_passed,
        },
        "checkpointRecovery": {
            "checkpointBytes": checkpoint_json.len(),
            "checkpointSyncedBeforeClose": true,
            "secretFree": checkpoint_secret_free,
            "sourceRepositoryReopened": true,
            "sqliteIndexReopened": true,
            "operatingSystemProcessRestartIncluded": false,
            "continuityPassed": reopen_continuity_passed,
            "firstClose": first_close,
            "secondClose": second_close,
            "closePassed": close_passed,
            "passed": checkpoint_secret_free && reopen_continuity_passed && close_passed,
        },
        "vectorIndex": {
            "backend": "a3s-memory/SqliteVectorIndex",
            "mutationConsistency": "index_revision_cas",
            "finalObservation": final_observation,
            "logicalByteBudget": VECTOR_MAX_BYTES,
            "regularFiles": vector_file_count,
            "diskBytes": vector_disk_bytes,
            "diskBudgetBytes": VECTOR_DISK_BUDGET_BYTES,
            "passed": index_passed && vector_disk_bytes <= VECTOR_DISK_BUDGET_BYTES,
        },
        "semanticRecall": {
            "codeBoundary": "DurableMemorySession::preview_recall",
            "targetRankedFirst": true,
            "retrievalChannel": "semantic",
            "p50Ms": query_latency.p50_ms,
            "p95Ms": query_latency.p95_ms,
            "maxMs": query_latency.max_ms,
            "budgetP95Ms": QUERY_P95_BUDGET_MS,
            "providerNetworkIncluded": false,
            "sourceReads": "FileMemoryRepository replay view in memory",
            "vectorReads": "SQLite from the warm operating-system cache",
            "passed": query_passed,
        },
        "providerBoundary": {
            "adapter": "deterministic in-process fixture",
            "refreshCallsBeforeQuery": provider_calls_before_query,
            "refreshInputsBeforeQuery": provider_inputs_before_query,
            "queryCalls": provider_calls_after_query - provider_calls_before_query,
            "queryInputs": provider_inputs_after_query - provider_inputs_before_query,
            "remoteTransmissionIncluded": false,
            "billingIncluded": false,
            "passed": provider_boundary_passed,
        },
        "durableStorage": {
            "regularFiles": total_file_count,
            "diskBytes": total_disk_bytes,
            "diskBudgetBytes": TOTAL_DURABLE_DISK_BUDGET_BYTES,
            "includesCheckpoint": true,
            "passed": disk_passed,
        },
        "resources": {
            "rssBeforeBytes": rss_before,
            "rssActiveBytes": rss_active,
            "rssAfterDropBytes": rss_after_drop,
            "activeRssDeltaBytes": active_rss_delta,
            "activeRssDeltaBudgetBytes": ACTIVE_RSS_DELTA_BUDGET_BYTES,
            "retainedRssDeltaBytes": retained_rss_delta,
            "retainedRssDeltaBudgetBytes": RETAINED_RSS_DELTA_BUDGET_BYTES,
            "linuxRssRequired": cfg!(target_os = "linux"),
            "passed": rss_passed,
        },
        "qualificationBoundary": {
            "realEmbeddingModelIncluded": false,
            "remoteVectorBackendIncluded": false,
            "distributedLeaseIncluded": false,
            "remoteFailoverIncluded": false,
            "longHorizonConsolidationIncluded": false,
        },
        "passed": passed,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    if !passed {
        bail!(
            "durable semantic refresh qualification failed: work={}, recovery={}, query p95={:.3} ms, rss={}, disk={}",
            phases_passed,
            reopen_continuity_passed,
            query_latency.p95_ms,
            rss_passed,
            disk_passed,
        );
    }
    Ok(())
}
