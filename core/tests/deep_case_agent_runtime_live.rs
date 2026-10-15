//! Deep case `agent_runtime`.
//!
//! One real completion on the pinned Flash model happens inside the scenario.
//! That completion is not the pass condition. The pass condition is
//! `create_session` returning `SessionClosed` after the agent is closed.

mod support;

use std::time::Duration;

use a3s_code_core::{Agent, CodeError, SessionOptions};
use support::layer_c_model::{load_pinned_layer_c_config, pinned_layer_c_model};

#[tokio::test]
async fn agent_runtime_closed_agent_rejects_the_next_session() {
    let config = load_pinned_layer_c_config();
    let model = pinned_layer_c_model(&config);
    let agent = Agent::from_config(config)
        .await
        .expect("build real-provider agent");
    let workspace =
        std::env::temp_dir().join(format!("a3s-deep-agent-runtime-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).expect("workspace");
    let options = SessionOptions::new()
        .with_session_id("deep-agent-runtime")
        .with_model(model)
        .with_max_tool_rounds(1)
        .with_temperature(0.0);
    let session = agent
        .session_async(workspace.to_string_lossy().as_ref(), Some(options.clone()))
        .await
        .expect("admit the session");
    tokio::time::timeout(
        Duration::from_secs(180),
        session.send("Reply with the single word ready.", None),
    )
    .await
    .expect("model completion timed out")
    .expect("model completion");

    agent.close().await;
    let again = agent
        .session_async(workspace.to_string_lossy().as_ref(), Some(options))
        .await;
    match again {
        Err(CodeError::SessionClosed { .. }) => {}
        other => panic!("expected SessionClosed from create_session, got {other:?}"),
    }
}
