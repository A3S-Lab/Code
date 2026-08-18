//! Release qualification for Code-owned session persistence backends.
//!
//! This profile repeatedly replaces and loads the same approximately 1 MiB
//! aggregate snapshot. It verifies percentile latency, atomic overwrite without
//! file accumulation, byte fidelity, and delete cleanup.

use a3s_code_core::config::StorageBackend;
use a3s_code_core::store::{
    ContextUsage, FileSessionStore, MemorySessionStore, SessionConfig, SessionData,
    SessionSnapshotV1, SessionState, SessionStore,
};
use a3s_code_core::{Message, TokenUsage};
use serde::Serialize;
use serde_json::json;
use std::path::Path;
use std::time::{Duration, Instant};

const SNAPSHOT_MIN_BYTES: usize = 1024 * 1024;
const SNAPSHOT_MAX_BYTES: usize = 2 * 1024 * 1024;
const WARMUP_SAMPLES: usize = 3;
const MEASURED_SAMPLES: usize = 20;
const MEMORY_SAVE_P95_BUDGET_MS: f64 = 250.0;
const MEMORY_LOAD_P95_BUDGET_MS: f64 = 250.0;
const FILE_SAVE_P95_BUDGET_MS: f64 = 1_000.0;
const FILE_LOAD_P95_BUDGET_MS: f64 = 500.0;

#[derive(Clone, Copy, Debug, Serialize)]
struct Latency {
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

#[derive(Debug)]
struct StoreMeasurements {
    save: Latency,
    load: Latency,
    list_count: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        anyhow::bail!("persistence qualification must run with --release");
    }

    let snapshot = benchmark_snapshot();
    snapshot.validate_for_session(&snapshot.session.id)?;
    let compact_bytes = serde_json::to_vec(&snapshot)?.len();
    let persisted_bytes = serde_json::to_vec_pretty(&snapshot)?.len();
    if !(SNAPSHOT_MIN_BYTES..=SNAPSHOT_MAX_BYTES).contains(&compact_bytes) {
        anyhow::bail!(
            "benchmark snapshot must be between 1 and 2 MiB, observed {compact_bytes} bytes"
        );
    }

    let memory_store = MemorySessionStore::new();
    let memory = measure_store(&memory_store, &snapshot, compact_bytes).await?;
    memory_store.delete(&snapshot.session.id).await?;
    let memory_cleanup_passed =
        !memory_store.exists(&snapshot.session.id).await? && memory_store.list().await?.is_empty();

    let file_root = tempfile::tempdir()?;
    let file_store = FileSessionStore::new(file_root.path()).await?;
    let file = measure_store(&file_store, &snapshot, compact_bytes).await?;
    let (file_count_before_delete, file_bytes_before_delete) = file_tree_stats(file_root.path())?;
    let overwrite_passed = file_count_before_delete == 1
        && file_bytes_before_delete == u64::try_from(persisted_bytes)?;
    file_store.delete(&snapshot.session.id).await?;
    let (file_count_after_delete, file_bytes_after_delete) = file_tree_stats(file_root.path())?;
    let file_cleanup_passed = !file_store.exists(&snapshot.session.id).await?
        && file_store.list().await?.is_empty()
        && file_count_after_delete == 0
        && file_bytes_after_delete == 0;

    let memory_passed = memory.save.p95_ms <= MEMORY_SAVE_P95_BUDGET_MS
        && memory.load.p95_ms <= MEMORY_LOAD_P95_BUDGET_MS
        && memory.list_count == 1
        && memory_cleanup_passed;
    let file_passed = file.save.p95_ms <= FILE_SAVE_P95_BUDGET_MS
        && file.load.p95_ms <= FILE_LOAD_P95_BUDGET_MS
        && file.list_count == 1
        && overwrite_passed
        && file_cleanup_passed;
    let passed = memory_passed && file_passed;

    let report = json!({
        "schemaVersion": 1,
        "profile": "session-persistence-backends-v1",
        "build": "release",
        "machine": machine_metadata(),
        "parameters": {
            "compactSnapshotBytes": compact_bytes,
            "persistedPrettyJsonBytes": persisted_bytes,
            "messages": snapshot.session.messages.len(),
            "warmupSamples": WARMUP_SAMPLES,
            "measuredSamples": MEASURED_SAMPLES,
            "overwriteGenerations": WARMUP_SAMPLES + MEASURED_SAMPLES,
        },
        "memorySessionStore": {
            "save": latency_json(memory.save, MEMORY_SAVE_P95_BUDGET_MS),
            "load": latency_json(memory.load, MEMORY_LOAD_P95_BUDGET_MS),
            "sessionCountAfterOverwrite": memory.list_count,
            "deleteCleanupPassed": memory_cleanup_passed,
            "passed": memory_passed,
        },
        "fileSessionStore": {
            "save": latency_json(file.save, FILE_SAVE_P95_BUDGET_MS),
            "load": latency_json(file.load, FILE_LOAD_P95_BUDGET_MS),
            "sessionCountAfterOverwrite": file.list_count,
            "regularFilesAfterOverwrite": file_count_before_delete,
            "bytesAfterOverwrite": file_bytes_before_delete,
            "regularFilesAfterDelete": file_count_after_delete,
            "bytesAfterDelete": file_bytes_after_delete,
            "overwriteWithoutAccumulationPassed": overwrite_passed,
            "deleteCleanupPassed": file_cleanup_passed,
            "filesystemAndFsyncIncluded": true,
            "passed": file_passed,
        },
        "providerNetworkIncluded": false,
        "passed": passed,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    if !passed {
        anyhow::bail!(
            "persistence qualification failed: memory save/load p95 {:.3}/{:.3} ms, file save/load p95 {:.3}/{:.3} ms",
            memory.save.p95_ms,
            memory.load.p95_ms,
            file.save.p95_ms,
            file.load.p95_ms,
        );
    }
    Ok(())
}

fn benchmark_snapshot() -> SessionSnapshotV1 {
    let payload = format!(
        "{}\n",
        "A3S deterministic persisted conversation payload. ".repeat(96)
    );
    let mut messages = Vec::with_capacity(256);
    for index in 0..256 {
        let text = format!("message-{index:03}\n{payload}");
        if index % 2 == 0 {
            messages.push(Message::user(&text));
        } else {
            messages.push(Message::assistant(&text));
        }
    }

    let mut config = SessionConfig::default();
    config.name = "Persistence benchmark".to_owned();
    config.workspace = "/controlled/persistence-benchmark".to_owned();
    config.system_prompt = Some("Persist one complete deterministic generation.".to_owned());
    config.max_context_length = 200_000;
    config.storage_type = StorageBackend::File;

    SessionSnapshotV1::session_only(SessionData {
        id: "persistence-performance-v1".to_owned(),
        config,
        state: SessionState::Active,
        messages,
        context_usage: ContextUsage {
            used_tokens: 16_384,
            max_tokens: 200_000,
            percent: 0.08192,
            turns: 256,
        },
        total_usage: TokenUsage {
            prompt_tokens: 12_000,
            completion_tokens: 4_384,
            total_tokens: 16_384,
            cache_read_tokens: Some(2_000),
            cache_write_tokens: Some(1_000),
        },
        total_cost: 0.0,
        model_name: Some("controlled/fixture".to_owned()),
        cost_records: Vec::new(),
        tool_names: vec!["read".to_owned(), "search".to_owned()],
        thinking_enabled: false,
        thinking_budget: None,
        created_at: 1_700_000_000,
        updated_at: 1_700_000_100,
        llm_config: None,
        tasks: Vec::new(),
        parent_id: None,
        tenant_id: Some("qualification".to_owned()),
        principal: Some("performance-workflow".to_owned()),
        agent_template_id: Some("persistence-v1".to_owned()),
        correlation_id: Some("persistence-performance-v1".to_owned()),
        cognitive_package_binding: None,
    })
}

async fn measure_store<S: SessionStore>(
    store: &S,
    snapshot: &SessionSnapshotV1,
    expected_compact_bytes: usize,
) -> anyhow::Result<StoreMeasurements> {
    for _ in 0..WARMUP_SAMPLES {
        store.save_snapshot(snapshot).await?;
        let loaded = store
            .load_snapshot(&snapshot.session.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("warmup snapshot was not found"))?;
        validate_loaded(&loaded, snapshot, expected_compact_bytes)?;
    }

    let mut save_samples = Vec::with_capacity(MEASURED_SAMPLES);
    for _ in 0..MEASURED_SAMPLES {
        let started = Instant::now();
        store.save_snapshot(snapshot).await?;
        save_samples.push(started.elapsed());
    }

    let mut load_samples = Vec::with_capacity(MEASURED_SAMPLES);
    for _ in 0..MEASURED_SAMPLES {
        let started = Instant::now();
        let loaded = store
            .load_snapshot(&snapshot.session.id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("measured snapshot was not found"))?;
        load_samples.push(started.elapsed());
        validate_loaded(&loaded, snapshot, expected_compact_bytes)?;
    }

    Ok(StoreMeasurements {
        save: latency(save_samples)?,
        load: latency(load_samples)?,
        list_count: store.list().await?.len(),
    })
}

fn validate_loaded(
    loaded: &SessionSnapshotV1,
    expected: &SessionSnapshotV1,
    expected_compact_bytes: usize,
) -> anyhow::Result<()> {
    loaded.validate_for_session(&expected.session.id)?;
    if loaded.session.id != expected.session.id
        || loaded.session.messages.len() != expected.session.messages.len()
        || serde_json::to_vec(loaded)?.len() != expected_compact_bytes
    {
        anyhow::bail!("loaded snapshot changed identity, shape, or serialized byte count");
    }
    Ok(())
}

fn file_tree_stats(root: &Path) -> anyhow::Result<(usize, u64)> {
    let mut directories = vec![root.to_path_buf()];
    let mut files = 0usize;
    let mut bytes = 0u64;
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                directories.push(entry.path());
            } else if file_type.is_file() {
                files = files.saturating_add(1);
                bytes = bytes.saturating_add(entry.metadata()?.len());
            }
        }
    }
    Ok((files, bytes))
}

fn latency(mut samples: Vec<Duration>) -> anyhow::Result<Latency> {
    if samples.is_empty() {
        anyhow::bail!("benchmark has no measured samples");
    }
    samples.sort_unstable();
    Ok(Latency {
        p50_ms: percentile_ms(&samples, 50),
        p95_ms: percentile_ms(&samples, 95),
        max_ms: duration_ms(*samples.last().expect("samples are not empty")),
    })
}

fn percentile_ms(samples: &[Duration], percentile: usize) -> f64 {
    let rank = (samples.len() * percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(samples.len() - 1);
    duration_ms(samples[rank])
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn latency_json(latency: Latency, budget_ms: f64) -> serde_json::Value {
    json!({
        "p50Ms": latency.p50_ms,
        "p95Ms": latency.p95_ms,
        "maxMs": latency.max_ms,
        "budgetP95Ms": budget_ms,
        "passed": latency.p95_ms <= budget_ms,
    })
}

fn machine_metadata() -> serde_json::Value {
    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "logicalCpus": std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1),
        "processor": processor_name(),
    })
}

fn processor_name() -> Option<String> {
    std::env::var("PROCESSOR_IDENTIFIER").ok().or_else(|| {
        let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").ok()?;
        cpuinfo.lines().find_map(|line| {
            line.strip_prefix("model name")
                .and_then(|value| value.split_once(':'))
                .map(|(_, value)| value.trim().to_owned())
        })
    })
}
