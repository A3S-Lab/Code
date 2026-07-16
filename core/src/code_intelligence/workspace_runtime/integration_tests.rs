use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
use super::integration_test_support::process_exists;
use super::{
    integration_test_support::{compile_fake_server, fixture_started_pids},
    WorkspaceRuntime,
};
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

fn test_runtime(
    root: &Path,
    snapshot: &LocalWorkspaceManifestSnapshot,
    profiles: Vec<LanguageServerProfile>,
) -> WorkspaceRuntime {
    test_runtime_with_timeout(root, snapshot, profiles, TEST_QUERY_TIMEOUT)
}

fn test_runtime_with_timeout(
    root: &Path,
    snapshot: &LocalWorkspaceManifestSnapshot,
    profiles: Vec<LanguageServerProfile>,
    timeout: Duration,
) -> WorkspaceRuntime {
    let file_system: Arc<dyn WorkspaceFileSystem> =
        Arc::new(LocalWorkspaceBackend::new(root.to_path_buf()));
    WorkspaceRuntime::new_with_profiles(
        root.to_path_buf(),
        ProjectLayoutResolver::resolve(snapshot),
        snapshot,
        file_system,
        timeout,
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
async fn first_workspace_symbol_query_opens_a_saved_source_document() {
    let workspace = tempfile::tempdir().unwrap();
    write_workspace_files(
        workspace.path(),
        &[
            ("package.json", "{}\n"),
            (
                "src/main.ts",
                "export function answer(): number { return 42; }\n",
            ),
        ],
    );
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let snapshot = snapshot(&root, &["package.json", "src/main.ts"]);
    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "requires-open-fake-lsp.exe"
    } else {
        "requires-open-fake-lsp"
    });
    compile_fake_server(&server);
    let runtime = test_runtime(
        &root,
        &snapshot,
        vec![LanguageServerProfile::typescript_javascript(&server)],
    );

    let result = runtime
        .search_symbols("answer", 10, CancellationToken::new())
        .await
        .expect("the first workspace symbol query must prepare a language project");

    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].name, "answer");
    assert_eq!(result.items[0].location.path.as_str(), "src/main.ts");
    let log = std::fs::read_to_string(server.with_extension("log")).unwrap();
    let did_open = log.find("\"method\":\"textDocument/didOpen\"").unwrap();
    let workspace_symbol = log.find("\"method\":\"workspace/symbol\"").unwrap();
    assert!(
        did_open < workspace_symbol,
        "the saved source must be opened before workspace symbol search: {log}"
    );

    runtime.shutdown().await;
}

#[tokio::test]
async fn abandoned_caller_does_not_restart_an_in_flight_language_runtime() {
    let workspace = tempfile::tempdir().unwrap();
    write_workspace_files(
        workspace.path(),
        &[
            ("package.json", "{}\n"),
            (
                "src/main.ts",
                "export function answer(): number { return 42; }\n",
            ),
        ],
    );
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let snapshot = snapshot(&root, &["package.json", "src/main.ts"]);
    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "slow-initialize-fake-lsp.exe"
    } else {
        "slow-initialize-fake-lsp"
    });
    compile_fake_server(&server);
    let runtime = Arc::new(test_runtime(
        &root,
        &snapshot,
        vec![LanguageServerProfile::typescript_javascript(&server)],
    ));
    let path = WorkspacePath::from_normalized("src/main.ts");

    let abandoned = {
        let runtime = Arc::clone(&runtime);
        let path = path.clone();
        tokio::spawn(async move {
            runtime
                .document_symbols(&path, CancellationToken::new())
                .await
        })
    };
    let log_path = server.with_extension("log");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_to_string(&log_path)
                .is_ok_and(|log| log.contains("\"method\":\"initialize\""))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the first runtime must begin initialization");

    let surviving = {
        let runtime = Arc::clone(&runtime);
        let path = path.clone();
        tokio::spawn(async move {
            runtime
                .document_symbols(&path, CancellationToken::new())
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(25)).await;
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());

    let result = tokio::time::timeout(Duration::from_secs(3), surviving)
        .await
        .expect("the concurrent waiter must remain bounded")
        .expect("the concurrent waiter task must not fail")
        .expect("the concurrent waiter must reuse the in-flight runtime");
    assert_eq!(result.items.len(), 1);

    let mut status = runtime.subscribe_status();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if status.borrow().state == CodeIntelligenceState::Ready {
                break;
            }
            status.changed().await.expect("runtime status channel");
        }
    })
    .await
    .expect("detached initialization must publish ready status");
    let current_status = status.borrow().clone();
    assert_eq!(current_status.languages.len(), 1);
    assert_eq!(
        current_status.languages[0].state,
        CodeIntelligenceState::Ready
    );
    assert!(current_status.languages[0].capabilities.document_symbols);
    assert!(current_status.languages[0].message.is_none());
    tokio::time::timeout(Duration::from_secs(2), runtime.shutdown())
        .await
        .expect("runtime shutdown must remain bounded");

    let log = std::fs::read_to_string(log_path).unwrap();
    assert_eq!(
        log.matches("\"method\":\"initialize\"").count(),
        1,
        "abandoning one caller must not kill and restart shared initialization: {log}"
    );
}

#[tokio::test]
async fn abandoned_caller_before_start_task_runs_does_not_strand_the_slot() {
    let workspace = tempfile::tempdir().unwrap();
    write_workspace_files(
        workspace.path(),
        &[
            ("package.json", "{}\n"),
            ("src/main.ts", "export function answer() { return 42; }\n"),
        ],
    );
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let snapshot = snapshot(&root, &["package.json", "src/main.ts"]);
    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "preflight-slow-initialize-fake-lsp.exe"
    } else {
        "preflight-slow-initialize-fake-lsp"
    });
    compile_fake_server(&server);
    let runtime = Arc::new(test_runtime(
        &root,
        &snapshot,
        vec![LanguageServerProfile::typescript_javascript(&server)],
    ));
    let path = WorkspacePath::from_normalized("src/main.ts");

    // Hold status publication so the detached task cannot begin its attempt
    // until after the initiating query has been force-abandoned.
    let status_update = runtime.status_updates.lock().await;
    let abandoned = {
        let runtime = Arc::clone(&runtime);
        let path = path.clone();
        tokio::spawn(async move {
            runtime
                .document_symbols(&path, CancellationToken::new())
                .await
        })
    };
    let slot = runtime
        .slots
        .iter()
        .find(|slot| slot.profile.id() == ProjectLanguageProfile::TypeScriptJavaScript)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if matches!(&*slot.state.lock().await, super::SlotState::Starting(_)) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the slot must publish its shared starting generation");
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());
    drop(status_update);

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        runtime.document_symbols(&path, CancellationToken::new()),
    )
    .await
    .expect("a later query must not wait on a stranded starting state")
    .expect("a later query must share the detached start");
    assert_eq!(result.items.len(), 1);
    let status = runtime.subscribe_status().borrow().clone();
    assert_eq!(status.state, CodeIntelligenceState::Ready);
    assert_eq!(status.languages.len(), 1);
    assert_eq!(status.languages[0].state, CodeIntelligenceState::Ready);

    tokio::time::timeout(Duration::from_secs(2), runtime.shutdown())
        .await
        .expect("runtime shutdown must remain bounded");
    let log = std::fs::read_to_string(server.with_extension("log")).unwrap();
    assert_eq!(log.matches("\"method\":\"initialize\"").count(), 1);
}

#[tokio::test]
async fn shutdown_cancels_and_reaps_an_in_flight_language_runtime_start() {
    let workspace = tempfile::tempdir().unwrap();
    write_workspace_files(
        workspace.path(),
        &[
            ("package.json", "{}\n"),
            ("src/main.ts", "export function answer() { return 42; }\n"),
        ],
    );
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let snapshot = snapshot(&root, &["package.json", "src/main.ts"]);
    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "shutdown-slow-initialize-fake-lsp.exe"
    } else {
        "shutdown-slow-initialize-fake-lsp"
    });
    compile_fake_server(&server);
    let runtime = Arc::new(test_runtime(
        &root,
        &snapshot,
        vec![LanguageServerProfile::typescript_javascript(&server)],
    ));
    let query = {
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move {
            runtime
                .document_symbols(
                    &WorkspacePath::from_normalized("src/main.ts"),
                    CancellationToken::new(),
                )
                .await
        })
    };
    let log_path = server.with_extension("log");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_to_string(&log_path)
                .is_ok_and(|log| log.contains("\"method\":\"initialize\""))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the runtime must begin initialization");

    tokio::time::timeout(Duration::from_secs(2), runtime.shutdown())
        .await
        .expect("shutdown must cancel an in-flight startup within its host bound");
    let result = tokio::time::timeout(Duration::from_secs(1), query)
        .await
        .expect("the initiating query must settle after shutdown")
        .expect("the query task must not panic");
    assert!(result.is_err());

    let status = runtime.subscribe_status().borrow().clone();
    assert_eq!(status.state, CodeIntelligenceState::Unavailable);
    assert!(status.languages.is_empty());
    assert_eq!(
        status.message.as_deref(),
        Some("Code Intelligence runtime is shut down")
    );
    let log = std::fs::read_to_string(log_path).unwrap();
    assert_eq!(log.matches("\"method\":\"initialize\"").count(), 1);
}

#[tokio::test]
async fn forced_shutdown_reaps_an_unresponsive_language_runtime() {
    let workspace = tempfile::tempdir().unwrap();
    write_workspace_files(
        workspace.path(),
        &[
            ("package.json", "{}\n"),
            ("src/main.ts", "export function answer() { return 42; }\n"),
        ],
    );
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let snapshot = snapshot(&root, &["package.json", "src/main.ts"]);
    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "ignore-shutdown-fake-lsp.exe"
    } else {
        "ignore-shutdown-fake-lsp"
    });
    compile_fake_server(&server);
    let runtime = Arc::new(test_runtime_with_timeout(
        &root,
        &snapshot,
        vec![LanguageServerProfile::typescript_javascript(&server)],
        Duration::from_secs(10),
    ));
    runtime
        .document_symbols(
            &WorkspacePath::from_normalized("src/main.ts"),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let language_runtime = {
        let state = runtime.slots[0].state.lock().await;
        match &*state {
            super::SlotState::Ready(runtime) => Arc::clone(runtime),
            _ => panic!("language runtime must be ready before forced shutdown"),
        }
    };
    let process_state = language_runtime.subscribe_process_state();
    let log_path = server.with_extension("log");
    let log = std::fs::read_to_string(&log_path).unwrap();
    let pid = *fixture_started_pids(&log).last().unwrap();

    tokio::time::timeout(Duration::from_secs(6), runtime.shutdown())
        .await
        .expect("forced shutdown must remain within the workspace bound");
    assert!(!matches!(
        *process_state.borrow(),
        crate::code_intelligence::lsp::process::LspProcessState::Running
    ));
    #[cfg(unix)]
    assert!(
        !process_exists(pid),
        "forced language process {pid} survived"
    );
    let log = std::fs::read_to_string(log_path).unwrap();
    assert!(log.contains("\"method\":\"shutdown\""));
}

#[tokio::test]
async fn removing_a_source_during_start_reaps_it_before_the_slot_reopens() {
    let workspace = tempfile::tempdir().unwrap();
    write_workspace_files(
        workspace.path(),
        &[
            ("package.json", "{}\n"),
            ("src/main.ts", "export function answer() { return 42; }\n"),
        ],
    );
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let initial = snapshot(&root, &["package.json", "src/main.ts"]);
    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "source-removal-slow-initialize-fake-lsp.exe"
    } else {
        "source-removal-slow-initialize-fake-lsp"
    });
    compile_fake_server(&server);
    let runtime = Arc::new(test_runtime(
        &root,
        &initial,
        vec![LanguageServerProfile::typescript_javascript(&server)],
    ));
    let path = WorkspacePath::from_normalized("src/main.ts");
    let query = {
        let runtime = Arc::clone(&runtime);
        let path = path.clone();
        tokio::spawn(async move {
            runtime
                .document_symbols(&path, CancellationToken::new())
                .await
        })
    };
    let log_path = server.with_extension("log");
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if std::fs::read_to_string(&log_path)
                .is_ok_and(|log| log.contains("\"method\":\"initialize\""))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the removed language runtime must begin initialization");

    tokio::time::timeout(
        Duration::from_secs(5),
        runtime.update_snapshot(&snapshot(&root, &[])),
    )
    .await
    .expect("source removal cleanup must remain bounded");
    assert!(tokio::time::timeout(Duration::from_secs(1), query)
        .await
        .expect("the cancelled query must settle")
        .expect("the cancelled query task must not panic")
        .is_err());
    assert!(matches!(
        *runtime.slots[0].state.lock().await,
        super::SlotState::Dormant
    ));
    let first_log = std::fs::read_to_string(&log_path).unwrap();
    let first_pid = fixture_started_pids(&first_log)[0];
    assert!(first_log.contains(&format!(
        "\"event\":\"process_exiting\",\"pid\":{first_pid}"
    )));
    #[cfg(unix)]
    assert!(
        !process_exists(first_pid),
        "removed language process {first_pid} survived cleanup"
    );

    runtime.update_snapshot(&initial).await;
    runtime
        .document_symbols(&path, CancellationToken::new())
        .await
        .expect("the reopened slot must start a fresh generation");
    let final_log = std::fs::read_to_string(&log_path).unwrap();
    assert_eq!(fixture_started_pids(&final_log).len(), 2);
    assert_eq!(final_log.matches("\"method\":\"initialize\"").count(), 2);
    tokio::time::timeout(Duration::from_secs(2), runtime.shutdown())
        .await
        .expect("restored runtime shutdown must remain bounded");
}

#[tokio::test]
async fn abandoned_multi_language_start_publishes_a_complete_ready_snapshot() {
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
    let rust_server = server_dir.path().join(if cfg!(windows) {
        "rust-slow-initialize-fake-lsp.exe"
    } else {
        "rust-slow-initialize-fake-lsp"
    });
    let typescript_server = server_dir.path().join(if cfg!(windows) {
        "typescript-slow-initialize-fake-lsp.exe"
    } else {
        "typescript-slow-initialize-fake-lsp"
    });
    compile_fake_server(&rust_server);
    compile_fake_server(&typescript_server);
    let runtime = Arc::new(test_runtime(
        &root,
        &snapshot,
        vec![
            LanguageServerProfile::rust(&rust_server),
            LanguageServerProfile::typescript_javascript(&typescript_server),
        ],
    ));
    let abandoned = {
        let runtime = Arc::clone(&runtime);
        tokio::spawn(async move { runtime.diagnostics(None, CancellationToken::new()).await })
    };
    let logs = [
        rust_server.with_extension("log"),
        typescript_server.with_extension("log"),
    ];
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if logs.iter().all(|log| {
                std::fs::read_to_string(log)
                    .is_ok_and(|content| content.contains("\"method\":\"initialize\""))
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both language runtimes must begin initialization");
    abandoned.abort();
    assert!(abandoned.await.unwrap_err().is_cancelled());

    let mut status = runtime.subscribe_status();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let current = status.borrow().clone();
            if current.state == CodeIntelligenceState::Ready
                && current.languages.len() == 2
                && current
                    .languages
                    .iter()
                    .all(|language| language.state == CodeIntelligenceState::Ready)
            {
                break;
            }
            status.changed().await.expect("runtime status channel");
        }
    })
    .await
    .expect("both detached starts must publish a complete ready snapshot");

    tokio::time::timeout(Duration::from_secs(2), runtime.shutdown())
        .await
        .expect("multi-language shutdown must remain bounded");
    for log_path in logs {
        let log = std::fs::read_to_string(log_path).unwrap();
        assert_eq!(log.matches("\"method\":\"initialize\"").count(), 1);
    }
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
