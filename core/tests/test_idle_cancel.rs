//! U-CV-01: cancelling a session that has not started a turn is an idle
//! receipt. It must not append a synthetic transcript entry.

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::{Agent, SessionOptions};
use std::path::Path;

fn offline_config(sessions: &Path) -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        sessions_dir: Some(sessions.to_path_buf()),
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("offline-key".to_string()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4".to_string(),
                family: "claude-sonnet".to_string(),
                api_key: None,
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

fn file_count(root: &Path) -> usize {
    let mut count = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                count += 1;
            }
        }
    }
    count
}

#[tokio::test]
async fn idle_cancel_does_not_write_a_transcript() {
    let workspace = tempfile::tempdir().unwrap();
    let sessions = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(offline_config(sessions.path()))
        .await
        .unwrap();
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("u-cv-01")
                    .with_model("anthropic/claude-sonnet-4-20250514"),
            ),
        )
        .await
        .unwrap();

    assert!(session.history().is_empty());
    let stored = file_count(sessions.path());
    let workspace_files = file_count(workspace.path());

    assert!(
        !session.cancel().await,
        "cancel before any send must be an idle receipt"
    );
    assert!(session.history().is_empty());
    assert_eq!(file_count(sessions.path()), stored);
    assert_eq!(file_count(workspace.path()), workspace_files);

    assert!(!session.cancel().await);
    assert!(session.history().is_empty());
}
