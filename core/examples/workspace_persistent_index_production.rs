//! Production-style qualification of the workspace-owned zvec FTS projection.
//!
//! This exercises the real manifest scanner, catalog runtime, background
//! index coordinator, concurrent readers, same-content generation reuse,
//! changed-content publication, generation cleanup, and restart reopen.
//!
//! Run from `crates/code` in release mode:
//!
//! `cargo run --locked --release -p a3s-code-core --example workspace_persistent_index_production --features zvec-rust-fts-bundled`

use a3s_code_core::workspace::{
    scan_workspace_files, LexicalSearchRequest, ManifestWorkspaceBackend, WorkspaceLexicalEngine,
    WorkspacePersistentIndex, WorkspacePersistentIndexPhase, WorkspaceServices,
};
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_FILE_COUNT: usize = 512;
const DEFAULT_QUERY_WORKERS: usize = 8;
const QUERIES_PER_WORKER: usize = 16;
const READY_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let file_count = env_usize("A3S_WORKSPACE_ACCEPTANCE_FILES", DEFAULT_FILE_COUNT)?;
    let query_workers = env_usize(
        "A3S_WORKSPACE_ACCEPTANCE_QUERY_WORKERS",
        DEFAULT_QUERY_WORKERS,
    )?;
    if file_count == 0 || query_workers == 0 {
        bail!("file and query worker counts must be greater than zero");
    }

    let workspace = tempfile::tempdir().context("create production acceptance workspace")?;
    let source_root = workspace.path().join("src");
    write_fixture(&source_root, file_count)?;
    let discovery_started = Instant::now();
    let discovered_files = scan_workspace_files(workspace.path());
    let discovery_ms = elapsed_ms(discovery_started.elapsed());
    if discovered_files.len() != file_count {
        bail!(
            "standalone manifest scan discovered {} files, expected {file_count}",
            discovered_files.len()
        );
    }

    let backend = ManifestWorkspaceBackend::new_deferred(workspace.path());
    let index_root = workspace.path().join(".a3s-code").join("index");
    let persistent = backend
        .configure_persistent_index(&index_root)
        .context("configure persistent workspace index")?;
    let services = WorkspaceServices::local_with_retrieval_backend(Arc::clone(&backend));
    let catalog = services
        .chunk_catalog()
        .context("retrieval services did not expose a chunk catalog")?;

    let scan_started = Instant::now();
    backend.manifest().activate();
    wait_for_catalog(&catalog, file_count).await?;
    let scan_ms = elapsed_ms(scan_started.elapsed());
    let initial_snapshot = catalog
        .snapshot()
        .context("read initial catalog snapshot")?;

    let build_started = Instant::now();
    wait_for_ready(
        &persistent,
        initial_snapshot.source_revision(),
        initial_snapshot.chunk_count(),
    )
    .await?;
    let initial_snapshot = wait_for_catalog_stable(&catalog).await?;
    let build_ms = elapsed_ms(build_started.elapsed());
    let initial_generation = persistent
        .status()
        .generation
        .clone()
        .context("initial persistent generation is missing")?;

    let query = LexicalSearchRequest::new("production acceptance sentinel");
    let query_latencies =
        concurrent_queries(Arc::clone(&persistent), query.clone(), query_workers).await?;
    let query_p50_ms = percentile(&query_latencies, 0.50);
    let query_p95_ms = percentile(&query_latencies, 0.95);

    let unchanged_path = source_root.join("module-0000.rs");
    let unchanged_content = fs::read(&unchanged_path).context("read unchanged fixture")?;
    let unchanged_revision = initial_snapshot.source_revision();
    fs::write(&unchanged_path, &unchanged_content).context("rewrite unchanged fixture")?;
    wait_for_source_revision(&catalog, unchanged_revision).await?;
    let unchanged_snapshot = wait_for_catalog_stable(&catalog).await?;
    wait_for_ready(
        &persistent,
        unchanged_snapshot.source_revision(),
        unchanged_snapshot.chunk_count(),
    )
    .await?;
    let reused_generation = persistent
        .status()
        .generation
        .clone()
        .context("same-content generation is missing")?;
    if reused_generation != initial_generation {
        bail!(
            "same-content update published generation {reused_generation}, expected {initial_generation}"
        );
    }

    let changed_content = String::from_utf8(unchanged_content)
        .context("fixture should contain UTF-8 source")?
        .replace(
            "production acceptance sentinel",
            "production changed sentinel",
        );
    fs::write(&unchanged_path, changed_content).context("write changed fixture")?;
    let changed_snapshot = wait_for_catalog_marker(
        &catalog,
        "src/module-0000.rs",
        "production changed sentinel",
    )
    .await?;
    wait_for_ready(
        &persistent,
        changed_snapshot.source_revision(),
        changed_snapshot.chunk_count(),
    )
    .await?;
    let changed_generation = persistent
        .status()
        .generation
        .clone()
        .context("changed persistent generation is missing")?;
    if changed_generation == initial_generation {
        bail!("changed content reused the previous persistent generation");
    }
    if persistent
        .search(&LexicalSearchRequest::new("production changed sentinel"))?
        .hits
        .is_empty()
    {
        bail!("changed-content query returned no hits");
    }

    let generation_count = generation_count(&index_root)?;
    if generation_count != 1 {
        bail!("expected one retained generation after publication, found {generation_count}");
    }

    drop(persistent);
    drop(services);
    drop(backend);
    let reopened = WorkspacePersistentIndex::open(&index_root, WorkspaceLexicalEngine::ZvecRust)
        .context("reopen persistent workspace index")?;
    let restart_hits = reopened
        .search(&LexicalSearchRequest::new("production changed sentinel"))?
        .hits
        .len();
    if restart_hits == 0 {
        bail!("restart query returned no hits");
    }

    let status = reopened.status();
    println!(
        "{{\"files\":{file_count},\"discoveryMs\":{discovery_ms:.3},\"admissionMs\":{scan_ms:.3},\"chunks\":{},\"buildMs\":{build_ms:.3},\"concurrentQueries\":{},\"queryP50Ms\":{query_p50_ms:.3},\"queryP95Ms\":{query_p95_ms:.3},\"sameContentGenerationReused\":true,\"changedGenerationPublished\":true,\"restartHits\":{restart_hits},\"generationsRetained\":{},\"generation\":\"{}\"}}",
        status.indexed_chunks,
        query_latencies.len(),
        generation_count,
        status.generation.as_deref().unwrap_or_default(),
    );
    Ok(())
}

fn write_fixture(root: &Path, file_count: usize) -> Result<()> {
    fs::create_dir_all(root).context("create fixture source directory")?;
    for index in 0..file_count {
        let path = root.join(format!("module-{index:04}.rs"));
        let content = format!(
            "// production acceptance sentinel\n// production acceptance sentinel\npub fn module_{index}() {{\n    let marker = \"production acceptance sentinel\";\n    let _ = marker;\n}}\n"
        );
        fs::write(path, content).with_context(|| format!("write fixture {index}"))?;
    }
    Ok(())
}

async fn wait_for_catalog(
    catalog: &a3s_code_core::workspace::WorkspaceChunkCatalog,
    expected_files: usize,
) -> Result<()> {
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            let snapshot = catalog.snapshot().context("read catalog snapshot")?;
            if snapshot.file_count() == expected_files && snapshot.chunk_count() >= expected_files {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("manifest/catalog admission timed out")??;
    Ok(())
}

async fn wait_for_source_revision(
    catalog: &a3s_code_core::workspace::WorkspaceChunkCatalog,
    previous: u64,
) -> Result<()> {
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            if catalog
                .snapshot()
                .context("read catalog snapshot")?
                .source_revision()
                > previous
            {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("workspace change admission timed out")??;
    Ok(())
}

async fn wait_for_catalog_stable(
    catalog: &a3s_code_core::workspace::WorkspaceChunkCatalog,
) -> Result<a3s_code_core::workspace::ChunkCatalogSnapshot> {
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            let first = catalog.snapshot().context("read catalog snapshot")?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            let second = catalog.snapshot().context("read catalog snapshot")?;
            if first.source_revision() == second.source_revision()
                && first.chunk_count() == second.chunk_count()
            {
                return Ok::<_, anyhow::Error>(second);
            }
        }
    })
    .await
    .context("catalog did not settle")?
}

async fn wait_for_catalog_marker(
    catalog: &a3s_code_core::workspace::WorkspaceChunkCatalog,
    path: &str,
    marker: &str,
) -> Result<a3s_code_core::workspace::ChunkCatalogSnapshot> {
    tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            let snapshot = catalog.snapshot().context("read catalog snapshot")?;
            if snapshot
                .chunks()
                .iter()
                .any(|chunk| chunk.path.as_ref() == path && chunk.text.contains(marker))
            {
                return Ok::<_, anyhow::Error>(snapshot);
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .context("changed catalog content was not admitted")?
}

async fn wait_for_ready(
    index: &WorkspacePersistentIndex,
    source_revision: u64,
    chunk_count: usize,
) -> Result<()> {
    let result = tokio::time::timeout(READY_TIMEOUT, async {
        loop {
            let status = index.status();
            if status.phase == WorkspacePersistentIndexPhase::Ready
                && status.source_revision >= source_revision
                && status.indexed_chunks >= chunk_count
            {
                return Ok::<(), anyhow::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    if result.is_err() {
        let status = index.status();
        bail!(
            "persistent index readiness timed out (phase={:?}, sourceRevision={}, minimumSourceRevision={}, indexedChunks={}, minimumChunks={})",
            status.phase,
            status.source_revision,
            source_revision,
            status.indexed_chunks,
            chunk_count,
        );
    }
    result.context("persistent index readiness timed out")??;
    Ok(())
}

async fn concurrent_queries(
    index: Arc<WorkspacePersistentIndex>,
    request: LexicalSearchRequest,
    workers: usize,
) -> Result<Vec<f64>> {
    let mut tasks = Vec::with_capacity(workers);
    for _ in 0..workers {
        let index = Arc::clone(&index);
        let request = request.clone();
        tasks.push(tokio::task::spawn_blocking(move || -> Result<Vec<f64>> {
            let mut samples = Vec::with_capacity(QUERIES_PER_WORKER);
            for _ in 0..QUERIES_PER_WORKER {
                let started = Instant::now();
                if index.search(&request)?.hits.is_empty() {
                    bail!("concurrent query returned no hits");
                }
                samples.push(elapsed_ms(started.elapsed()));
            }
            Ok(samples)
        }));
    }
    let mut samples = Vec::with_capacity(workers * QUERIES_PER_WORKER);
    for task in tasks {
        samples.extend(task.await.context("join concurrent query worker")??);
    }
    samples.sort_by(f64::total_cmp);
    Ok(samples)
}

fn generation_count(root: &Path) -> Result<usize> {
    Ok(fs::read_dir(root)
        .with_context(|| format!("read generation root {}", root.display()))?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("generation-"))
        })
        .count())
}

fn env_usize(name: &str, default: usize) -> Result<usize> {
    std::env::var(name)
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("parse {name}"))
        })
        .unwrap_or(Ok(default))
}

fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    let index = ((sorted.len().saturating_sub(1)) as f64 * quantile).round() as usize;
    sorted[index]
}
