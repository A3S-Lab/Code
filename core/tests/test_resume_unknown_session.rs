//! U-RT-03: resuming an unknown session id fail-closes and does not create a
//! store file for that id.

use std::path::Path;
use std::sync::Arc;

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::store::FileSessionStore;
use a3s_code_core::{Agent, SessionOptions};

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
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
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
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
async fn resume_unknown_session_does_not_create_a_store_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(
        FileSessionStore::new(dir.path())
            .await
            .expect("open empty session store"),
    );
    let before = file_count(dir.path());
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session_id = "missing-u-rt-03";

    for _ in 0..2 {
        let error = agent
            .resume_session_async(
                session_id,
                SessionOptions::new().with_session_store(
                    Arc::clone(&store) as Arc<dyn a3s_code_core::store::SessionStore>
                ),
            )
            .await
            .expect_err("unknown id must fail closed");
        let message = error.to_string();
        assert!(
            message.contains("Session not found"),
            "expected a typed not-found error, got {message}"
        );
    }

    assert_eq!(
        file_count(dir.path()),
        before,
        "resume of a missing id must not write a session file"
    );
    let leaked = std::fs::read_dir(dir.path()).unwrap().any(|entry| {
        entry
            .ok()
            .map(|entry| entry.file_name().to_string_lossy().contains(session_id))
            .unwrap_or(false)
    });
    assert!(!leaked, "store must not contain the missing session id");
}
