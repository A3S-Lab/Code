//! Real AgentLoop validation through the local Codex login.
//!
//! Run from `crates/code`:
//!
//! ```bash
//! cargo test -p a3s-code-core --test test_agent_codex_login \
//!   -- --ignored --nocapture --test-threads=1
//! ```

mod support;

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::hitl::AutoApproveConfirmation;
use a3s_code_core::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};
use a3s_code_core::permissions::PermissionPolicy;
use a3s_code_core::store::{MemorySessionStore, SessionStore};
use a3s_code_core::{
    Agent, CodeConfig, ContentBlock, LlmClient, Message, PlanningMode, SessionOptions, TokenUsage,
};
use support::codex_login_client::{default_codex_model, CodexLoginClient};

const CALL_TIMEOUT: Duration = Duration::from_secs(180);

fn test_config() -> CodeConfig {
    CodeConfig::from_acl(
        r#"
        default_model = "openai/codex-login"
        providers "openai" {
          api_key = "test-only-overridden-client"
          models "codex-login" { name = "Codex Login" }
        }
        "#,
    )
    .expect("valid test config")
}

fn codex_client(session_id: &str) -> Arc<dyn LlmClient> {
    let model = default_codex_model();
    eprintln!("[codex-agent] model = {model}");
    Arc::new(
        CodexLoginClient::from_local_login(&model, session_id).expect("local Codex login client"),
    )
}

async fn agent() -> Agent {
    Agent::from_config(test_config())
        .await
        .expect("agent from test config")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires local Codex login and network access"]
async fn codex_login_agent_reads_workspace_and_converges() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        workspace.path().join("evidence.txt"),
        "The verification token is A3S_CODEX_AGENT_OK.\n",
    )
    .expect("write fixture");
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let options = SessionOptions::new()
        .with_session_id("codex-agent-tool-convergence")
        .with_llm_client(codex_client("codex-agent-tool-convergence"))
        .with_session_store(store.clone())
        .with_permission_policy(PermissionPolicy::new().allow("read(*)"))
        .with_confirmation_manager(Arc::new(AutoApproveConfirmation))
        .with_planning_mode(PlanningMode::Disabled)
        .with_continuation(false)
        .with_max_tool_rounds(6);
    let session = agent()
        .await
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("session");

    let result = tokio::time::timeout(
        CALL_TIMEOUT,
        session.send(
            "Use the read tool to read evidence.txt. Then report its verification token exactly.",
            None,
        ),
    )
    .await
    .expect("Codex AgentLoop call exceeded 180 seconds")
    .expect("Codex AgentLoop call");

    assert!(
        result.text.contains("A3S_CODEX_AGENT_OK"),
        "final answer must contain fixture evidence: {}",
        result.text
    );
    assert!(result.tool_calls_count >= 1, "Codex must use the read tool");
    assert!(result.usage.total_tokens > 0, "real usage must be recorded");
    let runs = session.runs().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, a3s_code_core::run::RunStatus::Completed);
    assert!(store
        .load_loop_checkpoint(&runs[0].id)
        .await
        .expect("load checkpoint")
        .is_none());
}

/// Live proof that the model can use the resumable write contract through the
/// complete AgentLoop, then consume the result through nested batch calls and
/// a bounded command. Deterministic edge cases remain covered by the tool unit
/// tests; this test covers the model-facing schemas and tool-result round trips.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires local Codex login and network access"]
async fn codex_login_agent_uses_resumable_write_batch_and_bash() {
    let workspace = tempfile::tempdir().expect("workspace");
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let options = SessionOptions::new()
        .with_session_id("codex-agent-resumable-tools")
        .with_llm_client(codex_client("codex-agent-resumable-tools"))
        .with_session_store(store)
        .with_permission_policy(
            PermissionPolicy::new()
                .allow("write(*)")
                .allow("read(*)")
                .allow("batch(*)")
                .allow("bash(*)"),
        )
        .with_confirmation_manager(Arc::new(AutoApproveConfirmation))
        .with_planning_mode(PlanningMode::Disabled)
        .with_continuation(false)
        .with_max_tool_rounds(10);
    let session = agent()
        .await
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("session");

    let result = tokio::time::timeout(
        CALL_TIMEOUT,
        session.send(
            r#"Exercise the tool contracts exactly in this order; do not skip or replace a tool:
1. Call write with file_path="segments.txt", content="alpha\n", and mode="overwrite".
2. Call write with file_path="segments.txt", content="beta\n", mode="append", and expected_offset=6.
3. Call batch once with two read invocations for segments.txt (one from offset 0 and one from offset 1).
4. Call bash with command="wc -c < segments.txt" and timeout=2000.
After every tool succeeds, reply with the exact token A3S_CODEX_RESUMABLE_TOOLS_OK."#,
            None,
        ),
    )
    .await
    .expect("Codex resumable-tools call exceeded 180 seconds")
    .expect("Codex resumable-tools call");

    let history = session.history();
    eprintln!("[codex-agent:resumable-tools] result={}", result.text);
    for block in history.iter().flat_map(|message| message.content.iter()) {
        match block {
            ContentBlock::ToolUse { name, input, .. } => {
                eprintln!("[codex-agent:resumable-tools] call {name} {input}");
            }
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                eprintln!(
                    "[codex-agent:resumable-tools] result error={is_error:?} {}",
                    content.as_text()
                );
            }
            _ => {}
        }
    }
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("segments.txt"))
            .expect("segments.txt should exist"),
        "alpha\nbeta\n"
    );
    assert!(
        result.text.contains("A3S_CODEX_RESUMABLE_TOOLS_OK"),
        "final answer must contain the convergence token: {}",
        result.text
    );

    let tool_names = history
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        tool_names.iter().filter(|name| **name == "write").count(),
        2,
        "the live model must exercise overwrite and append: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"batch"),
        "batch was not called: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"bash"),
        "bash was not called: {tool_names:?}"
    );
    assert!(result.usage.total_tokens > 0, "real usage must be recorded");
}

/// A timed-out tool invocation must settle without poisoning the live model
/// session: the next tool call still runs and the agent reaches a final answer.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires local Codex login and network access"]
async fn codex_login_agent_recovers_after_bash_timeout() {
    let workspace = tempfile::tempdir().expect("workspace");
    let options = SessionOptions::new()
        .with_session_id("codex-agent-timeout-recovery")
        .with_llm_client(codex_client("codex-agent-timeout-recovery"))
        .with_permission_policy(PermissionPolicy::new().allow("bash(*)").allow("write(*)"))
        .with_confirmation_manager(Arc::new(AutoApproveConfirmation))
        .with_planning_mode(PlanningMode::Disabled)
        .with_continuation(false)
        .with_max_tool_rounds(8);
    let session = agent()
        .await
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("session");

    let result = tokio::time::timeout(
        CALL_TIMEOUT,
        session.send(
            r#"Perform both steps even though the first one is expected to fail:
1. Call bash with command="sleep 30" and timeout=1000. Observe the timeout; do not retry it.
2. Call write with file_path="recovered.txt" and content="recovered\n".
Then reply with the exact token A3S_CODEX_TIMEOUT_RECOVERY_OK."#,
            None,
        ),
    )
    .await
    .expect("Codex timeout-recovery call exceeded 180 seconds")
    .expect("Codex timeout-recovery call");

    assert_eq!(
        std::fs::read_to_string(workspace.path().join("recovered.txt"))
            .expect("the post-timeout write must run"),
        "recovered\n"
    );
    assert!(
        result.text.contains("A3S_CODEX_TIMEOUT_RECOVERY_OK"),
        "the session must converge after the timeout: {}",
        result.text
    );
    assert!(
        result.tool_calls_count >= 2,
        "both live tool calls must run"
    );
    assert!(result.usage.total_tokens > 0, "real usage must be recorded");
}

/// Account-backed transports must bind each delegated child to a distinct
/// provider session. Otherwise concurrent children contend for one live
/// operation and all expire at the outer parallel-task deadline.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires local Codex login and network access"]
async fn codex_login_parallel_children_use_independent_provider_sessions() {
    let workspace = tempfile::tempdir().expect("workspace");
    let options = SessionOptions::new()
        .with_session_id("codex-agent-parallel-sessions")
        .with_llm_client(codex_client("codex-agent-parallel-sessions"))
        .with_permission_policy(PermissionPolicy::new().allow("parallel_task(*)"))
        .with_confirmation_manager(Arc::new(AutoApproveConfirmation))
        .with_planning_mode(PlanningMode::Disabled)
        .with_manual_delegation_enabled(true)
        .with_max_parallel_tasks(3);
    let session = agent()
        .await
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("session");

    let result = tokio::time::timeout(
        CALL_TIMEOUT,
        session.tool(
            "parallel_task",
            serde_json::json!({
                "timeout_ms": 60_000,
                "tasks": [
                    {
                        "agent": "general",
                        "description": "Independent alpha",
                        "prompt": "Reply with exactly ALPHA.",
                        "max_steps": 1
                    },
                    {
                        "agent": "general",
                        "description": "Independent bravo",
                        "prompt": "Reply with exactly BRAVO.",
                        "max_steps": 1
                    },
                    {
                        "agent": "general",
                        "description": "Independent charlie",
                        "prompt": "Reply with exactly CHARLIE.",
                        "max_steps": 1
                    }
                ]
            }),
        ),
    )
    .await
    .expect("Codex parallel children exceeded 180 seconds")
    .expect("parallel_task call");

    assert_eq!(
        result.exit_code, 0,
        "parallel_task failed: {}",
        result.output
    );
    for token in ["ALPHA", "BRAVO", "CHARLIE"] {
        assert!(
            result.output.contains(token),
            "parallel output omitted {token}: {}",
            result.output
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires local Codex login and network access"]
async fn codex_login_agent_resume_preserves_cumulative_accounting() {
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let checkpoint_run_id = "codex-agent-seeded-run";
    store
        .save_loop_checkpoint(
            checkpoint_run_id,
            &LoopCheckpoint {
                schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
                run_id: checkpoint_run_id.to_string(),
                session_id: "codex-agent-resume".to_string(),
                turn: 2,
                messages: vec![
                    Message::user("Finish with the exact token A3S_CODEX_RESUME_OK."),
                    Message {
                        role: "assistant".to_string(),
                        content: vec![ContentBlock::Text {
                            text: "I will now finish the interrupted task.".to_string(),
                        }],
                        reasoning_content: None,
                    },
                ],
                total_usage: TokenUsage {
                    prompt_tokens: 400,
                    completion_tokens: 100,
                    total_tokens: 500,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                tool_calls_count: 2,
                verification_reports: Vec::new(),
                convergence: Default::default(),
                checkpoint_ms: 1_700_000_000_000,
            },
        )
        .await
        .expect("seed checkpoint");
    let options = SessionOptions::new()
        .with_session_id("codex-agent-resume")
        .with_llm_client(codex_client("codex-agent-resume"))
        .with_session_store(store)
        .with_planning_mode(PlanningMode::Disabled)
        .with_continuation(false)
        .with_max_tool_rounds(6);
    let session = agent()
        .await
        .session_async("/tmp/a3s-codex-agent-resume", Some(options))
        .await
        .expect("session");

    let result = tokio::time::timeout(CALL_TIMEOUT, session.resume_run(checkpoint_run_id))
        .await
        .expect("Codex resume exceeded 180 seconds")
        .expect("Codex resume");

    assert!(
        result.text.contains("A3S_CODEX_RESUME_OK"),
        "resumed answer must finish the seeded task: {}",
        result.text
    );
    assert!(result.usage.total_tokens > 500);
    assert!(result.tool_calls_count >= 2);
}
