#![cfg(test)]

use super::*;

#[test]
fn source_selection_is_exact_and_never_invents_criteria_or_unpauses() {
    let services = WorkspaceServices::local("/workspace");
    let raw = r#"{"objective":"  保留原目标  ","paused":true}"#;
    let first = parse_source("session", services.workspace_ref(), raw, None).unwrap();
    assert_eq!(first.goal.objective, "  保留原目标  ");
    assert!(first.goal.paused);
    let same = parse_source(
        "session",
        services.workspace_ref(),
        raw,
        Some(&first.source),
    )
    .unwrap();
    assert_eq!(same.source, first.source);
    assert!(parse_source("other", services.workspace_ref(), raw, Some(&first.source)).is_err());
    assert!(parse_source(
        "session",
        services.workspace_ref(),
        &(raw.to_owned() + "\n"),
        Some(&first.source)
    )
    .is_err());
    let other = WorkspaceServices::local("/other");
    assert!(parse_source("session", other.workspace_ref(), raw, Some(&first.source)).is_err());
    let wire = serde_json::to_value(first).unwrap();
    assert!(wire["goal"].get("success_criteria").is_none());
    assert!(wire.get("execution").is_none());
}

#[test]
fn malformed_ambiguous_and_oversize_sources_refuse() {
    let services = WorkspaceServices::local("/workspace");
    for raw in [
        "",
        "null",
        "[]",
        "{}",
        "invalid",
        r#"[{"objective":"one","paused":false},{"objective":"two","paused":true}]"#,
        r#"{"objective":"one"}"#,
        r#"{"objective":"one","paused":"false"}"#,
        r#"{"objective":"one","objective":"two","paused":false}"#,
        r#"{"objective":"one","paused":false,"paused":true}"#,
        r#"{"objective":"one","paused":false,"session_id":"other"}"#,
        r#"{"objective":" \n ","paused":false}"#,
    ] {
        assert!(
            parse_source("session", services.workspace_ref(), raw, None).is_err(),
            "{raw}"
        );
    }
    for objective in ["x".repeat(4096), "目".repeat(1365)] {
        let raw = serde_json::json!({"objective":objective,"paused":false}).to_string();
        assert!(parse_source("session", services.workspace_ref(), &raw, None).is_ok());
    }
    for objective in ["x".repeat(4097), "目".repeat(1366)] {
        let raw = serde_json::json!({"objective":objective,"paused":false}).to_string();
        assert!(parse_source("session", services.workspace_ref(), &raw, None).is_err());
    }
    let raw = " ".repeat(MAX_SOURCE_BYTES) + r#"{"objective":"one","paused":false}"#;
    assert!(parse_source("session", services.workspace_ref(), &raw, None).is_err());
    for raw in [
        r#"{"session_id":"s"}"#,
        r#"{"session_id":"s","expected_source":null,"path":"elsewhere"}"#,
    ] {
        assert!(serde_json::from_str::<GoalImportPreviewRequest>(raw).is_err());
    }
}

#[tokio::test]
async fn bounded_reader_keeps_the_source_and_refuses_special_files() {
    let root = tempfile::tempdir().unwrap();
    let request = GoalImportPreviewRequest {
        session_id: "session".into(),
        expected_source: None,
    };
    assert!(preview_import(root.path(), &request).await.is_err());
    assert!(!root.path().join(".a3s").exists());
    std::fs::create_dir_all(root.path().join(".a3s")).unwrap();
    std::fs::write(
        root.path().join(SOURCE_PATH),
        r#"{"objective":"retained","paused":true}"#,
    )
    .unwrap();
    let path = root.path().join(SOURCE_PATH);
    let before = std::fs::read(&path).unwrap();
    assert!(
        preview_import(root.path(), &request)
            .await
            .unwrap()
            .goal
            .paused
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::write(
        &path,
        r#"{"objective":"  legacy  ","extra":"retained compatibility"}"#,
    )
    .unwrap();
    assert!(preview_import(root.path(), &request).await.is_err());
    std::fs::write(&path, vec![b' '; MAX_SOURCE_BYTES + 1]).unwrap();
    assert!(preview_import(root.path(), &request).await.is_err());
    std::fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(preview_import(root.path(), &request).await.is_err());
    assert!(super::super::load_goal(root.path(), "unclaimed")
        .await
        .unwrap()
        .is_none());
    std::fs::File::create(&path)
        .unwrap()
        .set_len(16 * 1024 * 1024 + 1)
        .unwrap();
    assert!(super::super::load_goal(root.path(), "unclaimed")
        .await
        .unwrap()
        .is_none());
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(preview_import(root.path(), &request).await.is_err());
    std::fs::remove_dir(&path).unwrap();
    #[cfg(unix)]
    {
        let target = root.path().join("elsewhere");
        std::fs::write(&target, before).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(preview_import(root.path(), &request).await.is_err());
        assert!(super::super::load_goal(root.path(), "unclaimed")
            .await
            .unwrap()
            .is_none());
        std::fs::remove_file(&path).unwrap();
        assert!(std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            assert!(preview_import(root.path(), &request).await.is_err());
            assert!(super::super::load_goal(root.path(), "unclaimed")
                .await
                .unwrap()
                .is_none());
        })
        .await
        .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(root.path().join(".a3s")).unwrap();
        std::fs::create_dir(root.path().join("outside")).unwrap();
        std::os::unix::fs::symlink(root.path().join("outside"), root.path().join(".a3s")).unwrap();
        std::fs::write(root.path().join(SOURCE_PATH), "{}").unwrap();
        assert!(preview_import(root.path(), &request).await.is_err());
    }
}
