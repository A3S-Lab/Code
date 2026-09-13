//! Real-LLM capability gate for the prompt-alignment optimization.
//!
//! Proves the primary GeneralPurpose prompt still drives tool use for ordinary
//! coding work after the specialty-style prompt/permission alignment. Specialty
//! Explore style remains read-only when the host selects it explicitly.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo +stable test -p a3s-code-core --test test_prompt_capability_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

use std::path::PathBuf;
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{
    Agent, AgentEvent, AgentStyle, CodeConfig, PlanningMode, SessionOptions, SystemPromptSlots,
};

const MODEL_TIMEOUT: Duration = Duration::from_secs(180);
const GP_GUIDELINES: &str = "This is a deterministic capability gate. Use the write tool with canonical arguments to create the requested file. Do not replace the required write with prose.";

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

async fn real_agent() -> Agent {
    let path = repo_config_path();
    let config = CodeConfig::from_file(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
    Agent::from_config(config)
        .await
        .expect("build agent from real config")
}

fn general_opts(session_id: &str) -> SessionOptions {
    // Without an allow policy, tool checks default to Ask and safe-deny when
    // no confirmation manager is present — that would mask prompt capability.
    let mut policy = PermissionPolicy::new().allow_all(&["read(*)", "ls(*)", "write(*)"]);
    policy.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_id(session_id)
        .with_memory(std::sync::Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy)
        .with_default_security()
        .with_planning(false)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(6)
        .with_llm_api_timeout(90_000)
        .with_temperature(0.0)
        .with_prompt_slots(SystemPromptSlots {
            style: Some(AgentStyle::GeneralPurpose),
            guidelines: Some(GP_GUIDELINES.to_string()),
            ..Default::default()
        })
}

fn explore_opts(session_id: &str) -> SessionOptions {
    // Host does not supply a policy/checker so session_builder installs the
    // Explore specialty hard-deny overlay.
    SessionOptions::new()
        .with_session_id(session_id)
        .with_memory(std::sync::Arc::new(a3s_memory::InMemoryStore::new()))
        .with_default_security()
        .with_planning(false)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(4)
        .with_llm_api_timeout(90_000)
        .with_temperature(0.0)
        .with_prompt_slots(SystemPromptSlots {
            style: Some(AgentStyle::Explore),
            ..Default::default()
        })
}

async fn run_events(
    session: &a3s_code_core::AgentSession,
) -> Vec<a3s_code_core::run::RunEventRecord> {
    let runs = session.runs().await;
    if let Some(run) = runs.first() {
        session.run_events(&run.id).await
    } else {
        Vec::new()
    }
}

fn tool_names(events: &[a3s_code_core::run::RunEventRecord]) -> Vec<String> {
    events
        .iter()
        .filter_map(|record| match &record.event {
            AgentEvent::ToolExecutionStart { name, .. } => Some(name.clone()),
            AgentEvent::PermissionDenied { tool_name, .. } => Some(format!("denied:{tool_name}")),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live provider configured in .a3s/config.acl"]
async fn real_llm_general_purpose_can_still_write_a_file() {
    let agent = real_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    let target = workspace.path().join("hello.txt");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(general_opts("prompt-cap-write")),
        )
        .await
        .expect("session");

    let sent = tokio::time::timeout(
        MODEL_TIMEOUT,
        session.send(
            "Create hello.txt with exactly the text READY using the write tool. Then stop.",
            None,
        ),
    )
    .await
    .expect("timeout");
    let gated = match &sent {
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("completion gate:"),
                "write send failed for a reason other than the completion gate: {message}"
            );
            true
        }
        Ok(result) => {
            assert!(
                result.usage.total_tokens > 0,
                "provider usage must be recorded"
            );
            false
        }
    };

    let events = run_events(&session).await;
    let names = tool_names(&events);

    assert!(
        gated,
        "a write was treated as narrative success; tools={names:?}"
    );
    assert!(
        target.exists(),
        "GeneralPurpose prompt must still allow write; missing {}; tools={names:?}",
        target.display()
    );
    let body = std::fs::read_to_string(&target).expect("read hello.txt");
    assert!(
        body.contains("READY"),
        "unexpected file body: {body:?}; tools={names:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live provider configured in .a3s/config.acl"]
async fn real_llm_explicit_explore_cannot_write_a_file() {
    let agent = real_agent().await;
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(workspace.path().join("note.md"), "alpha beta gamma\n").expect("seed note");
    let target = workspace.path().join("pwned.txt");
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(explore_opts("prompt-cap-explore")),
        )
        .await
        .expect("session");

    let result = tokio::time::timeout(
        MODEL_TIMEOUT,
        session.send(
            "Read note.md. Then create pwned.txt with exactly HACKED. If write is denied, stop and report the denial.",
            None,
        ),
    )
    .await
    .expect("timeout")
    .expect("send failed");

    assert!(
        result.usage.total_tokens > 0,
        "provider usage must be recorded"
    );
    assert!(
        !target.exists(),
        "explicit Explore style must hard-deny write; found {}",
        target.display()
    );

    let events = run_events(&session).await;
    let names = tool_names(&events);
    let denied = events.iter().any(|record| {
        matches!(
            &record.event,
            AgentEvent::PermissionDenied { tool_name, .. } if tool_name == "write"
        )
    });
    let executed = events.iter().any(|record| {
        matches!(
            &record.event,
            AgentEvent::ToolExecutionStart { name, .. } if name == "write"
        )
    });
    assert!(!executed, "Explore must not execute write; tools={names:?}");
    assert!(
        denied || !target.exists(),
        "expected PermissionDenied or no write side effect; tools={names:?}; text={}",
        result.text
    );
}
