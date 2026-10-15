//! Small, reproducible qualification for the workspace-owned a3s-vec FTS index.
//!
//! Run from `crates/code` in release mode:
//!
//! `cargo run --locked --release -p a3s-code-core --example workspace_persistent_index_benchmark --features a3s-vec-fts`

use a3s_code_core::workspace::{
    ChunkCatalogLimits, ChunkingConfig, LexicalSearchRequest, WorkspaceChunkCatalog,
    WorkspaceLexicalEngine, WorkspacePath, WorkspacePersistentIndex,
};
use anyhow::{Context, Result};
use std::time::{Duration, Instant};

// Catalog admission is intentionally not part of this benchmark. The public
// single-file mutation API publishes an immutable snapshot on every call, so
// constructing thousands of files through it would measure O(n²) fixture setup
// instead of a3s-vec indexing/query performance. Production reconciliation builds
// the initial snapshot in one pass before this layer is invoked.
fn file_count() -> usize {
    std::env::var("A3S_BENCH_FILES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(64)
}
const QUERY_SAMPLES: usize = 32;

fn main() -> Result<()> {
    let workspace = tempfile::tempdir().context("create benchmark workspace")?;
    let fixture_started = Instant::now();
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .context("create a3s-vec catalog")?;
    let file_count = file_count();
    for index in 0..file_count {
        let path = WorkspacePath::from_normalized(format!("src/module-{index:04}.rs"));
        catalog
            .replace_file(
                &path,
                Some("rust"),
                1,
                &format!(
                    "pub fn module_{index}() {{\n    // persistent a3s-vec benchmark sentinel\n}}\n"
                ),
            )
            .with_context(|| format!("admit {}", path.as_str()))?;
    }
    let fixture_ms = elapsed_ms(fixture_started.elapsed());
    let index = WorkspacePersistentIndex::open(
        workspace.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::A3sVec,
    )
    .context("open persistent a3s-vec index")?;

    let build_started = Instant::now();
    index
        .sync_snapshot(&catalog.snapshot()?)
        .context("build persistent generation")?;
    let build_ms = elapsed_ms(build_started.elapsed());

    let request = LexicalSearchRequest::new("persistent a3s-vec benchmark sentinel");
    let mut latencies = Vec::with_capacity(QUERY_SAMPLES);
    for _ in 0..QUERY_SAMPLES {
        let started = Instant::now();
        let result = index
            .search(&request)
            .context("query persistent generation")?;
        if result.hits.is_empty() {
            anyhow::bail!("benchmark query returned no hits");
        }
        latencies.push(elapsed_ms(started.elapsed()));
    }
    latencies.sort_by(f64::total_cmp);

    // A source revision with identical bytes should not rebuild native
    // postings. This is the common watcher path for metadata-only updates.
    let unchanged_path = WorkspacePath::from_normalized("src/module-0000.rs");
    catalog.replace_file(
        &unchanged_path,
        Some("rust"),
        2,
        "pub fn module_0() {\n    // persistent a3s-vec benchmark sentinel\n}\n",
    )?;
    let reuse_started = Instant::now();
    index
        .sync_snapshot(&catalog.snapshot()?)
        .context("reuse persistent generation")?;
    let reuse_ms = elapsed_ms(reuse_started.elapsed());

    // A single-file content change now builds the next generation
    // incrementally (copy-forward + engine delete/upsert) when the chunk
    // delta is bounded; this is the dominant editing-path cost.
    catalog.replace_file(
        &unchanged_path,
        Some("rust"),
        3,
        "pub fn module_0() {\n    // incrementally edited persistent a3s-vec benchmark sentinel\n}\n",
    )?;
    let incremental_started = Instant::now();
    index
        .sync_snapshot(&catalog.snapshot()?)
        .context("publish incrementally edited persistent generation")?;
    let incremental_ms = elapsed_ms(incremental_started.elapsed());
    let incremental_hits = index
        .search(&LexicalSearchRequest::new("incrementally edited"))
        .context("query incrementally edited generation")?
        .hits
        .len();
    anyhow::ensure!(
        incremental_hits > 0,
        "incremental edit is not retrievable after publication"
    );

    // The full-rebuild fallback stays measurable: churn every file so the
    // delta exceeds its bound and the full build path runs.
    for file in 0..file_count {
        let path = WorkspacePath::from_normalized(format!("src/module-{file:04}.rs"));
        catalog.replace_file(
            &path,
            Some("rust"),
            4,
            &format!(
                "pub fn module_{file}() {{\n    // fully churned persistent a3s-vec benchmark sentinel\n}}\n"
            ),
        )?;
    }
    let rebuild_started = Instant::now();
    index
        .sync_snapshot(&catalog.snapshot()?)
        .context("publish fully churned persistent generation")?;
    let rebuild_ms = elapsed_ms(rebuild_started.elapsed());

    let status = index.status();
    println!(
        "{{\"files\":{file_count},\"fixtureAdmissionMs\":{fixture_ms:.3},\"indexedChunks\":{},\"buildMs\":{build_ms:.3},\"queryP50Ms\":{:.3},\"queryP95Ms\":{:.3},\"sameContentUpdateMs\":{reuse_ms:.3},\"incrementalEditMs\":{incremental_ms:.3},\"fullChurnRebuildMs\":{rebuild_ms:.3},\"generation\":\"{}\"}}",
        status.indexed_chunks,
        percentile(&latencies, 0.50),
        percentile(&latencies, 0.95),
        status.generation.as_deref().unwrap_or_default(),
    );
    Ok(())
}

fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    let index = ((sorted.len().saturating_sub(1)) as f64 * quantile).round() as usize;
    sorted[index]
}
