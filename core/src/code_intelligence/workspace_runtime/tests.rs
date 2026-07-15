use super::*;
use crate::code_intelligence::{project_layout::ProjectLayoutResolver, CodeRange, LanguageId};
use crate::workspace::{LocalWorkspaceBackend, LocalWorkspaceFile, LocalWorkspaceFileStatus};

fn snapshot(root: &Path, version: u64, paths: &[&str]) -> LocalWorkspaceManifestSnapshot {
    LocalWorkspaceManifestSnapshot {
        version,
        root: root.to_path_buf(),
        files: paths
            .iter()
            .map(|path| LocalWorkspaceFile {
                path: (*path).to_owned(),
                size: 1,
                modified_ms: Some(1),
                language: None,
                status: LocalWorkspaceFileStatus::Tracked,
                binary: false,
                generated: false,
            })
            .collect(),
        scanned_at_ms: 1,
    }
}

#[test]
fn stopped_runtime_status_is_retained_without_live_subscribers() {
    let rust_capabilities = CodeIntelligenceCapabilities {
        definition: true,
        ..CodeIntelligenceCapabilities::default()
    };
    let web_capabilities = CodeIntelligenceCapabilities {
        references: true,
        ..CodeIntelligenceCapabilities::default()
    };
    let (status, receiver) = watch::channel(CodeIntelligenceStatus {
        state: CodeIntelligenceState::Ready,
        capabilities: CodeIntelligenceCapabilities {
            definition: true,
            references: true,
            ..CodeIntelligenceCapabilities::default()
        },
        languages: vec![
            CodeIntelligenceLanguageStatus {
                language: LanguageId::from("rust"),
                state: CodeIntelligenceState::Ready,
                capabilities: rust_capabilities,
                message: None,
            },
            CodeIntelligenceLanguageStatus {
                language: LanguageId::from("typescript-javascript"),
                state: CodeIntelligenceState::Ready,
                capabilities: web_capabilities,
                message: None,
            },
        ],
        message: None,
    });
    drop(receiver);

    publish_stopped_language_status(
        &status,
        LanguageId::from("rust"),
        "process exited".to_owned(),
    );

    let current = status.borrow();
    assert_eq!(current.state, CodeIntelligenceState::Degraded);
    assert!(!current.capabilities.definition);
    assert!(current.capabilities.references);
    assert_eq!(
        current.languages[0].state,
        CodeIntelligenceState::Unavailable
    );
    assert_eq!(
        current.languages[0].message.as_deref(),
        Some("process exited")
    );
}

#[test]
fn workspace_diagnostic_aggregation_never_exceeds_the_hard_limit() {
    let diagnostic = |index| CodeDiagnostic {
        location: CodeLocation {
            path: WorkspacePath::from_normalized(format!("src/{index}.rs")),
            range: CodeRange::new(CodePosition::new(0, 0), CodePosition::new(0, 1)),
        },
        severity: None,
        code: None,
        source: Some("test".to_owned()),
        message: format!("diagnostic-{index}"),
    };
    let mut items = (0..WORKSPACE_DIAGNOSTIC_LIMIT - 1)
        .map(diagnostic)
        .collect::<Vec<_>>();

    let truncated = append_bounded(
        &mut items,
        vec![
            diagnostic(WORKSPACE_DIAGNOSTIC_LIMIT),
            diagnostic(WORKSPACE_DIAGNOSTIC_LIMIT + 1),
        ],
        WORKSPACE_DIAGNOSTIC_LIMIT,
    );

    assert!(truncated);
    assert_eq!(items.len(), WORKSPACE_DIAGNOSTIC_LIMIT);
    assert_eq!(
        items.last().unwrap().message,
        format!("diagnostic-{WORKSPACE_DIAGNOSTIC_LIMIT}")
    );
}

#[test]
fn workspace_diagnostic_selection_round_robins_languages() {
    let slots = vec![
        LanguageSlot::new(LanguageServerProfile::rust("rust"), true, DOCUMENT_CAPACITY),
        LanguageSlot::new(
            LanguageServerProfile::typescript_javascript("typescript"),
            true,
            DOCUMENT_CAPACITY,
        ),
    ];
    let paths = vec![
        WorkspacePath::from_normalized("src/a.rs"),
        WorkspacePath::from_normalized("src/b.rs"),
        WorkspacePath::from_normalized("web/main.ts"),
    ];

    let (selected, truncated) = select_workspace_diagnostic_paths(&slots, &[0, 1], &paths, 2);

    assert!(truncated);
    assert_eq!(selected[0], (0, WorkspacePath::from_normalized("src/a.rs")));
    assert_eq!(
        selected[1],
        (1, WorkspacePath::from_normalized("web/main.ts"))
    );
}

#[tokio::test]
async fn removing_the_last_supported_source_resets_failed_runtime_state() {
    let workspace = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(workspace.path()).unwrap();
    let initial = snapshot(&root, 1, &["src/lib.rs"]);
    let file_system: Arc<dyn WorkspaceFileSystem> =
        Arc::new(LocalWorkspaceBackend::new(root.clone()));
    let runtime = WorkspaceRuntime::new(
        root.clone(),
        ProjectLayoutResolver::resolve(&initial),
        &initial,
        file_system,
        Duration::from_secs(1),
    );
    let rust = runtime
        .slots
        .iter()
        .find(|slot| slot.profile.id() == ProjectLanguageProfile::Rust)
        .unwrap();
    *rust.state.lock().await = SlotState::Failed(StartFailure {
        at: Instant::now(),
        message: "failed".to_owned(),
    });

    runtime.update_snapshot(&snapshot(&root, 2, &[])).await;

    assert!(!rust.relevant.load(Ordering::Acquire));
    assert!(matches!(*rust.state.lock().await, SlotState::Dormant));
}
