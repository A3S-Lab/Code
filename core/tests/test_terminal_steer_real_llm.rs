#![cfg(test)]

//! Provider-only terminal Steer qualification through actual Code sessions.
//! No native tool execution is admitted. Run explicitly with --ignored.

mod support;

use a3s_code_core::budget::{BudgetDecision, BudgetGuard};
use a3s_code_core::fact_control::{
    read_workspace_facts, thread_for_session, TURN_COMPLETE_FACT_KIND,
};
use a3s_code_core::llm::TokenUsage;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::{
    Agent, AgentEvent, PlanningMode, RunControlReceiptState, RunStatus, SessionOptions,
    SteerRequest,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

const LIMIT: Duration = Duration::from_secs(300);
const SESSION: &str = "terminal-steer-live";
const TOKEN: &str = "CODE_TERMINAL_STEER_7319";
const STEER: &str = " \tFor the next reply, the host token is CODE_TERMINAL_STEER_7319. When asked for the token, reply with exactly that token and nothing else.\n";

#[derive(Default)]
struct NoCommands(AtomicUsize);

#[async_trait::async_trait]
impl a3s_code_core::sandbox::BashSandbox for NoCommands {
    async fn exec_command(
        &self,
        _: &str,
        _: &str,
    ) -> anyhow::Result<a3s_code_core::sandbox::SandboxOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("provider-only qualification refuses command execution")
    }

    async fn shutdown(&self) {}
}

#[derive(Debug, Default)]
struct ReceiptCapture(Mutex<Vec<a3s_code_core::RunControlReceipt>>);

#[async_trait::async_trait]
impl a3s_code_core::hooks::HookExecutor for ReceiptCapture {
    async fn fire(&self, _: &a3s_code_core::hooks::HookEvent) -> a3s_code_core::hooks::HookResult {
        a3s_code_core::hooks::HookResult::continue_()
    }

    async fn record_run_control(
        &self,
        _: &a3s_code_core::RunControlRequest,
        receipt: &a3s_code_core::RunControlReceipt,
    ) {
        self.0.lock().unwrap().push(receipt.clone());
    }
}

struct FirstActualAccounting {
    entered: Semaphore,
    release: Semaphore,
    calls: AtomicUsize,
    usage: Mutex<Vec<TokenUsage>>,
}

#[async_trait::async_trait]
impl BudgetGuard for FirstActualAccounting {
    async fn check_before_llm(&self, _: &str, _: usize) -> BudgetDecision {
        BudgetDecision::Allow
    }
    async fn record_after_llm(&self, _: &str, usage: &TokenUsage) {
        self.usage.lock().unwrap().push(usage.clone());
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
    }
}

fn options(commands: Arc<NoCommands>, receipts: Arc<ReceiptCapture>) -> SessionOptions {
    let mut policy = PermissionPolicy::new().deny("*");
    policy.default_decision = PermissionDecision::Deny;
    SessionOptions::new()
        .with_session_id(SESSION)
        // Supplying the host seam skips the default native constructor.
        .with_sandbox_handle(commands)
        .with_hook_executor(receipts)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_permission_policy(policy)
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_llm_api_timeout(180_000)
        .with_temperature(0.0)
        .with_continuation(false)
}

async fn terminal_steer(streaming: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let configured = support::layer_c_model::load_pinned_layer_c_config();
    eprintln!(
        "terminal-steer configured_route={:?} streaming={streaming}",
        configured.default_model
    );
    // Keep only the ACL's real provider binding. Host MCP connections can
    // spawn processes during Agent bootstrap, before Session tool policy.
    let config = a3s_code_core::config::CodeConfig {
        providers: configured.providers,
        default_model: configured.default_model,
        memory: Some(a3s_code_core::memory::MemoryConfig {
            llm_extraction: false,
            prune_policy: None,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(config.mcp_servers.is_empty());
    assert!(config.workspace_retrieval.is_none());
    assert!(config.notifications.is_none());
    let agent = Agent::from_config(config).await.unwrap();
    let commands = Arc::new(NoCommands::default());
    let receipts = Arc::new(ReceiptCapture::default());
    let accounting = Arc::new(FirstActualAccounting {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
        calls: AtomicUsize::new(0),
        usage: Mutex::new(Vec::new()),
    });
    let session = Arc::new(
        agent
            .session_async(
                workspace.path().display().to_string(),
                Some(
                    options(commands.clone(), receipts.clone())
                        .with_budget_guard(accounting.clone()),
                ),
            )
            .await
            .unwrap(),
    );
    let prompt = "Do not call any tools. Reply with exactly INITIAL_CODE_ACK and nothing else.";
    let mut stream = if streaming {
        Some(session.stream_with_run(prompt, None).await.unwrap())
    } else {
        None
    };
    let caller = if streaming {
        None
    } else {
        let session = session.clone();
        Some(tokio::spawn(
            async move { session.send(prompt, None).await },
        ))
    };
    accounting.entered.acquire().await.unwrap().forget();
    // This is a real returned provider response blocked in its original host
    // accounting callback, after the last actor safe point, not a fake result.
    let run = session.current_run().await.unwrap();
    let mut request = SteerRequest::new(STEER).with_run_id(run.id());
    request.request_id = Some("real-terminal-steer-request".into());
    let accepted = session.steer(request.clone()).await.unwrap();
    assert_eq!(accepted.state, RunControlReceiptState::Accepted);
    assert_eq!(session.steer(request).await.unwrap(), accepted);
    accounting.release.add_permits(1);
    let first_text = if let Some(mut stream) = stream.take() {
        let mut answer = None;
        while let Some(event) = stream.events.recv().await {
            match event {
                AgentEvent::End { text, .. } => {
                    assert!(answer.replace(text).is_none(), "duplicate End");
                }
                AgentEvent::Error { message } => panic!("terminal failure: {message}"),
                AgentEvent::ToolExecutionStart { .. } | AgentEvent::PermissionDenied { .. } => {
                    panic!("provider-only scenario attempted a tool");
                }
                _ => {}
            }
        }
        stream.worker.await.unwrap();
        answer.unwrap()
    } else {
        let result = caller.unwrap().await.unwrap().unwrap();
        assert_eq!(result.tool_calls_count, 0);
        result.text
    };
    assert_eq!(first_text.trim(), "INITIAL_CODE_ACK");
    assert_eq!(
        accounting.calls.load(Ordering::SeqCst),
        1,
        "terminal retention must not dispatch another model"
    );
    assert_eq!(run.status().await, Some(RunStatus::Completed));
    assert!(
        session
            .cancel_and_settle(Duration::from_secs(5), Duration::from_secs(1))
            .await
    );
    let facts = read_workspace_facts(workspace.path(), &thread_for_session(SESSION)).unwrap();
    let retained: Vec<_> = facts
        .iter()
        .filter(|fact| fact.kind == "user.message" && fact.payload["text"] == STEER)
        .collect();
    assert_eq!(retained.len(), 1, "exact original input retained once");
    let digest = a3s_code_core::digest_json(
        "a3s.code.steer-input.identity.v1",
        &[SESSION, run.id(), accepted.request_id.as_str()],
    )
    .unwrap();
    assert_eq!(retained[0].key, format!("run-steer-v1:{digest}"));
    {
        let observed = receipts.0.lock().unwrap();
        assert_eq!(
            observed.len(),
            2,
            "one Accepted and one Applied observation"
        );
        assert_eq!(observed[0], accepted);
        assert_eq!(observed[1].state, RunControlReceiptState::Applied);
        assert_eq!(observed[1].request_id, accepted.request_id);
        assert_eq!(observed[1].run_id, run.id());
        assert_eq!(observed[1].session_id, SESSION);
    }
    let marker = facts
        .iter()
        .find(|fact| fact.kind == TURN_COMPLETE_FACT_KIND)
        .unwrap();
    assert!(
        retained[0].seq < marker.seq,
        "input retention must precede success marker"
    );
    assert_eq!(
        facts
            .iter()
            .filter(|fact| fact.kind == "model.turn")
            .count(),
        1
    );
    eprintln!(
        "terminal-steer retained run={} fact_key={} seq={} marker_seq={} usage={:?}",
        run.id(),
        retained[0].key,
        retained[0].seq,
        marker.seq,
        accounting.usage.lock().unwrap()
    );
    let snapshot = serde_json::to_string(&serde_json::json!({
        "stage": "terminal",
        "session_id": SESSION,
        "original_run_id": run.id(),
        "request_id": accepted.request_id,
        "receipts": &*receipts.0.lock().unwrap(),
        "facts": &facts,
    }))
    .unwrap();
    assert!(
        snapshot.len() <= 64 * 1024,
        "fact snapshot exceeds evidence bound"
    );
    eprintln!("terminal-steer facts_snapshot={snapshot}");
    session.close().await;
    let reopened = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(options(commands.clone(), receipts)),
        )
        .await
        .unwrap();
    let result = reopened.send("Do not call tools. What is the exact host token from my latest steering directive? Reply only with that token.", None).await.unwrap();
    assert_eq!(result.text.trim(), TOKEN);
    assert_eq!(result.tool_calls_count, 0);
    assert_eq!(commands.0.load(Ordering::SeqCst), 0);
    let after = read_workspace_facts(workspace.path(), &thread_for_session(SESSION)).unwrap();
    assert_eq!(
        after
            .iter()
            .filter(|fact| fact.kind == "user.message" && fact.payload["text"] == STEER)
            .count(),
        1
    );
    assert_eq!(
        after
            .iter()
            .filter(|fact| fact.kind == "model.turn")
            .count(),
        2
    );
    eprintln!(
        "terminal-steer reopened actual_response={:?} usage={:?}",
        result.text, result.usage
    );
    let snapshot = serde_json::to_string(&serde_json::json!({
        "stage": "reopened",
        "session_id": SESSION,
        "original_run_id": run.id(),
        "request_id": accepted.request_id,
        "facts": &after,
    }))
    .unwrap();
    assert!(
        snapshot.len() <= 64 * 1024,
        "fact snapshot exceeds evidence bound"
    );
    eprintln!("terminal-steer facts_snapshot={snapshot}");
    reopened.close().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials through the existing host ACL"]
async fn real_blocking_terminal_steer_survives_close_and_reopen() {
    tokio::time::timeout(LIMIT, terminal_steer(false))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials through the existing host ACL"]
async fn real_streaming_terminal_steer_survives_close_and_reopen() {
    tokio::time::timeout(LIMIT, terminal_steer(true))
        .await
        .unwrap();
}
