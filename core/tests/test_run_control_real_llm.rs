//! Live-provider qualification for public steer and interrupt controls.
//!
//! Ignored by default because it consumes provider quota. Select a model from
//! the same ACL with `A3S_TEST_MODEL=provider/model`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{
    Agent, AgentEvent, AgentSession, CodeConfig, InterruptRequest, PlanningMode,
    RunControlOperation, RunControlReceiptState, RunStatus, SessionOptions, SteerRequest,
    SystemPromptSlots,
};

const MODEL_TIMEOUT: Duration = Duration::from_secs(240);
#[cfg(not(windows))]
const STEER_COMMAND: &str =
    "printf STARTED > steer-started.txt && sleep 2 && printf FINISHED > steer-finished.txt";
#[cfg(windows)]
const STEER_COMMAND: &str = "[System.IO.File]::WriteAllText('steer-started.txt','STARTED'); Start-Sleep -Seconds 2; [System.IO.File]::WriteAllText('steer-finished.txt','FINISHED')";
#[cfg(not(windows))]
const INTERRUPT_COMMAND: &str =
    "printf STARTED > interrupt-started.txt && sleep 10 && printf LEAK > interrupt-leak.txt";
#[cfg(windows)]
const INTERRUPT_COMMAND: &str = "[System.IO.File]::WriteAllText('interrupt-started.txt','STARTED'); Start-Sleep -Seconds 10; [System.IO.File]::WriteAllText('interrupt-leak.txt','LEAK')";

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

async fn configured_agent_and_model() -> (Agent, String) {
    let path = repo_config_path();
    let config = CodeConfig::from_file(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
    let model = std::env::var("A3S_TEST_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| config.default_model.clone())
        .expect("real config must declare default_model");
    let (provider, model_id) = model
        .split_once('/')
        .expect("selected model must use provider/model syntax");
    assert!(
        config.llm_config(provider, model_id).is_some(),
        "selected model {model} is not declared in {}",
        path.display()
    );
    eprintln!("[run-control-real] model={model}");
    (
        Agent::from_config(config)
            .await
            .expect("build real-provider agent"),
        model,
    )
}

fn options(model: &str, session_id: &str, command: &str) -> SessionOptions {
    let mut policy = PermissionPolicy::new().allow(&format!("bash({command})"));
    policy.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_id(session_id)
        .with_model(model)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy)
        .with_default_security()
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(4)
        .with_llm_api_timeout(120_000)
        .with_temperature(0.0)
        .with_continuation(false)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(
            "This is a deterministic run-control test. Invoke the exact requested command once, do not substitute another tool or command, and obey later host steering as the newest user instruction.",
        ))
}

async fn active_snapshot(session: &AgentSession) -> a3s_code_core::RunControlSnapshot {
    tokio::time::timeout(MODEL_TIMEOUT, async {
        loop {
            if let Some(snapshot) = session.run_control_snapshot().await {
                if snapshot.turn_id.is_some() {
                    return snapshot;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("active run-control snapshot timed out")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_model_applies_steer_after_an_in_flight_tool() {
    let workspace = tempfile::tempdir().expect("steer workspace");
    let (agent, model) = configured_agent_and_model().await;
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(options(&model, "real-steer", STEER_COMMAND)),
        )
        .await
        .expect("create steer session");
    let prompt = format!(
        "Invoke bash exactly once with this command: `{STEER_COMMAND}`. After it finishes, reply with exactly ORIGINAL_RESULT."
    );
    let (mut events, worker) = session
        .stream(&prompt, None)
        .await
        .expect("start steer stream");

    let snapshot = tokio::time::timeout(MODEL_TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(AgentEvent::ToolExecutionStart { name, args, .. }) if name == "bash" => {
                    assert_eq!(args["command"], STEER_COMMAND);
                    return active_snapshot(&session).await;
                }
                Some(AgentEvent::PermissionDenied {
                    tool_name, reason, ..
                }) => {
                    panic!("model selected an unauthorized tool {tool_name}: {reason}")
                }
                Some(AgentEvent::Error { message }) => panic!("stream failed: {message}"),
                Some(AgentEvent::End { .. }) | None => panic!("run ended before bash started"),
                _ => {}
            }
        }
    })
    .await
    .expect("model did not start steer command");
    let receipt = session
        .steer(
            SteerRequest::new(
                "Replace the final answer with exactly STEER_REAL_OK and no other text.",
            )
            .with_run_id(snapshot.run_id.clone())
            .with_expected_turn(snapshot.turn_id.clone().unwrap(), snapshot.turn_revision),
        )
        .await
        .expect("accept steer");
    assert_eq!(receipt.state, RunControlReceiptState::Accepted);

    let mut saw_applied = false;
    let mut final_text = None;
    tokio::time::timeout(MODEL_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            match event {
                AgentEvent::RunControlApplied {
                    operation: RunControlOperation::Steer,
                    ..
                } => saw_applied = true,
                AgentEvent::End { text, .. } => {
                    final_text = Some(text);
                    return;
                }
                AgentEvent::Error { message } => panic!("steered stream failed: {message}"),
                _ => {}
            }
        }
    })
    .await
    .expect("steered run did not settle");
    worker.await.expect("steer worker join");

    assert!(saw_applied, "stream omitted RunControlApplied");
    assert_eq!(final_text.as_deref().map(str::trim), Some("STEER_REAL_OK"));
    assert!(workspace.path().join("steer-finished.txt").is_file());
    assert_eq!(
        session.run_snapshot(&snapshot.run_id).await.unwrap().status,
        RunStatus::Completed
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn real_model_interrupt_cancels_an_in_flight_tool_without_late_effects() {
    let workspace = tempfile::tempdir().expect("interrupt workspace");
    let (agent, model) = configured_agent_and_model().await;
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(options(&model, "real-interrupt", INTERRUPT_COMMAND)),
        )
        .await
        .expect("create interrupt session");
    let prompt = format!(
        "Invoke bash exactly once with this command: `{INTERRUPT_COMMAND}`. Reply only after it finishes."
    );
    let (mut events, worker) = session
        .stream(&prompt, None)
        .await
        .expect("start interrupt stream");

    let snapshot = tokio::time::timeout(MODEL_TIMEOUT, async {
        loop {
            match events.recv().await {
                Some(AgentEvent::ToolExecutionStart { name, args, .. }) if name == "bash" => {
                    assert_eq!(args["command"], INTERRUPT_COMMAND);
                    return active_snapshot(&session).await;
                }
                Some(AgentEvent::PermissionDenied {
                    tool_name, reason, ..
                }) => {
                    panic!("model selected an unauthorized tool {tool_name}: {reason}")
                }
                Some(AgentEvent::Error { message }) => panic!("stream failed: {message}"),
                Some(AgentEvent::End { .. }) | None => panic!("run ended before bash started"),
                _ => {}
            }
        }
    })
    .await
    .expect("model did not start interrupt command");
    tokio::time::timeout(Duration::from_secs(5), async {
        while !workspace.path().join("interrupt-started.txt").is_file() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("interrupt command did not publish its start marker");
    let receipt = session
        .interrupt(
            InterruptRequest::new()
                .with_reason("real conformance interrupt")
                .with_run_id(snapshot.run_id.clone())
                .with_expected_turn(snapshot.turn_id.clone().unwrap(), snapshot.turn_revision),
        )
        .await
        .expect("accept interrupt");
    assert_eq!(receipt.state, RunControlReceiptState::Accepted);

    let mut saw_applied = false;
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = events.recv().await {
            if matches!(
                event,
                AgentEvent::RunControlApplied {
                    operation: RunControlOperation::Interrupt,
                    ..
                }
            ) {
                saw_applied = true;
            }
        }
    })
    .await
    .expect("interrupted run did not settle");
    worker.await.expect("interrupt worker join");

    assert!(saw_applied, "stream omitted interrupt application evidence");
    assert_eq!(
        session.run_snapshot(&snapshot.run_id).await.unwrap().status,
        RunStatus::Cancelled
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !workspace.path().join("interrupt-leak.txt").exists(),
        "cancelled command produced a late side effect"
    );
}
