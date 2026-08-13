use super::super::catalog::WorkspaceChunkCatalog;
use super::super::eligibility::WorkspaceEligibilityPolicy;
use super::super::reconcile::WorkspaceCatalogReconciler;
use super::super::{ChunkCatalogLimits, ChunkingConfig, LexicalSearchRequest};
use crate::workspace::{
    LocalWorkspaceFile, LocalWorkspaceFileStatus, LocalWorkspaceManifestSnapshot,
    WorkspaceDirEntry, WorkspaceError, WorkspaceFileChange, WorkspaceFileChangeKind,
    WorkspaceFileSystem, WorkspacePath, WorkspaceResult, WorkspaceWriteOutcome,
};
use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[tokio::test]
async fn dropping_manifest_backend_releases_catalog_and_background_owner() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("README.md"), "ephemeral catalog\n").unwrap();
    let backend = crate::workspace::ManifestWorkspaceBackend::new(temp.path());
    let catalog = backend.chunk_catalog();
    let weak = Arc::downgrade(&catalog);
    drop(catalog);
    drop(backend);

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("catalog allocation remained owned after backend drop");
}

#[tokio::test]
async fn plain_manifest_services_do_not_enable_the_chunk_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let services = crate::workspace::WorkspaceServices::local_with_manifest(temp.path());

    assert!(services.chunk_catalog().is_none());
}

#[tokio::test]
async fn lifecycle_fixture_drives_incremental_and_lag_reconciliation() {
    let fixture: LifecycleFixture = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/workspace-retrieval-v1/lifecycle.json"
    )))
    .unwrap();
    assert_eq!(fixture.schema_version, 1);
    let file_system = Arc::new(CountingFileSystem::default());
    file_system.replace_all(&fixture.initial_documents);
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .unwrap();
    let reconciler = WorkspaceCatalogReconciler::new(
        Arc::clone(&catalog),
        WorkspaceEligibilityPolicy::default(),
        file_system.clone(),
    );
    let mut documents = fixture.initial_documents.clone();
    let mut version = 1u64;
    let initial = snapshot(version, &documents);
    let initial_report = reconciler.reconcile_snapshot(&initial).await.unwrap();
    assert_eq!(initial_report.read_paths.len(), documents.len());

    for step in fixture.steps {
        version += 1;
        apply_step(&mut documents, &step);
        file_system.replace_all(&documents);
        file_system.take_reads();
        let snapshot = snapshot(version, &documents);
        let report = match step.id.as_str() {
            "unchanged-rescan" => reconciler.reconcile_snapshot(&snapshot).await.unwrap(),
            "lagged-reconcile" => reconciler.reconcile_after_lag(&snapshot).await.unwrap(),
            _ => {
                let changes = step_changes(&step);
                reconciler
                    .reconcile_changes(&snapshot, &changes)
                    .await
                    .unwrap()
            }
        };
        assert!(
            report.failures.is_empty(),
            "step {}: {:?}",
            step.id,
            report.failures
        );
        assert_eq!(
            file_system.take_reads(),
            step.expected_read_paths,
            "step {} read drift",
            step.id
        );
        assert_eq!(
            catalog.snapshot().unwrap().paths(),
            step.expected_catalog_paths,
            "step {} catalog drift",
            step.id
        );
    }
}

#[tokio::test]
async fn initial_reconciliation_publishes_only_after_reading_the_first_corpus() {
    let documents = vec![FixtureDocument {
        path: "src/lib.rs".to_owned(),
        content: "initial corpus\n".to_owned(),
    }];
    let file_system = Arc::new(CountingFileSystem::default());
    file_system.replace_all(&documents);
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .unwrap();
    let reconciler = WorkspaceCatalogReconciler::new(
        Arc::clone(&catalog),
        WorkspaceEligibilityPolicy::default(),
        file_system,
    );

    let report = reconciler
        .reconcile_snapshot(&snapshot(1, &documents))
        .await
        .unwrap();

    assert_eq!(report.catalog_revision, 1);
    assert_eq!(catalog.snapshot().unwrap().source_revision(), 1);
}

#[tokio::test]
async fn failed_replacement_tombstones_old_content_before_reporting_partial_coverage() {
    let file_system = Arc::new(CountingFileSystem::default());
    let old = vec![FixtureDocument {
        path: "src/secret.rs".to_owned(),
        content: "old searchable sentinel\n".to_owned(),
    }];
    file_system.replace_all(&old);
    let catalog =
        WorkspaceChunkCatalog::new(ChunkingConfig::default(), ChunkCatalogLimits::default())
            .unwrap();
    let reconciler = WorkspaceCatalogReconciler::new(
        Arc::clone(&catalog),
        WorkspaceEligibilityPolicy::default(),
        file_system.clone(),
    );
    reconciler
        .reconcile_snapshot(&snapshot(1, &old))
        .await
        .unwrap();
    assert_eq!(catalog.snapshot().unwrap().paths(), ["src/secret.rs"]);

    let changed = vec![FixtureDocument {
        path: "src/secret.rs".to_owned(),
        content: "new unavailable content\n".to_owned(),
    }];
    file_system.replace_all(&[]);
    let report = reconciler
        .reconcile_changes(
            &snapshot(2, &changed),
            &[WorkspaceFileChange {
                path: WorkspacePath::from_normalized("src/secret.rs"),
                kind: WorkspaceFileChangeKind::Changed,
            }],
        )
        .await
        .unwrap();

    assert_eq!(report.failures.len(), 1);
    let published = catalog.snapshot().unwrap();
    assert_eq!(published.source_revision(), 2);
    assert!(published.paths().is_empty());
    assert!(published
        .lexical_search(&LexicalSearchRequest::new("old searchable sentinel"))
        .unwrap()
        .hits
        .is_empty());
}

#[tokio::test]
async fn reconciliation_keeps_a_safe_subset_when_one_file_exceeds_the_budget() {
    let documents = vec![
        FixtureDocument {
            path: "a-too-large.rs".to_owned(),
            content: "x".repeat(64),
        },
        FixtureDocument {
            path: "b-small.rs".to_owned(),
            content: "small searchable token\n".to_owned(),
        },
    ];
    let file_system = Arc::new(CountingFileSystem::default());
    file_system.replace_all(&documents);
    let catalog = WorkspaceChunkCatalog::new(
        ChunkingConfig::default(),
        ChunkCatalogLimits {
            max_files: 8,
            max_chunks: 8,
            max_text_bytes: 32,
            max_index_bytes: 1024 * 1024,
        },
    )
    .unwrap();
    let reconciler = WorkspaceCatalogReconciler::new(
        Arc::clone(&catalog),
        WorkspaceEligibilityPolicy::default(),
        file_system,
    );

    let report = reconciler
        .reconcile_snapshot(&snapshot(1, &documents))
        .await
        .unwrap();

    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].path, "a-too-large.rs");
    assert_eq!(catalog.snapshot().unwrap().paths(), ["b-small.rs"]);
    let result = catalog
        .snapshot()
        .unwrap()
        .lexical_search(&LexicalSearchRequest::new("searchable token"))
        .unwrap();
    assert_eq!(result.hits[0].chunk.path.as_ref(), "b-small.rs");
}

fn snapshot(version: u64, documents: &[FixtureDocument]) -> LocalWorkspaceManifestSnapshot {
    LocalWorkspaceManifestSnapshot {
        version,
        root: std::path::PathBuf::from("fixture"),
        files: documents
            .iter()
            .map(|document| {
                manifest_file(
                    &document.path,
                    document.content.len() as u64,
                    content_revision(&document.content),
                )
            })
            .collect(),
        scanned_at_ms: version,
    }
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

fn content_revision(content: &str) -> u64 {
    let digest = Sha256::digest(content.as_bytes());
    u64::from_le_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 prefix is eight bytes"),
    )
}

fn apply_step(documents: &mut Vec<FixtureDocument>, step: &LifecycleStep) {
    match step.operation.as_str() {
        "reconcile" => *documents = step.documents.clone(),
        "upsert" => {
            for replacement in &step.documents {
                documents.retain(|document| document.path != replacement.path);
                documents.push(replacement.clone());
            }
        }
        "rename" => {
            documents.retain(|document| Some(&document.path) != step.from_path.as_ref());
            documents.extend(step.documents.clone());
        }
        "delete" => {
            documents.retain(|document| Some(&document.path) != step.from_path.as_ref());
        }
        operation => panic!("unexpected fixture operation: {operation}"),
    }
    documents.sort_by(|left, right| left.path.cmp(&right.path));
}

fn step_changes(step: &LifecycleStep) -> Vec<WorkspaceFileChange> {
    let mut changes = Vec::new();
    if let Some(path) = &step.from_path {
        changes.push(WorkspaceFileChange {
            path: WorkspacePath::from_normalized(path),
            kind: WorkspaceFileChangeKind::Deleted,
        });
    }
    changes.extend(step.documents.iter().map(|document| WorkspaceFileChange {
        path: WorkspacePath::from_normalized(&document.path),
        kind: if step.id == "change" {
            WorkspaceFileChangeKind::Changed
        } else {
            WorkspaceFileChangeKind::Created
        },
    }));
    changes
}

#[derive(Clone, Debug, Deserialize)]
struct FixtureDocument {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct LifecycleFixture {
    schema_version: u32,
    initial_documents: Vec<FixtureDocument>,
    steps: Vec<LifecycleStep>,
}

#[derive(Debug, Deserialize)]
struct LifecycleStep {
    id: String,
    operation: String,
    #[serde(default)]
    from_path: Option<String>,
    documents: Vec<FixtureDocument>,
    expected_read_paths: Vec<String>,
    expected_catalog_paths: Vec<String>,
}

#[derive(Default)]
struct CountingFileSystem {
    files: Mutex<BTreeMap<String, String>>,
    reads: Mutex<Vec<String>>,
}

impl CountingFileSystem {
    fn replace_all(&self, documents: &[FixtureDocument]) {
        let mut files = self.files.lock().unwrap();
        *files = documents
            .iter()
            .map(|document| (document.path.clone(), document.content.clone()))
            .collect();
    }

    fn take_reads(&self) -> Vec<String> {
        let mut reads = self.reads.lock().unwrap();
        reads.sort();
        std::mem::take(&mut *reads)
    }
}

#[async_trait]
impl WorkspaceFileSystem for CountingFileSystem {
    async fn read_text(&self, path: &WorkspacePath) -> WorkspaceResult<String> {
        self.reads.lock().unwrap().push(path.as_str().to_owned());
        self.files
            .lock()
            .unwrap()
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| WorkspaceError::NotFound {
                path: path.as_str().to_owned(),
            })
    }

    async fn write_text(
        &self,
        path: &WorkspacePath,
        content: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome> {
        self.files
            .lock()
            .unwrap()
            .insert(path.as_str().to_owned(), content.to_owned());
        Ok(WorkspaceWriteOutcome {
            bytes: content.len(),
            lines: content.lines().count(),
        })
    }

    async fn list_dir(&self, _path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
        Ok(Vec::new())
    }
}
