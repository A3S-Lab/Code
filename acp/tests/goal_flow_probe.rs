//! Goal command flow probe: drives set/pause/resume/status directly and
//! prints every streamed text event per turn.
//!
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-acp --test goal_flow_probe -- --ignored --nocapture --test-threads=1

use std::path::PathBuf;
use std::time::Duration;

use a3s_code_acp::{load_launch_config, A3sCodeAgent, LaunchConfig};
use agent_client_protocol as acp;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the model configured in .a3s/config.acl"]
async fn goal_commands_flow_end_to_end() {
    let config_path = std::env::var_os("A3S_CONFIG_FILE").map(PathBuf::from);
    let workspace = std::env::temp_dir().join(format!("a3s-goal-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&workspace);
    std::fs::create_dir_all(&workspace).expect("workspace");

    let mut launch = load_launch_config(workspace.clone(), config_path).expect("launch config");
    if let Ok(raw) = std::env::var("A3S_TEST_MODEL") {
        let model = raw.trim().to_string();
        launch.model_id = model.clone();
        launch.config.default_model = Some(model);
    }
    let agent = A3sCodeAgent::new(launch);
    let created = acp::Agent::new_session(&agent, acp::NewSessionRequest::new(workspace.clone()))
        .await
        .expect("session");

    let turns: &[(&str, &str)] = &[
        ("set", "/goal Reply with exactly GOALSET-9137 and nothing else. Do not use any tools."),
        ("pause", "/goal pause"),
        ("resume", "/goal resume"),
        ("status", "/goal status"),
    ];

    for (label, prompt) in turns {
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(90),
            acp::Agent::prompt(
                &agent,
                acp::PromptRequest::new(
                    created.session_id.clone(),
                    vec![acp::ContentBlock::Text(acp::TextContent::new((*prompt).to_string()))],
                ),
            ),
        )
        .await
        .expect("turn timed out");
        let elapsed = started.elapsed();
        match &result {
            Ok(response) => {
                println!("GOALPROBE {label}: {:?} in {elapsed:?}", response.stop_reason);
            }
            Err(error) => {
                println!("GOALPROBE {label}: ERROR {error:?} in {elapsed:?}");
            }
        }
    }

    // Pager queue timing: fire resume, then IMMEDIATELY queue status while
    // the resume inference is mid-flight (what the pager's Enter:queue does).
    println!("GOALPROBE queue-race: firing resume then immediate status");
    let resume_fut = acp::Agent::prompt(
        &agent,
        acp::PromptRequest::new(
            created.session_id.clone(),
            vec![acp::ContentBlock::Text(acp::TextContent::new(
                "/goal resume".to_string(),
            ))],
        ),
    );
    let status_fut = acp::Agent::prompt(
        &agent,
        acp::PromptRequest::new(
            created.session_id.clone(),
            vec![acp::ContentBlock::Text(acp::TextContent::new(
                "/goal status".to_string(),
            ))],
        ),
    );
    let started = std::time::Instant::now();
    let (resume_result, status_result) = tokio::join!(resume_fut, status_fut);
    println!(
        "GOALPROBE queue-race done in {:?}: resume={:?} status={:?}",
        started.elapsed(),
        resume_result.map(|r| r.stop_reason),
        status_result.map(|r| r.stop_reason)
    );
}
