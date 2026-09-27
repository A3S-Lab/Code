//! Real-model path the a3s code TUI uses: ACP `new_session` + `prompt`.
//!
//! Oracles are the workspace file and, when the turn is blocked, the
//! completion-gate error the pager shows as TurnFailed. Assistant prose is
//! not an oracle.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-acp --test live_tui_prompt -- --ignored --test-threads=1 --nocapture
//! ```

use std::path::PathBuf;
use std::time::Duration;

use a3s_code_acp::{load_launch_config, A3sCodeAgent, LaunchConfig};
use agent_client_protocol as acp;

const TOKEN: &str = "tui-acp-marker-6c1d";
const TIMEOUT: Duration = Duration::from_secs(420);

fn pin_test_model(launch: &mut LaunchConfig) {
    let Ok(raw) = std::env::var("A3S_TEST_MODEL") else {
        return;
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return;
    }
    let model = if raw == "boyue/bailian/deepseek-v4-flash" {
        "boyue/bailian/deepseek-v4.1-flash".to_string()
    } else {
        raw.to_string()
    };
    launch.model_id = model.clone();
    launch.config.default_model = Some(model);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model configured in .a3s/config.acl"]
async fn live_prompt_copies_the_marker_through_the_tui_agent() {
    let config_path = std::env::var_os("A3S_CONFIG_FILE").map(PathBuf::from);
    let workspace = std::env::temp_dir().join(format!("a3s-tui-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join("marker.txt"), TOKEN).expect("marker");

    let mut launch = load_launch_config(workspace.clone(), config_path).expect("launch config");
    pin_test_model(&mut launch);
    let agent = A3sCodeAgent::new(launch);

    let created = acp::Agent::new_session(&agent, acp::NewSessionRequest::new(workspace.clone()))
        .await
        .expect("ACP session");
    let prompt = format!(
        "Using the write tool, create copied.txt containing exactly the contents of marker.txt. \
         Do not modify marker.txt. Then stop."
    );
    let result = tokio::time::timeout(
        TIMEOUT,
        acp::Agent::prompt(
            &agent,
            acp::PromptRequest::new(
                created.session_id,
                vec![acp::ContentBlock::Text(acp::TextContent::new(prompt))],
            ),
        ),
    )
    .await
    .expect("prompt timed out");

    let copied = std::fs::read_to_string(workspace.join("copied.txt")).unwrap_or_default();
    let marker = std::fs::read_to_string(workspace.join("marker.txt")).unwrap_or_default();
    assert_eq!(marker, TOKEN, "marker.txt must stay unchanged");
    assert!(
        copied.contains(TOKEN),
        "write through ACP prompt must copy the marker, got {copied:?}; prompt result: {result:?}"
    );
    if let Err(error) = &result {
        let detail = error
            .data
            .as_ref()
            .and_then(|value| value.as_str())
            .unwrap_or("");
        assert!(
            detail.contains("completion gate:"),
            "a blocked turn must be the completion gate, got {detail:?}"
        );
    }
}
