#![cfg(test)]

use super::*;

#[tokio::test]
async fn navigation_explicit_mount_shares_tool_manifest_and_reaches_actual_model_once() {
    use crate::context::RecentWorkspaceFilesContextProvider;
    use crate::workspace::{ManifestWorkspaceBackend, WorkspaceServices};
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("资料")).unwrap();
    std::fs::write(
        root.path().join("资料/最新.rs"),
        "CONTENT-MUST-NOT-BE-IN-NAVIGATION",
    )
    .unwrap();
    let backend = ManifestWorkspaceBackend::new_deferred(root.path());
    let manifest = backend.manifest();
    let mut ready = manifest.subscribe();
    assert!(manifest.activate());
    tokio::time::timeout(std::time::Duration::from_secs(10), ready.recv())
        .await
        .unwrap()
        .unwrap();
    manifest.touch_file("资料/最新.rs");
    let systems = Arc::new(std::sync::Mutex::new(vec![]));
    let provider = Arc::new(RecentWorkspaceFilesContextProvider::new(manifest.clone()));
    let agent = durable_source::source_agent().await;
    let session = agent
        .session_async(
            root.path().to_string_lossy(),
            Some(
                crate::SessionOptions::new()
                    .with_session_id("navigation-mount")
                    .with_llm_client(Arc::new(SkillCapturingClient {
                        systems: systems.clone(),
                    }))
                    .with_workspace_backend(WorkspaceServices::local_with_manifest_backend(
                        backend.clone(),
                    ))
                    .with_context_provider(provider.clone())
                    .with_context_provider(provider)
                    .with_planning_mode(crate::PlanningMode::Disabled)
                    .with_auto_delegation_enabled(false)
                    .with_continuation(false)
                    .with_permission_policy(
                        crate::permissions::PermissionPolicy::new()
                            .deny("write")
                            .deny("bash"),
                    ),
            ),
        )
        .await
        .unwrap();
    session.send("What file did I touch?", None).await.unwrap();
    {
        let captured = systems.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0].matches("a3s://workspace/recent-files").count(),
            1
        );
        assert!(captured[0].contains("Recent: 资料/最新.rs"));
        assert!(captured[0].contains("Directory: 资料/"));
        assert!(!captured[0].contains("CONTENT-MUST-NOT-BE-IN-NAVIGATION"));
    }
    assert!(Arc::ptr_eq(&manifest, &backend.manifest()));
    assert!(!manifest.activate(), "no second activation");
    let facts = read_workspace_facts(root.path(), "navigation-mount").unwrap();
    assert_eq!(
        last_context_sections(&facts)
            .iter()
            .filter(|(provider, _)| provider == "recent_workspace_files")
            .count(),
        1
    );
    assert!(!facts.iter().any(|fact| fact.kind == "tool.call"));
    session.close().await;
    manifest.shutdown();
}
