use std::{sync::Arc, time::Duration};

use tokio_util::sync::CancellationToken;

use super::LanguageRuntime;
use crate::{
    code_intelligence::{
        diagnostics::DiagnosticsStore, document_store::DocumentStore,
        integration_test_support::compile_fake_server, language_profile::LanguageServerProfile,
        project_layout::ProjectLayoutResolver, CodePosition, NavigationKind,
    },
    workspace::{
        LocalWorkspaceFile, LocalWorkspaceFileStatus, LocalWorkspaceManifestSnapshot,
        WorkspaceFileChange, WorkspaceFileChangeKind, WorkspacePath,
    },
};

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

#[tokio::test]
async fn saved_document_runtime_completes_a_real_process_protocol_lifecycle() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let workspace = tempfile::tempdir().unwrap();
    let source_dir = workspace.path().join("src");
    std::fs::create_dir(&source_dir).unwrap();
    std::fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname='fixture'\n",
    )
    .unwrap();
    let source_path = source_dir.join("lib.rs");
    let first_saved = "pub fn answer() -> u32 { 42 }\n";
    std::fs::write(&source_path, first_saved).unwrap();
    let canonical_root = std::fs::canonicalize(workspace.path()).unwrap();

    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "code-intelligence-fake-lsp.exe"
    } else {
        "code-intelligence-fake-lsp"
    });
    compile_fake_server(&server);

    let snapshot = LocalWorkspaceManifestSnapshot {
        version: 7,
        root: canonical_root.clone(),
        files: vec![manifest_file("Cargo.toml"), manifest_file("src/lib.rs")],
        scanned_at_ms: 1,
    };
    let layout = ProjectLayoutResolver::resolve(&snapshot);
    let runtime = LanguageRuntime::start(
        LanguageServerProfile::rust(&server),
        canonical_root,
        layout,
        Arc::new(DocumentStore::new(8)),
        Arc::new(DiagnosticsStore::new(8)),
        CancellationToken::new(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let path = WorkspacePath::from_normalized("src/lib.rs");

    let symbols = runtime
        .document_symbols(&path, first_saved, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(symbols.items.len(), 1);
    assert_eq!(symbols.items[0].name, "answer");
    assert_eq!(symbols.workspace_revision, 7);
    assert!(symbols.document.is_some());

    let workspace_symbols = runtime
        .search_symbols("answer", 10, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(workspace_symbols.items[0].location.path, path);

    let definition = runtime
        .navigate(
            NavigationKind::Definition,
            &path,
            CodePosition::new(0, 7),
            first_saved,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(definition.items[0].path, path);

    let diagnostics = runtime
        .diagnostics(&path, first_saved, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(diagnostics.items.len(), 1);
    assert_eq!(diagnostics.items[0].message, "fixture warning");

    let second_saved = "pub fn answer() -> u32 { 43 }\n";
    std::fs::write(&source_path, second_saved).unwrap();
    let changed = runtime
        .document_symbols(&path, second_saved, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        changed.document.unwrap().revision > symbols.document.unwrap().revision,
        "a newly saved body must advance the document revision"
    );

    runtime
        .notify_file_changes(&[WorkspaceFileChange {
            path: path.clone(),
            kind: WorkspaceFileChangeKind::Changed,
        }])
        .await
        .unwrap();
    runtime.shutdown().await.unwrap();

    let protocol_log = std::fs::read_to_string(server.with_extension("log")).unwrap();
    for method in [
        "initialize",
        "initialized",
        "textDocument/didOpen",
        "textDocument/didSave",
        "textDocument/didChange",
        "textDocument/documentSymbol",
        "workspace/symbol",
        "textDocument/definition",
        "textDocument/diagnostic",
        "textDocument/didClose",
        "workspace/didChangeWatchedFiles",
        "shutdown",
        "exit",
    ] {
        assert!(
            protocol_log.contains(&format!("\"method\":\"{method}\"")),
            "protocol log did not contain {method}: {protocol_log}"
        );
    }
    assert!(
        !protocol_log.contains("\"identifier\":null")
            && !protocol_log.contains("\"previousResultId\":null"),
        "optional diagnostic parameters must be omitted instead of serialized as null: {protocol_log}"
    );
}

#[tokio::test]
async fn publish_only_diagnostics_wait_for_the_current_document_revision() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let workspace = tempfile::tempdir().unwrap();
    let source_dir = workspace.path().join("src");
    std::fs::create_dir(&source_dir).unwrap();
    std::fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname='fixture'\n",
    )
    .unwrap();
    let saved = "pub fn answer() -> u32 { 42 }\n";
    std::fs::write(source_dir.join("lib.rs"), saved).unwrap();
    let canonical_root = std::fs::canonicalize(workspace.path()).unwrap();

    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "push-diagnostics-fake-lsp.exe"
    } else {
        "push-diagnostics-fake-lsp"
    });
    compile_fake_server(&server);
    let snapshot = LocalWorkspaceManifestSnapshot {
        version: 1,
        root: canonical_root.clone(),
        files: vec![manifest_file("Cargo.toml"), manifest_file("src/lib.rs")],
        scanned_at_ms: 1,
    };
    let runtime = LanguageRuntime::start(
        LanguageServerProfile::rust(&server),
        canonical_root,
        ProjectLayoutResolver::resolve(&snapshot),
        Arc::new(DocumentStore::new(1)),
        Arc::new(DiagnosticsStore::new(1)),
        CancellationToken::new(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let path = WorkspacePath::from_normalized("src/lib.rs");

    let query = runtime
        .diagnostics(&path, saved, CancellationToken::new())
        .await;
    runtime.shutdown().await.unwrap();

    let result = query.unwrap();
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].message, "fixture push warning");
    assert_eq!(result.document.unwrap().revision.value(), 1);
    let protocol_log = std::fs::read_to_string(server.with_extension("log")).unwrap();
    assert!(protocol_log.contains("\"method\":\"textDocument/didOpen\""));
    assert!(!protocol_log.contains("\"method\":\"textDocument/diagnostic\""));
}

#[tokio::test]
async fn initialization_settle_delays_readiness_and_honors_cancellation() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let canonical_root = std::fs::canonicalize(workspace.path()).unwrap();
    let server_dir = tempfile::tempdir().unwrap();
    let readiness_server = server_dir.path().join(if cfg!(windows) {
        "code-intelligence-initialization-readiness-lsp.exe"
    } else {
        "code-intelligence-initialization-readiness-lsp"
    });
    compile_fake_server(&readiness_server);
    let snapshot = LocalWorkspaceManifestSnapshot {
        version: 1,
        root: canonical_root.clone(),
        files: vec![manifest_file("Cargo.toml"), manifest_file("src/lib.rs")],
        scanned_at_ms: 1,
    };
    let layout = ProjectLayoutResolver::resolve(&snapshot);

    let started = tokio::time::Instant::now();
    let runtime = LanguageRuntime::start(
        LanguageServerProfile::rust(&readiness_server)
            .with_settle_delays(Duration::from_millis(75), Duration::ZERO),
        canonical_root.clone(),
        layout.clone(),
        Arc::new(DocumentStore::new(1)),
        Arc::new(DiagnosticsStore::new(1)),
        CancellationToken::new(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(60));
    runtime.shutdown().await.unwrap();

    let cancellation_server = server_dir.path().join(if cfg!(windows) {
        "code-intelligence-initialization-cancellation-lsp.exe"
    } else {
        "code-intelligence-initialization-cancellation-lsp"
    });
    compile_fake_server(&cancellation_server);
    let log_path = cancellation_server.with_extension("log");
    let cancellation = CancellationToken::new();
    let startup = tokio::spawn({
        let cancellation = cancellation.clone();
        async move {
            LanguageRuntime::start(
                LanguageServerProfile::rust(&cancellation_server)
                    .with_settle_delays(Duration::from_secs(5), Duration::ZERO),
                canonical_root,
                layout,
                Arc::new(DocumentStore::new(1)),
                Arc::new(DiagnosticsStore::new(1)),
                cancellation,
                Duration::from_secs(5),
            )
            .await
        }
    });
    tokio::time::timeout(
        crate::test_support::external_resource_start_timeout(Duration::from_secs(10)),
        async {
            loop {
                if std::fs::read_to_string(&log_path)
                    .is_ok_and(|log| log.contains("\"method\":\"initialized\""))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        },
    )
    .await
    .expect("the language runtime must enter initialization settling");

    cancellation.cancel();
    let cancelled = tokio::time::timeout(Duration::from_secs(3), startup)
        .await
        .expect("cancellation must interrupt initialization settling")
        .expect("language runtime startup task must not panic");
    assert!(matches!(
        cancelled,
        Err(super::LanguageRuntimeError::Cancelled)
    ));
}

#[tokio::test]
async fn first_navigation_waits_for_empty_and_partial_cold_results_to_settle() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    for mode in ["cold-empty", "cold-partial"] {
        let workspace = tempfile::tempdir().unwrap();
        let source_dir = workspace.path().join("src");
        std::fs::create_dir(&source_dir).unwrap();
        std::fs::write(
            workspace.path().join("Cargo.toml"),
            "[package]\nname='fixture'\n",
        )
        .unwrap();
        let saved = "pub fn answer() -> u32 { 42 }\n";
        std::fs::write(source_dir.join("lib.rs"), saved).unwrap();
        let canonical_root = std::fs::canonicalize(workspace.path()).unwrap();

        let server_dir = tempfile::tempdir().unwrap();
        let server = server_dir.path().join(if cfg!(windows) {
            format!("code-intelligence-{mode}-lsp.exe")
        } else {
            format!("code-intelligence-{mode}-lsp")
        });
        compile_fake_server(&server);
        let snapshot = LocalWorkspaceManifestSnapshot {
            version: 1,
            root: canonical_root.clone(),
            files: vec![manifest_file("Cargo.toml"), manifest_file("src/lib.rs")],
            scanned_at_ms: 1,
        };
        let profile = LanguageServerProfile::rust(&server)
            .with_settle_delays(Duration::ZERO, Duration::from_millis(25));
        let runtime = LanguageRuntime::start(
            profile,
            canonical_root,
            ProjectLayoutResolver::resolve(&snapshot),
            Arc::new(DocumentStore::new(1)),
            Arc::new(DiagnosticsStore::new(1)),
            CancellationToken::new(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let path = WorkspacePath::from_normalized("src/lib.rs");

        let first = runtime
            .navigate(
                NavigationKind::References,
                &path,
                CodePosition::new(0, 7),
                saved,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(first.items.len(), 3, "{mode} did not settle");

        let second = runtime
            .navigate(
                NavigationKind::References,
                &path,
                CodePosition::new(0, 7),
                saved,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(second.items.len(), 3);
        runtime.shutdown().await.unwrap();

        let protocol_log = std::fs::read_to_string(server.with_extension("log")).unwrap();
        assert_eq!(
            protocol_log
                .matches("\"method\":\"textDocument/references\"")
                .count(),
            3,
            "{mode} should retry only the first saved revision"
        );
    }
}

#[tokio::test]
async fn navigation_stabilization_is_cancellable() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let workspace = tempfile::tempdir().unwrap();
    let source_dir = workspace.path().join("src");
    std::fs::create_dir(&source_dir).unwrap();
    std::fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname='fixture'\n",
    )
    .unwrap();
    let saved = "pub fn answer() -> u32 { 42 }\n";
    std::fs::write(source_dir.join("lib.rs"), saved).unwrap();
    let canonical_root = std::fs::canonicalize(workspace.path()).unwrap();

    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "code-intelligence-cancellable-lsp.exe"
    } else {
        "code-intelligence-cancellable-lsp"
    });
    compile_fake_server(&server);
    let snapshot = LocalWorkspaceManifestSnapshot {
        version: 1,
        root: canonical_root.clone(),
        files: vec![manifest_file("Cargo.toml"), manifest_file("src/lib.rs")],
        scanned_at_ms: 1,
    };
    let profile = LanguageServerProfile::rust(&server)
        .with_settle_delays(Duration::ZERO, Duration::from_secs(5));
    let runtime = LanguageRuntime::start(
        profile,
        canonical_root,
        ProjectLayoutResolver::resolve(&snapshot),
        Arc::new(DocumentStore::new(1)),
        Arc::new(DiagnosticsStore::new(1)),
        CancellationToken::new(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();
    let path = WorkspacePath::from_normalized("src/lib.rs");
    let cancellation = CancellationToken::new();
    let trigger = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        trigger.cancel();
    });

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        runtime.navigate(
            NavigationKind::References,
            &path,
            CodePosition::new(0, 7),
            saved,
            cancellation,
        ),
    )
    .await
    .expect("cancellation must interrupt the settle delay");
    assert!(matches!(
        result,
        Err(super::LanguageRuntimeError::Cancelled)
    ));
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn unexpected_process_exit_exposes_state_and_bounded_stderr() {
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let canonical_root = std::fs::canonicalize(workspace.path()).unwrap();
    let server_dir = tempfile::tempdir().unwrap();
    let server = server_dir.path().join(if cfg!(windows) {
        "code-intelligence-crashing-lsp.exe"
    } else {
        "code-intelligence-crashing-lsp"
    });
    compile_fake_server(&server);
    let snapshot = LocalWorkspaceManifestSnapshot {
        version: 1,
        root: canonical_root.clone(),
        files: vec![manifest_file("Cargo.toml"), manifest_file("src/lib.rs")],
        scanned_at_ms: 1,
    };
    let runtime = LanguageRuntime::start(
        LanguageServerProfile::rust(&server),
        canonical_root,
        ProjectLayoutResolver::resolve(&snapshot),
        Arc::new(DocumentStore::new(1)),
        Arc::new(DiagnosticsStore::new(1)),
        CancellationToken::new(),
        Duration::from_secs(5),
    )
    .await
    .unwrap();

    assert!(runtime
        .search_symbols("terminate-process", 1, CancellationToken::new())
        .await
        .is_err());
    let message = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(message) = runtime.unavailable_message().filter(|message| {
                message.contains("code 12") && message.contains("terminated unexpectedly")
            }) {
                break message;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("process state and stderr should settle");
    assert!(
        message.contains("code 12"),
        "unexpected health message: {message}"
    );
    runtime.shutdown().await.unwrap();
}
