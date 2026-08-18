//! Release qualification for Code Intelligence on a large saved workspace.
//!
//! The workflow places the checked-in fake LSP fixture on `PATH` as
//! `rust-analyzer`, so this profile exercises the real process protocol without
//! depending on a developer machine or a public service.

use a3s_code_core::{
    LocalCodeIntelligence, LocalWorkspaceManifest, LocalWorkspaceManifestSnapshot,
    ManifestWorkspaceBackend, WorkspaceCodeIntelligence, WorkspaceFileSystem, WorkspacePath,
};
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const WORKSPACE_FILES: usize = 5_000;
const WARMUP_SAMPLES: usize = 3;
const MEASURED_SAMPLES: usize = 20;
const MANIFEST_DEADLINE: Duration = Duration::from_secs(15);
const COLD_QUERY_BUDGET_MS: f64 = 5_000.0;
const WARM_QUERY_P95_BUDGET_MS: f64 = 250.0;
const SHUTDOWN_BUDGET_MS: f64 = 5_000.0;
const ACTIVE_RSS_DELTA_BUDGET_BYTES: u64 = 512 * 1024 * 1024;
const RETAINED_RSS_DELTA_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Serialize)]
struct Latency {
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        anyhow::bail!("Code Intelligence qualification must run with --release");
    }

    let fake_lsp = fake_lsp_path()?;
    let protocol_log = fake_lsp.with_extension("log");
    if protocol_log.exists() {
        std::fs::remove_file(&protocol_log)?;
    }

    let rss_before = resident_set_bytes();
    let workspace = tempfile::tempdir()?;
    let source_bytes = create_workspace(workspace.path())?;

    let backend = ManifestWorkspaceBackend::new(workspace.path());
    let manifest_started = Instant::now();
    let snapshot = wait_for_manifest(backend.manifest(), WORKSPACE_FILES + 1).await?;
    let manifest_elapsed = manifest_started.elapsed();
    let indexed_files = snapshot.files.len();

    let file_system: Arc<dyn WorkspaceFileSystem> = backend.clone();
    let provider_started = Instant::now();
    let provider = LocalCodeIntelligence::start(
        "code-intelligence-performance-v1",
        backend.manifest(),
        file_system,
    )
    .await?;
    let provider_start_elapsed = provider_started.elapsed();
    let target = WorkspacePath::from_normalized("src/module_00/file_0000.rs");

    let cold_started = Instant::now();
    let cold = provider
        .document_symbols(&target, CancellationToken::new())
        .await?;
    let cold_elapsed = cold_started.elapsed();
    ensure_nonempty("cold document-symbol", cold.items.len())?;

    for _ in 0..WARMUP_SAMPLES {
        let result = provider
            .document_symbols(&target, CancellationToken::new())
            .await?;
        ensure_nonempty("document-symbol warmup", result.items.len())?;
        let result = provider
            .search_symbols("answer", 20, CancellationToken::new())
            .await?;
        ensure_nonempty("workspace-symbol warmup", result.items.len())?;
    }

    let mut document_samples = Vec::with_capacity(MEASURED_SAMPLES);
    let mut workspace_samples = Vec::with_capacity(MEASURED_SAMPLES);
    for _ in 0..MEASURED_SAMPLES {
        let started = Instant::now();
        let result = provider
            .document_symbols(&target, CancellationToken::new())
            .await?;
        document_samples.push(started.elapsed());
        ensure_nonempty("document-symbol sample", result.items.len())?;

        let started = Instant::now();
        let result = provider
            .search_symbols("answer", 20, CancellationToken::new())
            .await?;
        workspace_samples.push(started.elapsed());
        ensure_nonempty("workspace-symbol sample", result.items.len())?;
    }
    let document_latency = latency(document_samples)?;
    let workspace_latency = latency(workspace_samples)?;
    let rss_active = resident_set_bytes();

    let shutdown_started = Instant::now();
    provider.shutdown().await;
    let shutdown_elapsed = shutdown_started.elapsed();
    drop(provider);
    drop(backend);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let rss_after_shutdown = resident_set_bytes();

    let protocol = std::fs::read_to_string(&protocol_log)?;
    let process_starts = protocol.matches("\"event\":\"process_started\"").count();
    let process_exits = protocol.matches("\"event\":\"process_exiting\"").count();
    let process_cleanup_passed = process_starts == 1
        && process_exits == process_starts
        && protocol.contains("\"method\":\"shutdown\"")
        && protocol.contains("\"method\":\"exit\"");

    let manifest_passed =
        indexed_files >= WORKSPACE_FILES + 1 && manifest_elapsed <= MANIFEST_DEADLINE;
    let cold_passed = duration_ms(cold_elapsed) <= COLD_QUERY_BUDGET_MS;
    let document_passed = document_latency.p95_ms <= WARM_QUERY_P95_BUDGET_MS;
    let workspace_passed = workspace_latency.p95_ms <= WARM_QUERY_P95_BUDGET_MS;
    let shutdown_passed = duration_ms(shutdown_elapsed) <= SHUTDOWN_BUDGET_MS;
    let active_rss_delta = rss_delta(rss_before, rss_active);
    let retained_rss_delta = rss_delta(rss_before, rss_after_shutdown);
    let rss_passed = rss_within_budget(active_rss_delta, ACTIVE_RSS_DELTA_BUDGET_BYTES)
        && rss_within_budget(retained_rss_delta, RETAINED_RSS_DELTA_BUDGET_BYTES);
    let passed = manifest_passed
        && cold_passed
        && document_passed
        && workspace_passed
        && shutdown_passed
        && process_cleanup_passed
        && rss_passed;

    let report = json!({
        "schemaVersion": 1,
        "profile": "code-intelligence-large-workspace-v1",
        "build": "release",
        "machine": machine_metadata(),
        "parameters": {
            "workspaceFiles": WORKSPACE_FILES,
            "sourceBytes": source_bytes,
            "warmupSamples": WARMUP_SAMPLES,
            "measuredSamples": MEASURED_SAMPLES,
            "languageServer": "checked-in-controlled-fixture",
        },
        "manifest": {
            "observedMs": duration_ms(manifest_elapsed),
            "deadlineMs": MANIFEST_DEADLINE.as_millis(),
            "indexedFiles": indexed_files,
            "passed": manifest_passed,
        },
        "providerStart": {
            "observedMs": duration_ms(provider_start_elapsed),
            "setupIncluded": false,
        },
        "coldDocumentSymbols": {
            "observedMs": duration_ms(cold_elapsed),
            "budgetMs": COLD_QUERY_BUDGET_MS,
            "includesProcessStart": true,
            "includesSourceRead": true,
            "passed": cold_passed,
        },
        "warmDocumentSymbols": latency_json(document_latency, document_passed),
        "warmWorkspaceSymbols": latency_json(workspace_latency, workspace_passed),
        "shutdown": {
            "observedMs": duration_ms(shutdown_elapsed),
            "budgetMs": SHUTDOWN_BUDGET_MS,
            "processStarts": process_starts,
            "processExits": process_exits,
            "protocolCleanupPassed": process_cleanup_passed,
            "passed": shutdown_passed && process_cleanup_passed,
        },
        "resources": {
            "rssBeforeBytes": rss_before,
            "rssActiveBytes": rss_active,
            "rssAfterShutdownBytes": rss_after_shutdown,
            "activeRssDeltaBytes": active_rss_delta,
            "activeRssDeltaBudgetBytes": ACTIVE_RSS_DELTA_BUDGET_BYTES,
            "retainedRssDeltaBytes": retained_rss_delta,
            "retainedRssDeltaBudgetBytes": RETAINED_RSS_DELTA_BUDGET_BYTES,
            "linuxRssRequired": cfg!(target_os = "linux"),
            "passed": rss_passed,
        },
        "providerNetworkIncluded": false,
        "workspaceSetupIncluded": false,
        "passed": passed,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);

    if !passed {
        anyhow::bail!(
            "Code Intelligence qualification failed: cold {:.3} ms, document p95 {:.3} ms, workspace p95 {:.3} ms, shutdown {:.3} ms",
            duration_ms(cold_elapsed),
            document_latency.p95_ms,
            workspace_latency.p95_ms,
            duration_ms(shutdown_elapsed),
        );
    }
    Ok(())
}

fn fake_lsp_path() -> anyhow::Result<PathBuf> {
    let path = std::env::var_os("A3S_CODE_BENCH_FAKE_LSP")
        .map(PathBuf::from)
        .ok_or_else(|| {
            anyhow::anyhow!("A3S_CODE_BENCH_FAKE_LSP must identify the checked-in fake LSP binary")
        })?;
    if !path.is_file() {
        anyhow::bail!("fake LSP binary does not exist at {}", path.display());
    }
    Ok(path)
}

fn create_workspace(root: &Path) -> anyhow::Result<usize> {
    let manifest = "[package]\nname = \"a3s-code-intelligence-benchmark\"\nversion = \"0.0.0\"\nedition = \"2021\"\n";
    std::fs::write(root.join("Cargo.toml"), manifest)?;
    let mut source_bytes = manifest.len();
    for index in 0..WORKSPACE_FILES {
        let module = root.join("src").join(format!("module_{:02}", index / 100));
        std::fs::create_dir_all(&module)?;
        let source = format!(
            "/// Deterministic large-workspace fixture.\npub fn fixture_symbol_{index:04}() -> usize {{ {index} }}\n"
        );
        std::fs::write(module.join(format!("file_{:04}.rs", index % 100)), &source)?;
        source_bytes = source_bytes.saturating_add(source.len());
    }
    Ok(source_bytes)
}

async fn wait_for_manifest(
    manifest: Arc<LocalWorkspaceManifest>,
    minimum_files: usize,
) -> anyhow::Result<LocalWorkspaceManifestSnapshot> {
    let mut updates = manifest.subscribe();
    let current = manifest.snapshot();
    if current.version > 0 && current.files.len() >= minimum_files {
        return Ok(current);
    }

    tokio::time::timeout(MANIFEST_DEADLINE, async move {
        loop {
            match updates.recv().await {
                Ok(snapshot) if snapshot.version > 0 && snapshot.files.len() >= minimum_files => {
                    return Ok(snapshot)
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    anyhow::bail!("workspace manifest closed before its initial scan completed")
                }
            }
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("workspace manifest exceeded its 15 second deadline"))?
}

fn ensure_nonempty(operation: &str, count: usize) -> anyhow::Result<()> {
    if count == 0 {
        anyhow::bail!("{operation} returned no semantic results");
    }
    Ok(())
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

fn latency_json(latency: Latency, passed: bool) -> serde_json::Value {
    json!({
        "p50Ms": latency.p50_ms,
        "p95Ms": latency.p95_ms,
        "maxMs": latency.max_ms,
        "budgetP95Ms": WARM_QUERY_P95_BUDGET_MS,
        "sourceReadsIncluded": true,
        "passed": passed,
    })
}

fn rss_delta(before: Option<u64>, after: Option<u64>) -> Option<u64> {
    Some(after?.saturating_sub(before?))
}

fn rss_within_budget(value: Option<u64>, budget: u64) -> bool {
    match value {
        Some(value) => value <= budget,
        None => !cfg!(target_os = "linux"),
    }
}

fn resident_set_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    kib.checked_mul(1024)
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
