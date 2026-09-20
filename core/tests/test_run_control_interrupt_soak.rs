//! S-RC-01: interrupt storms must end each run once, and a repeated steer
//! token must not be applied twice. The tool effect is counted only after
//! the invocation observes that it was not cancelled.

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, TokenUsage, ToolDefinition,
};
use a3s_code_core::memory::MemoryConfig;
use a3s_code_core::permissions::{PermissionDecision, PermissionPolicy};
use a3s_code_core::tools::{Tool, ToolContext, ToolOutput};
use a3s_code_core::{
    Agent, InterruptRequest, PlanningMode, RunStatus, SessionOptions, SteerRequest,
};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

struct ToolCallClient {
    calls: AtomicUsize,
}

fn tool_response(id: usize) -> LlmResponse {
    let mut message = Message::assistant("");
    message.content = vec![ContentBlock::ToolUse {
        id: format!("call-{id}"),
        name: "gate_marker".to_string(),
        input: serde_json::json!({}),
    }];
    LlmResponse {
        message,
        usage: TokenUsage {
            prompt_tokens: 4,
            completion_tokens: 1,
            total_tokens: 5,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("tool_use".to_string()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

#[async_trait]
impl LlmClient for ToolCallClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(tool_response(self.calls.fetch_add(1, Ordering::SeqCst)))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<a3s_code_core::llm::StreamEvent>> {
        let response = tool_response(self.calls.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            let _ = tx
                .send(a3s_code_core::llm::StreamEvent::Done(response))
                .await;
        });
        Ok(rx)
    }
}

struct GateTool {
    entered: Arc<AtomicUsize>,
    effects: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for GateTool {
    fn name(&self) -> &str {
        "gate_marker"
    }

    fn description(&self) -> &str {
        "Waits for cancellation before any side effect"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": false})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        self.entered.fetch_add(1, Ordering::SeqCst);
        ctx.cancellation_token().cancelled().await;
        if ctx.is_cancelled() {
            return Ok(ToolOutput::error("cancelled before side effect"));
        }
        self.effects.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::success("effect"))
    }
}

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        memory: Some(MemoryConfig {
            llm_extraction: false,
            ..MemoryConfig::default()
        }),
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("offline-key".to_string()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude".to_string(),
                family: "claude".to_string(),
                api_key: None,
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..CodeConfig::default()
    }
}

#[tokio::test]
#[ignore = "S-RC-01 soak: 50 interrupt storms must not double-apply steer or run the tool effect"]
async fn soak_interrupt_storm_settles_once_without_late_effects() {
    let entered = Arc::new(AtomicUsize::new(0));
    let effects = Arc::new(AtomicUsize::new(0));
    let workspace = tempfile::tempdir().expect("workspace");
    let agent = Agent::from_config(offline_config()).await.expect("agent");
    let mut policy = PermissionPolicy::new().allow("gate_marker");
    policy.default_decision = PermissionDecision::Allow;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("s-rc-01")
                    .with_model("anthropic/claude-sonnet-4-20250514")
                    .with_llm_client(Arc::new(ToolCallClient {
                        calls: AtomicUsize::new(0),
                    }))
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_continuation(false)
                    .without_workspace_retrieval()
                    .with_permission_policy(policy),
            ),
        )
        .await
        .expect("session");
    session
        .register_dynamic_tool(Arc::new(GateTool {
            entered: Arc::clone(&entered),
            effects: Arc::clone(&effects),
        }))
        .expect("register");

    for cycle in 0..50 {
        let (mut events, handle) = session
            .stream("invoke the gate", None)
            .await
            .unwrap_or_else(|error| panic!("cycle {cycle} stream: {error}"));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while entered.load(Ordering::SeqCst) == cycle {
            if std::time::Instant::now() > deadline {
                panic!("cycle {cycle} tool never started");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let steer = SteerRequest {
            request_id: Some(format!("steer-{cycle:04}")),
            ..SteerRequest::new(format!("STEER-{cycle:04}"))
        };
        let first_steer = session
            .steer(steer.clone())
            .await
            .unwrap_or_else(|error| panic!("cycle {cycle} steer: {error}"));
        let second_steer = session
            .steer(steer)
            .await
            .unwrap_or_else(|error| panic!("cycle {cycle} duplicate steer: {error}"));
        assert_eq!(first_steer.request_id, second_steer.request_id);
        assert_eq!(
            first_steer.sequence, second_steer.sequence,
            "duplicate steer token was admitted again"
        );

        let run = session
            .current_run()
            .await
            .unwrap_or_else(|| panic!("cycle {cycle} has no active run"));
        let interrupt = InterruptRequest {
            request_id: Some(format!("int-{cycle:04}")),
            ..InterruptRequest::new()
        };
        let first_interrupt = session
            .interrupt(interrupt.clone())
            .await
            .unwrap_or_else(|error| panic!("cycle {cycle} interrupt: {error}"));
        let effects_at_receipt = effects.load(Ordering::SeqCst);
        let second_interrupt = session
            .interrupt(interrupt)
            .await
            .unwrap_or_else(|error| panic!("cycle {cycle} duplicate interrupt: {error}"));
        assert_eq!(first_interrupt.request_id, second_interrupt.request_id);
        assert_eq!(first_interrupt.sequence, second_interrupt.sequence);

        tokio::time::timeout(Duration::from_secs(5), async {
            while events.recv().await.is_some() {}
            let _ = handle.await;
        })
        .await
        .unwrap_or_else(|_| panic!("cycle {cycle} did not settle"));

        let snapshot = run
            .snapshot()
            .await
            .unwrap_or_else(|| panic!("cycle {cycle} lost its run"));
        assert!(
            snapshot.status.is_terminal(),
            "cycle {cycle} left in {:?}",
            snapshot.status
        );
        assert_ne!(snapshot.status, RunStatus::Executing);
        let again = run.snapshot().await.expect("snapshot");
        assert_eq!(snapshot.status, again.status);
        assert_eq!(
            effects.load(Ordering::SeqCst),
            effects_at_receipt,
            "side effect after interrupt receipt"
        );
        assert!(session.current_run().await.is_none());

        let rendered = session
            .history()
            .iter()
            .map(Message::text)
            .collect::<Vec<_>>()
            .join("\n");
        let marker = format!("STEER-{cycle:04}");
        assert!(
            rendered.matches(&marker).count() <= 1,
            "steer token applied more than once"
        );
    }

    assert_eq!(entered.load(Ordering::SeqCst), 50);
    assert_eq!(
        effects.load(Ordering::SeqCst),
        0,
        "interrupted runs still performed the tool effect"
    );
    let runs = session.runs().await;
    assert_eq!(runs.len(), 50);
    assert!(runs.iter().all(|run| run.status.is_terminal()));
}
