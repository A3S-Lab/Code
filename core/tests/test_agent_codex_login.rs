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
