use super::catalog::WorkspaceChunkCatalog;
use super::chunk::{chunk_file, ChunkFileRequest};
use super::eligibility::WorkspaceEligibilityPolicy;
use super::types::{ChunkCatalogLimits, ChunkingConfig, WorkspaceIndexError};
use super::LexicalSearchRequest;
use crate::workspace::{LocalWorkspaceFile, LocalWorkspaceFileStatus, WorkspacePath};
use serde::Deserialize;
use std::sync::Arc;

mod lifecycle;
mod semantic;

#[test]
fn chunking_is_utf8_safe_deterministic_and_bounded_by_lines_and_bytes() {
    let config = ChunkingConfig {
        max_lines: 2,
        max_bytes: 8,
        max_chunks_per_file: 16,
    };
    let content = "alpha\n工作区\nomega\n";
    let request = || ChunkFileRequest {
        path: "src/lib.rs",
        language: Some("rust"),
        source_revision: 7,
        content,
    };
    let first = chunk_file(request(), config).unwrap();
    let second = chunk_file(request(), config).unwrap();

    assert_eq!(first.content_digest, second.content_digest);
    assert_eq!(first.chunks.len(), second.chunks.len());
    assert!(first.chunks.iter().all(|chunk| chunk.text.len() <= 8));
    assert!(first
        .chunks
        .iter()
        .all(|chunk| content.is_char_boundary(chunk.start_byte)
            && content.is_char_boundary(chunk.end_byte)));
    let rebuilt = first
        .chunks
        .iter()
        .map(|chunk| chunk.text.as_ref())
        .collect::<String>();
    assert_eq!(rebuilt, content);
    assert_eq!(
        first
            .chunks
            .iter()
            .map(|chunk| chunk.id.clone())
            .collect::<Vec<_>>(),
        second
            .chunks
            .iter()
            .map(|chunk| chunk.id.clone())
            .collect::<Vec<_>>()
    );
}

#[test]
fn chunking_rejects_over_limit_input_before_retaining_extra_ranges() {
    let error = chunk_file(
        ChunkFileRequest {
            path: "huge.txt",
            language: None,
            source_revision: 1,
            content: "abcdefghijklmnopqrstuvwxyz",
        },
        ChunkingConfig {
            max_lines: 1,
            max_bytes: 4,
            max_chunks_per_file: 2,
        },
    )
    .unwrap_err();

    assert!(matches!(
        error,
        WorkspaceIndexError::TooManyChunks { limit: 2, .. }
    ));
}

#[test]
fn catalog_budget_failure_preserves_the_published_snapshot() {
    let catalog = WorkspaceChunkCatalog::new(
        ChunkingConfig::default(),
        ChunkCatalogLimits {
            max_files: 1,
            max_chunks: 8,
            max_text_bytes: 12,
            max_index_bytes: 1024 * 1024,
        },
    )
    .unwrap();
    let first_path = WorkspacePath::from_normalized("a.rs");
    let second_path = WorkspacePath::from_normalized("b.rs");
    let first = catalog
        .replace_file(&first_path, Some("rust"), 1, "fn a() {}\n")
        .unwrap();
    let error = catalog
        .replace_file(&second_path, Some("rust"), 2, "fn b() {}\n")
        .unwrap_err();

    assert!(matches!(error, WorkspaceIndexError::BudgetExceeded { .. }));
    let after = catalog.snapshot().unwrap();
    assert_eq!(after.revision(), first.revision());
    assert_eq!(after.paths(), ["a.rs"]);
}

#[test]
fn lexical_index_budget_failure_preserves_the_published_snapshot() {
    let catalog = WorkspaceChunkCatalog::new(
        ChunkingConfig::default(),
        ChunkCatalogLimits {
            max_files: 8,
            max_chunks: 8,
            max_text_bytes: 1024,
            max_index_bytes: 1,
        },
    )
    .unwrap();
    let before = catalog.snapshot().unwrap();
    let error = catalog
        .replace_file(
            &WorkspacePath::from_normalized("src/lib.rs"),
            Some("rust"),
            1,
            "pub fn bounded_index() {}\n",
        )
        .unwrap_err();

    assert!(matches!(
        error,
        WorkspaceIndexError::BudgetExceeded {
            resource: "index byte estimate",
            ..
        }
    ));
    let after = catalog.snapshot().unwrap();
    assert_eq!(after.revision(), before.revision());
    assert_eq!(after.source_revision(), before.source_revision());
    assert!(after.paths().is_empty());
}

#[test]
fn catalog_rejects_root_and_parent_traversal_paths() {
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .unwrap();
    for path in [
        WorkspacePath::root(),
        WorkspacePath::from_normalized("../outside.rs"),
    ] {
        assert!(matches!(
            catalog.replace_file(&path, Some("rust"), 1, "content"),
            Err(WorkspaceIndexError::InvalidConfig(_))
        ));
    }
}

#[test]
fn catalog_queries_hold_immutable_snapshots_during_replacement() {
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .unwrap();
    let path = WorkspacePath::from_normalized("src/cache.rs");
    let old = catalog
        .replace_file(&path, Some("rust"), 1, "session cache invalidation\n")
        .unwrap();
    let old_chunk = Arc::clone(&old.chunks()[0]);
    let new = catalog
        .replace_file(&path, Some("rust"), 2, "credential expiry guard\n")
        .unwrap();

    assert_eq!(
        old.chunks()[0].text.as_ref(),
        "session cache invalidation\n"
    );
    assert!(Arc::ptr_eq(&old_chunk, &old.chunks()[0]));
    assert_eq!(new.chunks()[0].text.as_ref(), "credential expiry guard\n");
    assert_ne!(old.content_digest(&path), new.content_digest(&path));
}

#[test]
fn incremental_lexical_search_preserves_bm25_identifier_and_cjk_behavior() {
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .unwrap();
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("src/path_policy.rs"),
            Some("rust"),
            1,
            "pub struct LocalWorkspaceAccessPolicy;\n",
        )
        .unwrap();
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("src/cache.rs"),
            Some("rust"),
            2,
            "session cache invalidation policy\n",
        )
        .unwrap();
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("src/zh.rs"),
            Some("rust"),
            3,
            "工作区权限策略阻止越界访问\n",
        )
        .unwrap();
    let snapshot = catalog.snapshot().unwrap();

    let identifier = snapshot
        .lexical_search(&LexicalSearchRequest::new("LocalWorkspaceAccessPolicy"))
        .unwrap();
    assert_eq!(identifier.hits[0].chunk.path.as_ref(), "src/path_policy.rs");
    let cjk = snapshot
        .lexical_search(&LexicalSearchRequest::new("工作区权限策略"))
        .unwrap();
    assert_eq!(cjk.hits[0].chunk.path.as_ref(), "src/zh.rs");
    let paraphrase = snapshot
        .lexical_search(&LexicalSearchRequest::new("login token validation"))
        .unwrap();
    assert!(paraphrase.hits.is_empty());
}

#[test]
fn incremental_lexical_index_matches_the_locked_native_bm25_fixture() {
    let fixture: RelevanceFixture = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/workspace-retrieval-v1/corpus.json"
    )))
    .unwrap();
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .unwrap();
    for (index, document) in fixture.documents.iter().enumerate() {
        catalog
            .replace_file(
                &WorkspacePath::from_normalized(&document.path),
                document.path.ends_with(".rs").then_some("rust"),
                index as u64 + 1,
                &document.content,
            )
            .unwrap();
    }
    let snapshot = catalog.snapshot().unwrap();

    for query in fixture.queries {
        let result = snapshot
            .lexical_search(&LexicalSearchRequest::new(&query.query))
            .unwrap();
        let paths = result
            .hits
            .iter()
            .map(|hit| hit.chunk.path.to_string())
            .collect::<Vec<_>>();
        assert_eq!(paths, query.expected_bm25_paths, "query {}", query.id);
    }
}

#[test]
fn concurrent_file_replacements_do_not_lose_partitions() {
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .unwrap();
    let threads = (0..16)
        .map(|index| {
            let catalog = Arc::clone(&catalog);
            std::thread::spawn(move || {
                catalog
                    .replace_file(
                        &WorkspacePath::from_normalized(format!("src/file_{index}.rs")),
                        Some("rust"),
                        1,
                        &format!("pub fn file_{index}() {{}}\n"),
                    )
                    .unwrap();
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }

    let snapshot = catalog.snapshot().unwrap();
    assert_eq!(snapshot.file_count(), 16);
    assert_eq!(snapshot.chunk_count(), 16);
}

#[test]
fn eligibility_excludes_sensitive_generated_binary_and_oversized_files() {
    let policy = WorkspaceEligibilityPolicy::default();
    assert!(policy.admits(&manifest_file("src/lib.rs", 20, 1)));
    for path in [
        ".env",
        ".env.production",
        ".a3s/config.acl",
        ".claude/settings.local.json",
        ".codex/session.json",
        ".docker/config.json",
        ".git/config",
        ".npmrc",
        "secrets.json",
        "credentials.toml",
        "keys/server.pem",
    ] {
        assert!(!policy.admits(&manifest_file(path, 20, 1)), "{path}");
    }
    let mut generated = manifest_file("generated.rs", 20, 1);
    generated.generated = true;
    assert!(!policy.admits(&generated));
    let mut binary = manifest_file("blob.bin", 20, 1);
    binary.binary = true;
    assert!(!policy.admits(&binary));
    assert!(!policy.admits(&manifest_file("large.rs", 600 * 1024, 1)));
}

fn manifest_file(path: &str, size: u64, modified_ms: u64) -> LocalWorkspaceFile {
    LocalWorkspaceFile {
        path: path.to_owned(),
        size,
        modified_ms: Some(modified_ms),
        language: path.ends_with(".rs").then(|| "rust".to_owned()),
        status: LocalWorkspaceFileStatus::Tracked,
        binary: false,
        generated: false,
    }
}

#[derive(Clone, Debug, Deserialize)]
struct FixtureDocument {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct RelevanceFixture {
    documents: Vec<FixtureDocument>,
    queries: Vec<RelevanceQuery>,
}

#[derive(Debug, Deserialize)]
struct RelevanceQuery {
    id: String,
    query: String,
    expected_bm25_paths: Vec<String>,
}
