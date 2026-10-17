//! `SessionOptions::max_execution_time_ms` bounds one `session.stream` run
//! end to end, however many tool turns and continuation segments it takes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use a3s_code_core::permissions::PermissionPolicy;
use a3s_code_core::{Agent, AgentEvent, PlanningMode, SessionOptions};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const BUDGET_MS: u64 = 400;
const MODEL_LATENCY: Duration = Duration::from_millis(40);

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("fixture/endless".into()),
        providers: vec![ProviderConfig {
            name: "fixture".into(),
            api_key: Some("offline".into()),
            base_url: None,
            headers: HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "endless".into(),
                name: "Endless".into(),
                family: "fixture".into(),
                api_key: None,
                base_url: None,
                headers: HashMap::new(),
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
        ..Default::default()
    }
}

/// Never finishes: with `hang`, the first model call never returns; otherwise
/// every call writes a new file, so no read-only or
/// duplicate-call ceiling applies and only the deadline can stop it.
#[derive(Default)]
struct EndlessToolClient {
    calls: AtomicUsize,
    hang: bool,
}

fn tool_response(call: usize) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::ToolUse {
                id: format!("write-{call}"),
                name: "write".into(),
                input: serde_json::json!({
                    "file_path": format!("f{call}.txt"),
                    "content": format!("{call}\n"),
                }),
            }],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: Default::default(),
        },
        usage: TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("tool_use".into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

#[async_trait]
impl LlmClient for EndlessToolClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.hang {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(MODEL_LATENCY).await;
        Ok(tool_response(call))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(2);
        let latency = if self.hang {
            Duration::MAX
        } else {
            MODEL_LATENCY
        };
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(latency) => {
                    let _ = tx.send(StreamEvent::Done(tool_response(call))).await;
                }
                _ = cancel_token.cancelled() => {}
            }
        });
        Ok(rx)
    }
}

struct Outcome {
    elapsed: Duration,
    model_calls: usize,
    errors: Vec<String>,
}

async fn run(options: SessionOptions, hang: bool) -> Outcome {
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(EndlessToolClient {
        calls: AtomicUsize::new(0),
        hang,
    });
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let mut options = options
        .with_llm_client(client.clone())
        .with_permission_policy(PermissionPolicy::new().allow("write(*)"))
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_max_tool_rounds(10_000);
    options.max_execution_time_ms = Some(BUDGET_MS);
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .unwrap();

    let started = Instant::now();
    let (mut events, worker) = session.stream("write the files", None).await.unwrap();
    let mut errors = Vec::new();
    let drained = tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = events.recv().await {
            if let AgentEvent::Error { message } = event {
                errors.push(message);
            }
        }
    })
    .await;
    let elapsed = started.elapsed();
    let model_calls = client.calls.load(Ordering::SeqCst);
    session.close().await;
    worker.abort();
    assert!(
        drained.is_ok(),
        "the run outlived its {BUDGET_MS} ms budget by 20 s ({model_calls} model calls)"
    );
    Outcome {
        elapsed,
        model_calls,
        errors,
    }
}

fn assert_bounded(outcome: &Outcome) {
    assert!(
        outcome.elapsed < Duration::from_millis(BUDGET_MS * 5),
        "a {BUDGET_MS} ms budget ran {:?} ({} model calls)",
        outcome.elapsed,
        outcome.model_calls
    );
    assert!(
        outcome
            .errors
            .iter()
            .any(|message| message.contains("Execution timeout")),
        "no execution-timeout error: {:?}",
        outcome.errors
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_budget_stops_an_endless_tool_loop() {
    let outcome = run(SessionOptions::new().with_continuation(false), false).await;
    assert_bounded(&outcome);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuation_segments_share_one_budget() {
    let outcome = run(
        SessionOptions::new()
            .with_continuation(true)
            .with_max_continuation_turns(32),
        false,
    )
    .await;
    assert_bounded(&outcome);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_budget_cuts_a_model_call_that_never_returns() {
    let outcome = run(SessionOptions::new().with_continuation(false), true).await;
    assert_bounded(&outcome);
    assert_eq!(outcome.model_calls, 1);
}
