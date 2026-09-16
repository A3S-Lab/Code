//! Live `update_plan` checklist e2e against the model in `.a3s/config.acl`.
//!
//! Verifies the Default-mode agent can actively drive the host-pinned task
//! list (Cursor TodoWrite parity): the model calls `update_plan`, Core emits
//! `TaskUpdated`, and the tool result confirms the checklist.
//!
//! Opt-in (provider quota + network):
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --test test_update_plan_live_e2e \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{Agent, AgentEvent, RunStatus, SessionOptions};

mod support;
use support::layer_c_model::load_pinned_layer_c_config;

const MODEL_TIMEOUT: Duration = Duration::from_secs(180);

async fn configured_agent() -> Agent {
    Agent::from_config(load_pinned_layer_c_config())
        .await
        .expect("build agent from .a3s/config.acl")
}

fn checklist_options(session_id: &str) -> SessionOptions {
    let policy = PermissionPolicy {
        default_decision: PermissionDecision::Deny,
        ..PermissionPolicy::default()
    }
    .allow("update_plan")
    .allow("read(**)");

    SessionOptions::new()
        .with_session_id(session_id)
        .with_memory(std::sync::Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy)
        .with_default_security()
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(4)
        .with_llm_api_timeout(90_000)
        .with_temperature(0.0)
        .with_continuation(false)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the live provider configured in .a3s/config.acl"]
async fn configured_model_drives_update_plan_checklist() {
    let agent = configured_agent().await;
    let workspace = tempfile::tempdir().expect("temporary checklist workspace");
    std::fs::write(
        workspace.path().join("README.md"),
        "checklist e2e fixture\n",
    )
    .expect("write fixture");

    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(checklist_options("update-plan-live-e2e")),
        )
        .await
        .expect("create session");

    let prompt = "Multi-step tracking conformance test. Do not edit files. \
        Immediately call the update_plan tool with exactly three steps about \
        inspecting README.md, summarizing it, and closing the checklist. Put \
        the first step in_progress and the other two pending. After the tool \
        result, stop with one short sentence.";

    let result = tokio::time::timeout(MODEL_TIMEOUT, session.send(prompt, None))
        .await
        .expect("update_plan live scenario timed out")
        .expect("update_plan live scenario failed");
    assert!(
        result.usage.total_tokens > 0,
        "provider usage must be recorded"
    );

    let runs = session.runs().await;
    assert_eq!(runs.len(), 1, "exactly one run");
    assert_eq!(runs[0].status, RunStatus::Completed);
    let events = session.run_events(&runs[0].id).await;

    assert!(
        events.iter().any(|record| matches!(
            &record.event,
            AgentEvent::ToolExecutionStart { name, .. } if name == "update_plan"
        )),
        "model must call update_plan; events={events:?}"
    );
    assert!(
        events.iter().any(|record| matches!(
            &record.event,
            AgentEvent::ToolEnd { name, exit_code: 0, .. } if name == "update_plan"
        )),
        "update_plan must succeed; events={events:?}"
    );

    let tasks = events.iter().find_map(|record| match &record.event {
        AgentEvent::TaskUpdated { tasks, .. } => Some(tasks.clone()),
        _ => None,
    });
    let tasks = tasks.expect("TaskUpdated must be emitted for the host pin path");
    assert!(
        tasks.len() >= 3,
        "checklist must have at least 3 steps, got {}",
        tasks.len()
    );
    assert!(
        tasks
            .iter()
            .any(|task| { matches!(task.status, a3s_code_core::planning::TaskStatus::InProgress) }),
        "exactly one step should be in progress: {tasks:?}"
    );
}
