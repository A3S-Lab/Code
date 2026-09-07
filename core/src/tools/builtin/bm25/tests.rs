use super::*;
use crate::workspace::WorkspaceLexicalEngine;
use serde::Deserialize;
use std::collections::BTreeMap;

fn context_with_files(files: &[(&str, &str)]) -> (tempfile::TempDir, ToolContext) {
    let temp = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let path = temp.path().join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
    let context = ToolContext::new(temp.path().to_path_buf());
    (temp, context)
}

#[test]
fn schema_exposes_bounded_plain_text_search() {
    let tool = Bm25Tool;
    let schema = tool.parameters();

    assert_eq!(tool.name(), "bm25");
    assert_eq!(schema["required"], serde_json::json!(["query"]));
    assert_eq!(schema["properties"]["limit"]["maximum"], MAX_LIMIT);
    assert_eq!(
        schema["properties"]["context"]["maximum"],
        MAX_CONTEXT_LINES
    );
    assert_eq!(tool.capabilities(&serde_json::json!({})).max_parallelism, 2);
}

#[tokio::test]
async fn ranks_multi_term_chunks_and_returns_source_metadata() {
    let (_temp, context) = context_with_files(&[
        (
            "src/session.rs",
            "pub fn invalidate_session_cache() {\n    // session cache invalidation policy\n    clear_session_cache();\n}\n",
        ),
        (
            "src/log.rs",
            "pub fn log_session() {\n    println!(\"session\");\n}\n",
        ),
    ]);

    let result = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "session cache invalidation",
                "path": "src",
                "glob": "*.rs",
                "limit": 5,
                "context": 1
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("src/session.rs:1-3"));
    assert!(result.content.contains("session cache invalidation policy"));
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["algorithm"], "bm25");
    assert_eq!(
        metadata["parameters"]["engine"],
        WorkspaceLexicalEngine::default().stable_id()
    );
    assert_eq!(metadata["parameters"]["tokenizer"], "whitespace");
    assert_eq!(metadata["results"][0]["path"], "src/session.rs");
    assert!(metadata["results"][0]["score"].as_f64().unwrap() > 0.0);
    assert_eq!(metadata["source_anchors"][0], "src/session.rs");
}

#[tokio::test]
async fn honors_glob_filter() {
    let (_temp, context) = context_with_files(&[
        ("src/auth.rs", "authentication policy token\n"),
        ("README.md", "authentication policy token token token\n"),
    ]);

    let result = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "authentication policy token",
                "glob": "*.rs"
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    assert!(result.content.contains("src/auth.rs"));
    assert!(!result.content.contains("README.md"));
}

#[tokio::test]
async fn reports_no_matches_without_failing() {
    let (_temp, context) = context_with_files(&[("src/lib.rs", "pub fn existing() {}\n")]);

    let result = Bm25Tool
        .execute(&serde_json::json!({"query": "missing term"}), &context)
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.content.contains("No BM25 matches found"));
}

#[tokio::test]
async fn rejects_empty_punctuation_and_escaping_queries() {
    let (_temp, context) = context_with_files(&[("src/lib.rs", "content\n")]);

    for args in [
        serde_json::json!({"query": ""}),
        serde_json::json!({"query": "::"}),
        serde_json::json!({"query": "content", "path": "../outside"}),
    ] {
        let result = Bm25Tool.execute(&args, &context).await.unwrap();
        assert!(!result.success, "args={args} output={}", result.content);
    }
}

#[tokio::test]
async fn validates_numeric_bounds_for_direct_calls() {
    let (_temp, context) = context_with_files(&[("src/lib.rs", "content\n")]);

    for args in [
        serde_json::json!({"query": "content", "limit": 0}),
        serde_json::json!({"query": "content", "limit": MAX_LIMIT + 1}),
        serde_json::json!({"query": "content", "context": MAX_CONTEXT_LINES + 1}),
    ] {
        let result = Bm25Tool.execute(&args, &context).await.unwrap();
        assert!(!result.success, "args={args} output={}", result.content);
    }
}

#[tokio::test]
async fn manifest_backed_bm25_uses_the_incremental_catalog_without_query_reads() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("src/cache.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "pub fn invalidate_session_cache() { /* session cache invalidation policy */ }\n",
    )
    .unwrap();
    let services = crate::workspace::WorkspaceServices::local_with_retrieval(temp.path());
    let catalog = services.chunk_catalog().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if catalog.snapshot().unwrap().source_revision() > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("incremental catalog did not become ready");
    let context = ToolContext::new(temp.path().to_path_buf()).with_workspace_services(services);

    let result = Bm25Tool
        .execute(
            &serde_json::json!({"query": "session cache invalidation"}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["mode"], "incremental_catalog");
    assert_eq!(
        metadata["parameters"]["engine"],
        WorkspaceLexicalEngine::default().stable_id()
    );
    assert_eq!(metadata["scan"]["read_files"], 0);
    assert_eq!(metadata["results"][0]["path"], "src/cache.rs");
}

#[cfg(feature = "zvec-rust-fts")]
#[tokio::test]
async fn local_retrieval_automatically_uses_the_workspace_persistent_zvec_index() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("src/cache.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "pub fn invalidate_session_cache() { /* persistent session cache policy */ }\n",
    )
    .unwrap();
    let services = crate::workspace::WorkspaceServices::local_with_retrieval(temp.path());
    let catalog = services.chunk_catalog().unwrap();
    let index = services.persistent_index().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if catalog.snapshot().unwrap().source_revision() > 0 && index.is_ready() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("persistent index did not become ready");

    let context = ToolContext::new(temp.path().to_path_buf()).with_workspace_services(services);
    let result = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "persistent session cache",
                "_persistent_index": true
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{}", result.content);
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["index_kind"], "persistent_zvec_fts");
    assert_eq!(metadata["execution_mode"], "persistent_zvec_fts");
    assert_eq!(metadata["results"][0]["path"], "src/cache.rs");
    assert!(temp.path().join(".a3s-code/index/CURRENT").is_file());

    drop(context);
    let reopened_services = crate::workspace::WorkspaceServices::local_with_retrieval(temp.path());
    let reopened_index = reopened_services.persistent_index().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !reopened_index.is_ready() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("reopened persistent index did not become ready");
    let reopened_context =
        ToolContext::new(temp.path().to_path_buf()).with_workspace_services(reopened_services);
    let reopened_result = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "persistent session cache",
                "_persistent_index": true
            }),
            &reopened_context,
        )
        .await
        .unwrap();
    assert!(reopened_result.success, "{}", reopened_result.content);
}

#[cfg(feature = "zvec-rust-fts")]
#[tokio::test]
async fn persistent_bm25_never_returns_replaced_source_and_reindexes_new_content() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("src/state.rs");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "pub fn old_state_marker() {}\n").unwrap();

    let services = crate::workspace::WorkspaceServices::local_with_retrieval(temp.path());
    let catalog = services.chunk_catalog().unwrap();
    let index = services.persistent_index().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let status = index.status();
            if catalog.snapshot().unwrap().source_revision() > 0
                && status.phase == crate::workspace::WorkspacePersistentIndexPhase::Ready
            {
                break status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("initial persistent index did not become ready");
    let initial_status = index.status();
    let context = ToolContext::new(temp.path().to_path_buf()).with_workspace_services(services);

    let initial = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "old state marker",
                "_persistent_index": true
            }),
            &context,
        )
        .await
        .unwrap();
    assert!(initial.success, "{}", initial.content);
    assert_eq!(
        initial.metadata.as_ref().unwrap()["index_kind"],
        "persistent_zvec_fts"
    );
    assert_eq!(
        initial.metadata.as_ref().unwrap()["results"][0]["path"],
        "src/state.rs"
    );

    // Write through the real workspace path. The query below may race with
    // reconciliation, so either a fresh generation or source verification
    // must prevent the old chunk from being returned.
    std::fs::write(&path, "pub fn new_state_marker() {}\n").unwrap();
    let replaced = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "old state marker",
                "_persistent_index": true
            }),
            &context,
        )
        .await
        .unwrap();
    let replaced_metadata = replaced.metadata.as_ref().unwrap();
    assert!(
        !replaced.content.contains("old_state_marker"),
        "stale source leaked through persistent BM25: {replaced_metadata:?}; output={}",
        replaced.content
    );

    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let status = index.status();
            if status.source_revision > initial_status.source_revision
                && status.generation != initial_status.generation
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("persistent index did not publish the replacement generation");

    let current = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "new state marker",
                "_persistent_index": true
            }),
            &context,
        )
        .await
        .unwrap();
    assert!(current.success, "{}", current.content);
    let current_metadata = current.metadata.as_ref().unwrap();
    assert_eq!(current_metadata["index_kind"], "persistent_zvec_fts");
    assert!(matches!(
        current_metadata["freshness"].as_str(),
        Some("ready" | "rebuilding")
    ));
    assert_eq!(current_metadata["results"][0]["path"], "src/state.rs");
}

#[cfg(feature = "zvec-rust-fts")]
#[tokio::test]
async fn persistent_bm25_falls_back_to_the_catalog_before_native_ready() {
    let temp = tempfile::tempdir().unwrap();
    let path = WorkspacePath::from_normalized("src/fallback.rs");
    let content = "pub fn fallback_ready_marker() {}\n";
    std::fs::create_dir_all(temp.path().join("src")).unwrap();
    std::fs::write(temp.path().join(path.as_str()), content).unwrap();

    let catalog = crate::workspace::WorkspaceChunkCatalog::new(
        crate::workspace::ChunkingConfig::default(),
        crate::workspace::ChunkCatalogLimits::default(),
    )
    .unwrap();
    catalog
        .replace_file(&path, Some("rust"), 1, content)
        .unwrap();
    let index = crate::workspace::WorkspacePersistentIndex::open(
        temp.path().join(".a3s-code/index"),
        WorkspaceLexicalEngine::ZvecRust,
    )
    .unwrap();
    assert!(
        !index.is_ready(),
        "test must start before native publication"
    );

    let local = std::sync::Arc::new(crate::workspace::LocalWorkspaceBackend::new(
        temp.path().to_path_buf(),
    ));
    let file_system: std::sync::Arc<dyn crate::workspace::WorkspaceFileSystem> = local.clone();
    let search: std::sync::Arc<dyn crate::workspace::WorkspaceSearch> = local;
    let services = crate::workspace::WorkspaceServices::builder(
        crate::workspace::WorkspaceRef::new("fallback", "fallback://workspace"),
        file_system,
    )
    .search(search)
    .chunk_catalog(catalog)
    .persistent_index(index)
    .build();
    let context = ToolContext::new(temp.path().to_path_buf()).with_workspace_services(services);

    let result = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "fallback ready marker",
                "_persistent_index": true
            }),
            &context,
        )
        .await
        .unwrap();
    assert!(result.success, "{}", result.content);
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["mode"], "incremental_catalog");
    assert_eq!(metadata["scan"]["read_files"], 0);
    assert_eq!(metadata["source_verified"], true);
    assert_eq!(metadata["results"][0]["path"], "src/fallback.rs");

    // Native unavailability must not weaken source verification. A catalog
    // snapshot can lag behind an external edit before the first generation
    // is published, so even this fallback must suppress its stale chunk.
    std::fs::write(
        temp.path().join(path.as_str()),
        "pub fn unpublished_edit() {}\n",
    )
    .unwrap();
    let stale_fallback = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "fallback ready marker",
                "_persistent_index": true
            }),
            &context,
        )
        .await
        .unwrap();
    let stale_metadata = stale_fallback.metadata.unwrap();
    assert_eq!(stale_metadata["source_verified"], true);
    assert_eq!(stale_metadata["filtered_stale"], true);
    assert_eq!(stale_metadata["returned_results"], 0);
    std::fs::write(temp.path().join(path.as_str()), content).unwrap();

    // Once a ready generation becomes older than the catalog, the same public
    // request must remain useful instead of returning an empty stale-index
    // page while the replacement generation is being built.
    let catalog = context.workspace_services.chunk_catalog().unwrap();
    let index = context.workspace_services.persistent_index().unwrap();
    index.sync_snapshot(&catalog.snapshot().unwrap()).unwrap();

    // An unchanged file can still be served by the old immutable generation
    // while a newer snapshot is being built. Expose that state explicitly so
    // hosts can observe rebuild progress without making the model change its
    // public search mode.
    let stable_path = WorkspacePath::from_normalized("src/stable.rs");
    let stable_content = "pub fn stable_ready_marker() {}\n";
    std::fs::write(temp.path().join(stable_path.as_str()), stable_content).unwrap();
    catalog
        .replace_file(&stable_path, Some("rust"), 2, stable_content)
        .unwrap();
    let stable = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "fallback ready marker",
                "_persistent_index": true
            }),
            &context,
        )
        .await
        .unwrap();
    assert!(stable.success, "{}", stable.content);
    let stable_metadata = stable.metadata.unwrap();
    assert_eq!(stable_metadata["execution_mode"], "persistent_zvec_fts");
    assert_eq!(stable_metadata["freshness"], "rebuilding");
    assert_eq!(stable_metadata["catalog_source_revision"], 2);

    let replacement = "pub fn replacement_ready_marker() {}\n";
    std::fs::write(temp.path().join(path.as_str()), replacement).unwrap();
    catalog
        .replace_file(&path, Some("rust"), 3, replacement)
        .unwrap();
    let during_rebuild = Bm25Tool
        .execute(
            &serde_json::json!({
                "query": "replacement ready marker",
                "_persistent_index": true
            }),
            &context,
        )
        .await
        .unwrap();
    assert!(during_rebuild.success, "{}", during_rebuild.content);
    let during_metadata = during_rebuild.metadata.unwrap();
    assert_eq!(during_metadata["mode"], "incremental_catalog");
    assert_eq!(during_metadata["results"][0]["path"], "src/fallback.rs");
}

#[derive(Debug, Deserialize)]
struct RetrievalFixture {
    schema_version: u32,
    documents: Vec<RetrievalDocument>,
    queries: Vec<RetrievalQuery>,
    expected_bm25_summary: RetrievalSummary,
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
    expected_bm25_paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RetrievalSummary {
    query_count: usize,
    recall_at_10: f64,
    mean_reciprocal_rank: f64,
    category_recall_at_10: BTreeMap<String, f64>,
}

#[tokio::test]
async fn workspace_retrieval_v1_locks_native_bm25_baseline() {
    let fixture: RetrievalFixture = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/workspace-retrieval-v1/corpus.json"
    )))
    .expect("workspace retrieval fixture must parse");
    assert_eq!(fixture.schema_version, 1);
    assert_eq!(
        fixture.queries.len(),
        fixture.expected_bm25_summary.query_count
    );

    let files = fixture
        .documents
        .iter()
        .map(|document| (document.path.as_str(), document.content.as_str()))
        .collect::<Vec<_>>();
    let (_temp, context) = context_with_files(&files);
    let mut reciprocal_rank_sum = 0.0;
    let mut recalled = 0usize;
    let mut category_counts = BTreeMap::<String, (usize, usize)>::new();

    for query in &fixture.queries {
        let result = Bm25Tool
            .execute(
                &serde_json::json!({"query": query.query, "limit": 10}),
                &context,
            )
            .await
            .unwrap_or_else(|error| panic!("query '{}' failed: {error}", query.id));
        assert!(result.success, "query '{}': {}", query.id, result.content);

        let paths = result
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("results"))
            .and_then(serde_json::Value::as_array)
            .map(|results| {
                results
                    .iter()
                    .filter_map(|result| result.get("path").and_then(serde_json::Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        assert_eq!(
            paths, query.expected_bm25_paths,
            "BM25 baseline drifted for query '{}'",
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
    }

    let query_count = fixture.queries.len() as f64;
    assert_metric(
        recalled as f64 / query_count,
        fixture.expected_bm25_summary.recall_at_10,
        "Recall@10",
    );
    assert_metric(
        reciprocal_rank_sum / query_count,
        fixture.expected_bm25_summary.mean_reciprocal_rank,
        "MRR",
    );
    for (category, expected) in &fixture.expected_bm25_summary.category_recall_at_10 {
        let (category_recalled, category_total) =
            category_counts.get(category).copied().unwrap_or_default();
        assert_metric(
            category_recalled as f64 / category_total as f64,
            *expected,
            &format!("{category} Recall@10"),
        );
    }
}

fn assert_metric(actual: f64, expected: f64, name: &str) {
    assert!(
        (actual - expected).abs() < 1e-12,
        "{name} drifted: actual={actual}, expected={expected}"
    );
}

#[test]
fn workspace_retrieval_v1_lifecycle_contract_is_well_formed() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/workspace-retrieval-v1/lifecycle.json"
    )))
    .expect("workspace retrieval lifecycle fixture must parse");

    assert_eq!(fixture["schema_version"], 1);
    let steps = fixture["steps"]
        .as_array()
        .expect("lifecycle steps must be an array");
    let operations = steps
        .iter()
        .map(|step| {
            step["operation"]
                .as_str()
                .expect("every lifecycle step must have an operation")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        operations,
        [
            "reconcile",
            "upsert",
            "upsert",
            "rename",
            "delete",
            "reconcile"
        ]
    );
    for step in steps {
        assert!(step["id"].is_string());
        assert!(step["documents"].is_array());
        assert!(step["expected_read_paths"].is_array());
        assert!(step["expected_catalog_paths"].is_array());
    }
}
