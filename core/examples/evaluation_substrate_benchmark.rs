//! Release qualification for the provider-neutral evaluation substrate.
//!
//! The profile is deterministic and provider-free.  It measures bounded file
//! result publication, reopen/retention checks, and strict wire-envelope
//! round trips.  It deliberately excludes model, network, and Cloud latency.

use a3s_code_core::evaluation::{
    digest_bytes, EvaluationRecordV1, EvaluationResultSink, EvaluationResultV1,
    EvaluationWireEnvelopeV1, ExecutionTargetV1, FileEvaluationResultStore,
};
use serde::Serialize;
use serde_json::json;
use std::time::{Duration, Instant};

const RECORDS_PER_SAMPLE: usize = 32;
const RETAINED_RECORDS: usize = 24;
const WARMUP_SAMPLES: usize = 2;
const MEASURED_SAMPLES: usize = 10;
const WRITE_P95_BUDGET_MS: f64 = 5_000.0;
const REOPEN_P95_BUDGET_MS: f64 = 1_000.0;
const WIRE_P95_BUDGET_MS: f64 = 500.0;
const MAX_PERSISTED_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize)]
struct Latency {
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

#[derive(Debug)]
struct Sample {
    write: Duration,
    reopen: Duration,
    wire: Duration,
    retained: usize,
    persisted_bytes: u64,
    regular_files: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        anyhow::bail!("evaluation substrate qualification must run with --release");
    }

    for _ in 0..WARMUP_SAMPLES {
        run_sample().await?;
    }
    let mut samples = Vec::with_capacity(MEASURED_SAMPLES);
    for _ in 0..MEASURED_SAMPLES {
        samples.push(run_sample().await?);
    }
    let first = samples
        .first()
        .ok_or_else(|| anyhow::anyhow!("evaluation benchmark produced no samples"))?;
    if samples.iter().any(|sample| {
        sample.retained != first.retained
            || sample.persisted_bytes != first.persisted_bytes
            || sample.regular_files != first.regular_files
    }) {
        anyhow::bail!("evaluation benchmark produced nondeterministic resource counts");
    }

    let write = latency(samples.iter().map(|sample| sample.write).collect())?;
    let reopen = latency(samples.iter().map(|sample| sample.reopen).collect())?;
    let wire = latency(samples.iter().map(|sample| sample.wire).collect())?;
    let write_passed = write.p95_ms <= WRITE_P95_BUDGET_MS;
    let reopen_passed = reopen.p95_ms <= REOPEN_P95_BUDGET_MS;
    let wire_passed = wire.p95_ms <= WIRE_P95_BUDGET_MS;
    let resources_passed = first.retained == RETAINED_RECORDS
        && first.persisted_bytes <= MAX_PERSISTED_BYTES
        && first.regular_files == 2;
    let passed = write_passed && reopen_passed && wire_passed && resources_passed;

    let report = json!({
        "schemaVersion": 1,
        "profile": "evaluation-substrate-v1",
        "build": "release",
        "machine": machine_metadata(),
        "parameters": {
            "recordsPerSample": RECORDS_PER_SAMPLE,
            "retainedRecords": RETAINED_RECORDS,
            "warmupSamples": WARMUP_SAMPLES,
            "measuredSamples": MEASURED_SAMPLES,
        },
        "durableResultStore": {
            "write": latency_json(write, WRITE_P95_BUDGET_MS),
            "reopenAndValidate": latency_json(reopen, REOPEN_P95_BUDGET_MS),
            "retainedRecords": first.retained,
            "persistedBytes": first.persisted_bytes,
            "regularFiles": first.regular_files,
            "passed": write_passed && reopen_passed && resources_passed,
        },
        "wireRoundTrip": {
            "latency": latency_json(wire, WIRE_P95_BUDGET_MS),
            "envelopesPerSample": RETAINED_RECORDS,
            "strictDecodeAndDigestValidation": true,
            "passed": wire_passed,
        },
        "providerNetworkIncluded": false,
        "cloudIncluded": false,
        "passed": passed,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    if !passed {
        anyhow::bail!(
            "evaluation substrate qualification failed: write p95 {:.3} ms, reopen p95 {:.3} ms, wire p95 {:.3} ms",
            write.p95_ms,
            reopen.p95_ms,
            wire.p95_ms,
        );
    }
    Ok(())
}

async fn run_sample() -> anyhow::Result<Sample> {
    let directory = tempfile::tempdir()?;
    let store = FileEvaluationResultStore::with_max_records(directory.path(), RETAINED_RECORDS)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    let write_started = Instant::now();
    for index in 0..RECORDS_PER_SAMPLE {
        store
            .write(qualification_record(index))
            .await
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    }
    let write = write_started.elapsed();

    let reopen_started = Instant::now();
    let reopened = FileEvaluationResultStore::with_max_records(directory.path(), RETAINED_RECORDS)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let target = ExecutionTargetV1::new("evaluation-benchmark", "run-1");
    let records = reopened
        .list_for_target_checked(&target)
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    if records.len() != RETAINED_RECORDS
        || records
            .first()
            .map(|record| record.result.auxiliary_run_id.as_str())
            != Some("aux-8")
        || records
            .last()
            .map(|record| record.result.auxiliary_run_id.as_str())
            != Some("aux-31")
    {
        anyhow::bail!("durable result retention order is not FIFO");
    }
    let retained = reopened
        .validate_store()
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let reopen = reopen_started.elapsed();

    let wire_started = Instant::now();
    for record in &records {
        let envelope = EvaluationWireEnvelopeV1::from_evaluation_record(record.clone())?;
        let encoded = envelope.to_vec()?;
        let decoded = EvaluationWireEnvelopeV1::from_slice(&encoded)?;
        let round_trip: EvaluationRecordV1 = decoded
            .payload_as(a3s_code_core::evaluation::EvaluationWireKindV1::EvaluationRecord)?;
        if round_trip != *record {
            anyhow::bail!("evaluation wire round trip changed a record");
        }
    }
    let wire = wire_started.elapsed();
    let data_path = directory.path().join("evaluation-results.json");
    let persisted_bytes = tokio::fs::metadata(data_path).await?.len();
    let mut entries = tokio::fs::read_dir(directory.path()).await?;
    let mut regular_files = 0;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_file() {
            regular_files += 1;
        }
    }
    Ok(Sample {
        write,
        reopen,
        wire,
        retained,
        persisted_bytes,
        regular_files,
    })
}

fn qualification_record(index: usize) -> EvaluationRecordV1 {
    let target = ExecutionTargetV1::new("evaluation-benchmark", "run-1");
    let auxiliary = format!("aux-{index}");
    let result = EvaluationResultV1::new(
        "benchmark-evaluator",
        target,
        auxiliary.clone(),
        "host-token",
        json!({
            "index": index,
            "bounded": "evaluation substrate fixture",
        }),
        digest_bytes("evaluation-benchmark-evidence", auxiliary.as_bytes()),
    )
    .expect("benchmark fixture is valid");
    EvaluationRecordV1::new(result, u64::try_from(index + 1).expect("index fits"))
        .expect("benchmark record is valid")
}

fn latency(mut samples: Vec<Duration>) -> anyhow::Result<Latency> {
    if samples.is_empty() {
        anyhow::bail!("latency profile produced no samples");
    }
    samples.sort_unstable();
    let percentile = |fraction: f64| {
        let index = ((samples.len() - 1) as f64 * fraction).ceil() as usize;
        samples[index].as_secs_f64() * 1_000.0
    };
    Ok(Latency {
        p50_ms: percentile(0.50),
        p95_ms: percentile(0.95),
        max_ms: samples
            .last()
            .map(Duration::as_secs_f64)
            .unwrap_or_default()
            * 1_000.0,
    })
}

fn latency_json(latency: Latency, budget_p95_ms: f64) -> serde_json::Value {
    json!({
        "p50Ms": latency.p50_ms,
        "p95Ms": latency.p95_ms,
        "maxMs": latency.max_ms,
        "budgetP95Ms": budget_p95_ms,
        "passed": latency.p95_ms <= budget_p95_ms,
    })
}

fn machine_metadata() -> serde_json::Value {
    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "logicalCpus": std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1),
        "processor": std::env::var("PROCESSOR_IDENTIFIER").ok(),
    })
}
