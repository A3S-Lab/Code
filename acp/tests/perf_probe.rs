//! Timing probe for the real pager→ACP chain: submit→first chunk→last
//! chunk→prompt return. Ignore-gated; run with the model ACL configured.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-acp --test perf_probe -- --ignored --nocapture --test-threads=1
//! ```

use std::path::PathBuf;
use std::time::{Duration, Instant};

fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("perf_probe=info"))

        .try_init();
}

use a3s_code_acp::{load_launch_config, A3sCodeAgent, LaunchConfig};
use agent_client_protocol as acp;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model configured in .a3s/config.acl"]
async fn times_a_plain_text_turn_end_to_end() {
    let config_path = std::env::var_os("A3S_CONFIG_FILE").map(PathBuf::from);
    let workspace = std::env::temp_dir().join(format!("a3s-perf-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(&workspace).expect("workspace");

    let mut launch = load_launch_config(workspace.clone(), config_path).expect("launch config");
    if let Ok(raw) = std::env::var("A3S_TEST_MODEL") {
        let model = if raw.trim() == "boyue/bailian/deepseek-v4-flash" {
            "boyue/bailian/deepseek-v4.1-flash".to_string()
        } else {
            raw.trim().to_string()
        };
        launch.model_id = model.clone();
        launch.config.default_model = Some(model);
    }
    init_logging();
    let agent = A3sCodeAgent::new(launch);
    let created = acp::Agent::new_session(&agent, acp::NewSessionRequest::new(workspace.clone()))
        .await
        .expect("ACP session");
    let session_id = created.session_id.clone();

    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        acp::Agent::prompt(
            &agent,
            acp::PromptRequest::new(
                session_id.clone(),
                vec![acp::ContentBlock::Text(acp::TextContent::new(
                    "Reply with exactly: ok".to_string(),
                ))],
            ),
        ),
    )
    .await
    .expect("prompt timed out");
    let total = started.elapsed();
    println!("PROBE total prompt() latency: {total:?}");
    println!("PROBE result stop reason ok: {:?}", result.is_ok());

    // Second turn on the SAME session: measures steady-state TTFT without
    // first-turn session construction.
    let started = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        acp::Agent::prompt(
            &agent,
            acp::PromptRequest::new(
                session_id.clone(),
                vec![acp::ContentBlock::Text(acp::TextContent::new(
                    "Reply with exactly: done".to_string(),
                ))],
            ),
        ),
    )
    .await
    .expect("second prompt timed out");
    println!("PROBE second turn prompt() latency: {:?}", started.elapsed());
    let _ = result;
}
