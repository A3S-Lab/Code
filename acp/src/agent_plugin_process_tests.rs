#![cfg(test)]
#![cfg(unix)]

use super::*;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

struct Workspace(std::path::PathBuf);

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn acp_registered_tool_inherits_session_cancellation_and_cleans_its_writer() {
    let root = Workspace(std::env::temp_dir().join(format!(
        "a3s-acp-session-owned-tool-{}",
        uuid::Uuid::new_v4()
    )));
    std::fs::create_dir_all(&root.0).unwrap();
    let entry = root.0.join("owned");
    std::fs::write(
        &entry,
        "#!/bin/sh\ncat >/dev/null; (touch started; sleep 0.40; touch leaked) & wait\n",
    )
    .unwrap();
    std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut launch = tests::test_launch(Some(root.0.join("sessions")));
    launch.workspace = root.0.clone();
    let agent = A3sCodeAgent::new(launch);
    let request = serde_json::from_value(serde_json::json!({
        "cwd": root.0,
        "mcpServers": [],
        "_meta": {
            "a3sTools": [{ "id": "owned", "entry": entry }],
            "a3sHarness": { "components": ["system", "tools", "host:intent_stamp", "budget", "compact", "infer"] }
        }
    })).unwrap();
    let created = acp::Agent::new_session(&agent, request).await.unwrap();
    let session = {
        let sessions = agent.sessions.lock().await;
        Arc::clone(&sessions.get(created.session_id.0.as_ref()).unwrap().session)
    };
    let cancellation = session.session_cancel_token();
    let invocation =
        tokio::spawn(async move { session.tool("owned", serde_json::json!({})).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !root.0.join("started").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("ACP registered tool must actually execute before cancellation");
    cancellation.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), invocation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_ne!(result.exit_code, 0);
    assert!(result.output.contains("cancelled"));
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(
        !root.0.join("leaked").exists(),
        "a cancelled actual ACP session left its plugin writer running"
    );
}
