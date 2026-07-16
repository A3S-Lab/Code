use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::WorkspaceRuntime;
use crate::{
    code_intelligence::{
        language_profile::LanguageServerProfile,
        project_layout::{ProjectLanguageProfile, ProjectLayoutResolver},
        CodeIntelligenceState,
    },
    workspace::{
        LocalWorkspaceBackend, LocalWorkspaceFile, LocalWorkspaceFileStatus,
        LocalWorkspaceManifestSnapshot, WorkspaceDirEntry, WorkspaceFileSystem, WorkspacePath,
        WorkspaceResult, WorkspaceWriteOutcome,
    },
};

const TEST_QUERY_TIMEOUT: Duration = Duration::from_secs(15);

struct ChangeOnSecondReadFileSystem {
    inner: LocalWorkspaceBackend,
    root: PathBuf,
    target: WorkspacePath,
    replacement: String,
    target_reads: AtomicUsize,
}

impl ChangeOnSecondReadFileSystem {
    fn new(root: PathBuf, target: WorkspacePath, replacement: impl Into<String>) -> Self {
        Self {
            inner: LocalWorkspaceBackend::new(root.clone()),
            root,
            target,
            replacement: replacement.into(),
            target_reads: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl WorkspaceFileSystem for ChangeOnSecondReadFileSystem {
    async fn read_text(&self, path: &WorkspacePath) -> WorkspaceResult<String> {
        if path == &self.target && self.target_reads.fetch_add(1, Ordering::AcqRel) == 1 {
            tokio::fs::write(self.root.join(path.as_str()), &self.replacement)
                .await
                .unwrap();
        }
        self.inner.read_text(path).await
    }

    async fn write_text(
        &self,
        path: &WorkspacePath,
        content: &str,
    ) -> WorkspaceResult<WorkspaceWriteOutcome> {
        self.inner.write_text(path, content).await
    }

    async fn list_dir(&self, path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
        self.inner.list_dir(path).await
    }
}

fn manifest_file(path: &str) -> LocalWorkspaceFile {
    LocalWorkspaceFile {
        path: path.to_owned(),
        size: 1,
        modified_ms: Some(1),
        language: None,
        status: LocalWorkspaceFileStatus::Tracked,
        binary: false,
        generated: false,
    }
}

fn snapshot(root: &Path, paths: &[&str]) -> LocalWorkspaceManifestSnapshot {
    LocalWorkspaceManifestSnapshot {
        version: 7,
        root: root.to_path_buf(),
        files: paths.iter().map(|path| manifest_file(path)).collect(),
        scanned_at_ms: 1,
    }
}

fn write_workspace_files(root: &Path, files: &[(&str, &str)]) {
    for (path, content) in files {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }
}

fn compile_fake_server(output: &Path) {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/code_intelligence_fake_lsp.rs");
    let result = Command::new("rustc")
        .arg("--edition=2021")
        .arg(source)
        .arg("-o")
        .arg(output)
        .output()
        .expect("rustc must be available while Cargo tests are running");
    assert!(
        result.status.success(),
        "failed to compile fake language server: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn test_runtime(
    root: &Path,
    snapshot: &LocalWorkspaceManifestSnapshot,
    profiles: Vec<LanguageServerProfile>,
) -> WorkspaceRuntime {
    let file_system: Arc<dyn WorkspaceFileSystem> =
        Arc::new(LocalWorkspaceBackend::new(root.to_path_buf()));
    WorkspaceRuntime::new_with_profiles(
        root.to_path_buf(),
        ProjectLayoutResolver::resolve(snapshot),
        snapshot,
        file_system,
        TEST_QUERY_TIMEOUT,
        profiles,
    )
}

fn test_runtime_with_file_system(
    root: &Path,
    snapshot: &LocalWorkspaceManifestSnapshot,
    profiles: Vec<LanguageServerProfile>,
    file_system: Arc<dyn WorkspaceFileSystem>,
    document_capacity: usize,
) -> WorkspaceRuntime {
    WorkspaceRuntime::new_with_profiles_and_document_capacity(
        root.to_path_buf(),
        ProjectLayoutResolver::resolve(snapshot),
        snapshot,
        file_system,
        TEST_QUERY_TIMEOUT,
        profiles,
        document_capacity,
    )
}

#[tokio::test]
async fn saved_file_change_during_query_marks_the_document_result_stale() {
    let workspace = tempfile::tempdir().unwrap();
    let original = "pub fn answer() -> u32 { 42 }\n";
    let replacement = "pub fn answer() -> u32 { 43 }\n";
    write_workspace_files(
        workspace.path(),
        &[
            ("Cargo.toml", "[package]\nname='fixture'\n"),
            ("src/lib.rs", original),
        ],
    );
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let snapshot = snapshot(&root, &["Cargo.toml", "src/lib.rs"]);
    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "stale-query-fake-lsp.exe"
    } else {
        "stale-query-fake-lsp"
    });
    compile_fake_server(&server);
    let path = WorkspacePath::from_normalized("src/lib.rs");
    let file_system: Arc<dyn WorkspaceFileSystem> = Arc::new(ChangeOnSecondReadFileSystem::new(
        root.clone(),
        path.clone(),
        replacement,
    ));
    let runtime = test_runtime_with_file_system(
        &root,
        &snapshot,
        vec![LanguageServerProfile::rust(&server)],
        file_system,
        8,
    );

    let result = runtime
        .document_symbols(&path, CancellationToken::new())
        .await
        .unwrap();
    runtime.shutdown().await;

    let document = result.document.unwrap();
    assert!(document.stale);
    assert_eq!(document.content_hash, sha256::digest(original.as_bytes()));
    assert_eq!(
        std::fs::read_to_string(root.join(path.as_str())).unwrap(),
        replacement
    );
}

#[tokio::test]
async fn per_language_document_capacity_evicts_only_from_the_owning_runtime() {
    let workspace = tempfile::tempdir().unwrap();
    write_workspace_files(
        workspace.path(),
        &[
            ("Cargo.toml", "[package]\nname='fixture'\n"),
            ("package.json", "{}\n"),
            ("src/first.rs", "pub fn first() {}\n"),
            ("src/second.rs", "pub fn second() {}\n"),
            ("web/main.ts", "export function main() {}\n"),
        ],
    );
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let snapshot = snapshot(
        &root,
        &[
            "Cargo.toml",
            "package.json",
            "src/first.rs",
            "src/second.rs",
            "web/main.ts",
        ],
    );
    let server_dir = tempfile::tempdir().unwrap();
    let rust_server = server_dir.path().join(if cfg!(windows) {
        "rust-owner-fake-lsp.exe"
    } else {
        "rust-owner-fake-lsp"
    });
    let typescript_server = server_dir.path().join(if cfg!(windows) {
        "typescript-owner-fake-lsp.exe"
    } else {
        "typescript-owner-fake-lsp"
    });
    compile_fake_server(&rust_server);
    compile_fake_server(&typescript_server);
    let file_system: Arc<dyn WorkspaceFileSystem> =
        Arc::new(LocalWorkspaceBackend::new(root.clone()));
    let runtime = test_runtime_with_file_system(
        &root,
        &snapshot,
        vec![
            LanguageServerProfile::rust(&rust_server),
            LanguageServerProfile::typescript_javascript(&typescript_server),
        ],
        file_system,
        1,
    );
    let first_rust = WorkspacePath::from_normalized("src/first.rs");
    let second_rust = WorkspacePath::from_normalized("src/second.rs");
    let typescript = WorkspacePath::from_normalized("web/main.ts");

    runtime
        .document_symbols(&first_rust, CancellationToken::new())
        .await
        .unwrap();
    runtime
        .document_symbols(&typescript, CancellationToken::new())
        .await
        .unwrap();
    runtime
        .document_symbols(&second_rust, CancellationToken::new())
        .await
        .unwrap();

    let rust_slot = runtime
        .slots
        .iter()
        .find(|slot| slot.profile.id() == ProjectLanguageProfile::Rust)
        .unwrap();
    let typescript_slot = runtime
        .slots
        .iter()
        .find(|slot| slot.profile.id() == ProjectLanguageProfile::TypeScriptJavaScript)
        .unwrap();
    assert_eq!(rust_slot.documents.len().await, 1);
    assert!(rust_slot.documents.snapshot(&first_rust).await.is_none());
    assert!(rust_slot.documents.snapshot(&second_rust).await.is_some());
    assert_eq!(typescript_slot.documents.len().await, 1);
    assert!(typescript_slot
        .documents
        .snapshot(&typescript)
        .await
        .is_some());

    let rust_log = std::fs::read_to_string(rust_server.with_extension("log")).unwrap();
    let typescript_log = std::fs::read_to_string(typescript_server.with_extension("log")).unwrap();
    assert_eq!(
        rust_log
            .matches("\"method\":\"textDocument/didClose\"")
            .count(),
        1,
        "the Rust owner must close its own evicted document: {rust_log}"
    );
    assert!(
        !typescript_log.contains("\"method\":\"textDocument/didClose\""),
        "Rust eviction must not close the TypeScript document: {typescript_log}"
    );

    runtime.shutdown().await;
}

#[tokio::test]
async fn fresh_workspace_diagnostics_starts_the_manifest_language_runtime() {
    let workspace = tempfile::tempdir().unwrap();
    write_workspace_files(
        workspace.path(),
        &[
            ("Cargo.toml", "[package]\nname='fixture'\n"),
            ("src/lib.rs", "pub fn answer() -> u32 { 42 }\n"),
        ],
    );
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let snapshot = snapshot(&root, &["Cargo.toml", "src/lib.rs"]);
    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "workspace-diagnostics-fake-lsp.exe"
    } else {
        "workspace-diagnostics-fake-lsp"
    });
    compile_fake_server(&server);
    let runtime = test_runtime(&root, &snapshot, vec![LanguageServerProfile::rust(&server)]);
    let status = runtime.subscribe_status();

    let query = runtime.diagnostics(None, CancellationToken::new()).await;
    let current_status = status.borrow().clone();
    runtime.shutdown().await;

    let result = query.unwrap();
    assert_eq!(result.workspace_revision, 7);
    assert!(result.document.is_none());
    assert!(!result.truncated);
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].location.path.as_str(), "src/lib.rs");
    assert_eq!(result.items[0].message, "fixture warning");
    assert_eq!(current_status.state, CodeIntelligenceState::Ready);
    assert_eq!(current_status.languages.len(), 1);
    assert_eq!(
        current_status.languages[0].state,
        CodeIntelligenceState::Ready
    );

    let protocol_log = std::fs::read_to_string(server.with_extension("log")).unwrap();
    for method in [
        "initialize",
        "textDocument/didOpen",
        "textDocument/diagnostic",
    ] {
        assert!(
            protocol_log.contains(&format!("\"method\":\"{method}\"")),
            "protocol log did not contain {method}: {protocol_log}"
        );
    }
}

#[tokio::test]
async fn mixed_workspace_diagnostics_aggregates_each_supported_language() {
    let workspace = tempfile::tempdir().unwrap();
    write_workspace_files(
        workspace.path(),
        &[
            ("Cargo.toml", "[package]\nname='fixture'\n"),
            ("package.json", "{}\n"),
            ("src/lib.rs", "pub fn answer() -> u32 { 42 }\n"),
            ("web/main.ts", "export function answer() { return 42; }\n"),
        ],
    );
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let snapshot = snapshot(
        &root,
        &["Cargo.toml", "package.json", "src/lib.rs", "web/main.ts"],
    );
    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "mixed-workspace-fake-lsp.exe"
    } else {
        "mixed-workspace-fake-lsp"
    });
    compile_fake_server(&server);
    let runtime = test_runtime(
        &root,
        &snapshot,
        vec![
            LanguageServerProfile::rust(&server),
            LanguageServerProfile::typescript_javascript(&server),
        ],
    );
    let status = runtime.subscribe_status();

    let query = runtime.diagnostics(None, CancellationToken::new()).await;
    let current_status = status.borrow().clone();
    runtime.shutdown().await;

    let result = query.unwrap();
    assert!(!result.truncated);
    assert_eq!(
        result
            .items
            .iter()
            .map(|diagnostic| diagnostic.location.path.as_str())
            .collect::<Vec<_>>(),
        ["src/lib.rs", "web/main.ts"]
    );
    assert_eq!(current_status.state, CodeIntelligenceState::Ready);
    assert_eq!(current_status.languages.len(), 2);
    assert!(current_status
        .languages
        .iter()
        .all(|language| language.state == CodeIntelligenceState::Ready));
}

#[tokio::test]
async fn missing_mixed_language_runtime_returns_available_diagnostics_and_degraded_status() {
    let workspace = tempfile::tempdir().unwrap();
    write_workspace_files(
        workspace.path(),
        &[
            ("Cargo.toml", "[package]\nname='fixture'\n"),
            ("package.json", "{}\n"),
            ("src/lib.rs", "pub fn answer() -> u32 { 42 }\n"),
            ("web/main.ts", "export function answer() { return 42; }\n"),
        ],
    );
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let snapshot = snapshot(
        &root,
        &["Cargo.toml", "package.json", "src/lib.rs", "web/main.ts"],
    );
    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "degraded-workspace-fake-lsp.exe"
    } else {
        "degraded-workspace-fake-lsp"
    });
    compile_fake_server(&server);
    let missing = server_dir.path().join("missing-typescript-language-server");
    let runtime = test_runtime(
        &root,
        &snapshot,
        vec![
            LanguageServerProfile::rust(&server),
            LanguageServerProfile::typescript_javascript(missing),
        ],
    );
    let status = runtime.subscribe_status();

    let query = runtime.diagnostics(None, CancellationToken::new()).await;
    let current_status = status.borrow().clone();
    runtime.shutdown().await;

    let result = query.unwrap();
    assert!(result.truncated);
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].location.path.as_str(), "src/lib.rs");
    assert_eq!(current_status.state, CodeIntelligenceState::Degraded);
    let rust = current_status
        .languages
        .iter()
        .find(|language| language.language.as_str() == "rust")
        .unwrap();
    let typescript = current_status
        .languages
        .iter()
        .find(|language| language.language.as_str() == "typescript-javascript")
        .unwrap();
    assert_eq!(rust.state, CodeIntelligenceState::Ready);
    assert_eq!(typescript.state, CodeIntelligenceState::Unavailable);
    assert!(typescript.message.is_some());
}
