use super::*;
use crate::agent::invocation_context::InvocationGovernance;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

struct PendingStreamingClient {
    provider_cancelled: Arc<tokio::sync::Notify>,
}

struct CompletedStreamingClient;

struct UsageRecordedGuard {
    recorded: Arc<tokio::sync::Notify>,
}

#[derive(Clone)]
struct BlockingConcurrencyClient {
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
    started: tokio::sync::mpsc::UnboundedSender<()>,
    release: Arc<tokio::sync::Semaphore>,
}

#[test]
fn typed_model_requests_bind_their_evidence_kind() {
    let messages = [Message::user("hello")];
    let tools = [];
    let completion = ModelCallRequest::completion(&messages, None, &tools);
    let completion_observation = completion.observation(ModelPresentationApplicationV1::Auxiliary);
    assert_eq!(completion_observation.kind, ModelInputKindV1::Completion);
    assert!(completion_observation.estimated_prompt_tokens > 0);

    let directive = StructuredDirective::default();
    let structured = ModelCallRequest::structured(&messages, None, &tools, &directive);
    let structured_observation = structured.observation(ModelPresentationApplicationV1::Auxiliary);
    assert_eq!(structured_observation.kind, ModelInputKindV1::Structured);
    assert!(structured_observation.directive.is_some());

    let retry = ModelCallRequest::completion(&messages, None, &tools);
    assert_eq!(
        completion.idempotency_identity("session-1").unwrap(),
        retry.idempotency_identity("session-1").unwrap()
    );
    assert_ne!(
        completion.idempotency_identity("session-1").unwrap(),
        completion.idempotency_identity("session-2").unwrap()
    );
}

#[test]
fn typed_stream_requests_bind_their_evidence_kind() {
    let messages = [Message::user("hello")];
    let tools = [];
    let cancellation = CancellationToken::new();
    let streaming = ModelStreamRequest::completion(&messages, None, &tools, cancellation.clone());
    assert_eq!(
        streaming
            .observation(ModelPresentationApplicationV1::Auxiliary)
            .kind,
        ModelInputKindV1::Streaming
    );

    let directive = StructuredDirective::default();
    let structured =
        ModelStreamRequest::structured(&messages, None, &tools, &directive, cancellation);
    let observation = structured.observation(ModelPresentationApplicationV1::Auxiliary);
    assert_eq!(observation.kind, ModelInputKindV1::StreamingStructured);
    assert!(observation.directive.is_some());

    assert_ne!(
        streaming.idempotency_identity("session-1").unwrap(),
        structured.idempotency_identity("session-1").unwrap()
    );
}

#[async_trait]
impl crate::budget::BudgetGuard for UsageRecordedGuard {
    async fn record_after_llm(&self, _session_id: &str, _usage: &TokenUsage) {
        self.recorded.notify_one();
    }
}

#[test]
fn prompt_estimator_counts_tool_results() {
    let messages = vec![Message::tool_result("tool-1", &"x".repeat(4_000), false)];

    assert!(estimate_prompt_tokens(&messages, None, &[]) >= 1_000);
}

#[test]
fn prompt_estimator_counts_tool_definitions() {
    let tools = vec![ToolDefinition {
        name: "large_tool".to_string(),
        description: "x".repeat(2_000),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "payload": { "type": "string", "description": "y".repeat(2_000) }
            }
        }),
    }];

    assert!(estimate_prompt_tokens(&[], None, &tools) >= 1_000);
}

#[derive(Clone)]
struct SessionBindingClient {
    bound_session: Option<String>,
    observed_sessions: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone)]
struct TimeoutAwareClient {
    observed: Arc<Mutex<Vec<std::time::Duration>>>,
}

#[async_trait]
impl LlmClient for TimeoutAwareClient {
    fn with_active_generation_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Option<Arc<dyn LlmClient>> {
        self.observed.lock().unwrap().push(timeout);
        Some(Arc::new(self.clone()))
    }

    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(LlmResponse {
            message: Message::assistant("ok"),
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("stop".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by this test")
    }
}

#[async_trait]
impl LlmClient for SessionBindingClient {
    fn fork_for_session(&self, session_id: &str) -> Option<Arc<dyn LlmClient>> {
        Some(Arc::new(Self {
            bound_session: Some(session_id.to_string()),
            observed_sessions: Arc::clone(&self.observed_sessions),
        }))
    }

    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.observed_sessions
            .lock()
            .unwrap()
            .push(self.bound_session.clone().unwrap_or_default());
        Ok(LlmResponse {
            message: Message::assistant("ok"),
            usage: TokenUsage {
                prompt_tokens: 11,
                completion_tokens: 5,
                total_tokens: 16,
                cache_read_tokens: Some(3),
                cache_write_tokens: None,
            },
            stop_reason: Some("stop".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by this test")
    }
}

#[async_trait]
impl LlmClient for PendingStreamingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("non-streaming is not used by this test")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (tx, rx) = mpsc::channel(1);
        let provider_cancelled = Arc::clone(&self.provider_cancelled);
        tokio::spawn(async move {
            cancel_token.cancelled().await;
            drop(tx);
            provider_cancelled.notify_one();
        });
        Ok(rx)
    }
}

#[async_trait]
impl LlmClient for CompletedStreamingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("non-streaming is not used by this test")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (tx, rx) = mpsc::channel(1);
        tx.send(StreamEvent::Done(LlmResponse {
            message: Message::assistant("streamed"),
            usage: TokenUsage {
                prompt_tokens: 17,
                completion_tokens: 3,
                total_tokens: 20,
                cache_read_tokens: None,
                cache_write_tokens: Some(4),
            },
            stop_reason: Some("stop".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }))
        .await
        .unwrap();
        Ok(rx)
    }
}

#[async_trait]
impl LlmClient for BlockingConcurrencyClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_active.fetch_max(active, Ordering::SeqCst);
        self.started.send(()).unwrap();
        self.release.acquire().await.unwrap().forget();
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(LlmResponse {
            message: Message::assistant("ok"),
            usage: TokenUsage::default(),
            stop_reason: Some("stop".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by this test")
    }
}

#[tokio::test]
async fn independent_invokers_share_scheduler_backed_provider_capacity() {
    let scheduler = Arc::new(
        crate::task_scheduler::TaskScheduler::new(crate::task_scheduler::TaskSchedulerConfig {
            max_active: 1,
            aging_interval_ms: 1_000,
        })
        .unwrap(),
    );
    let pool = ModelGenerationPool::for_endpoint(
        "test-provider",
        "test-model",
        "https://provider.test",
        ModelGenerationConcurrency::single_flight(),
    )
    .unwrap();
    let quota = crate::task_scheduler::TaskSchedulerQuota::new(
        pool.identity.clone(),
        pool.max_concurrency().get(),
    )
    .unwrap();
    let admission = |label: &'static str| {
        ModelGenerationAdmission::new(ModelGenerationConcurrency::single_flight())
            .with_scheduler_quota(
                Arc::clone(&scheduler),
                quota.clone(),
                crate::task_scheduler::TaskPriority::Foreground,
                label,
            )
            .unwrap()
    };
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let client: Arc<dyn LlmClient> = Arc::new(BlockingConcurrencyClient {
        active,
        max_active: Arc::clone(&max_active),
        started: started_tx,
        release: Arc::clone(&release),
    });
    let first_invocation = InvocationContext::new(
        Arc::<str>::from("provider-run-a"),
        Arc::<str>::from("provider-session-a"),
        CancellationToken::new(),
        None,
        InvocationGovernance::default(),
    );
    let second_invocation = InvocationContext::new(
        Arc::<str>::from("provider-run-b"),
        Arc::<str>::from("provider-session-b"),
        CancellationToken::new(),
        None,
        InvocationGovernance::default(),
    );
    let first = Arc::new(LlmInvoker::new_with_admission(
        Arc::clone(&client),
        first_invocation,
        admission("provider-a"),
    ));
    let second = Arc::new(LlmInvoker::new_with_admission(
        Arc::clone(&client),
        second_invocation,
        admission("provider-b"),
    ));

    let first_call =
        tokio::spawn(async move { first.complete(&[Message::user("first")], None, &[]).await });
    started_rx.recv().await.unwrap();
    let second_call =
        tokio::spawn(async move { second.complete(&[Message::user("second")], None, &[]).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(20), started_rx.recv())
            .await
            .is_err(),
        "the second provider call must remain queued"
    );
    release.add_permits(1);
    started_rx.recv().await.unwrap();
    release.add_permits(1);
    first_call.await.unwrap().unwrap();
    second_call.await.unwrap().unwrap();

    assert_eq!(max_active.load(Ordering::SeqCst), 1);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn streaming_provider_capacity_is_held_until_receiver_drop() {
    let scheduler = Arc::new(
        crate::task_scheduler::TaskScheduler::new(crate::task_scheduler::TaskSchedulerConfig {
            max_active: 1,
            aging_interval_ms: 1_000,
        })
        .unwrap(),
    );
    let pool = ModelGenerationPool::for_endpoint(
        "stream-provider",
        "stream-model",
        "https://provider.test",
        ModelGenerationConcurrency::single_flight(),
    )
    .unwrap();
    let quota = crate::task_scheduler::TaskSchedulerQuota::new(
        pool.identity.clone(),
        pool.max_concurrency().get(),
    )
    .unwrap();
    let admission = || {
        ModelGenerationAdmission::new(ModelGenerationConcurrency::single_flight())
            .with_scheduler_quota(
                Arc::clone(&scheduler),
                quota.clone(),
                crate::task_scheduler::TaskPriority::Foreground,
                "stream-generation",
            )
            .unwrap()
    };
    let provider_cancelled = Arc::new(tokio::sync::Notify::new());
    let client: Arc<dyn LlmClient> = Arc::new(PendingStreamingClient {
        provider_cancelled: Arc::clone(&provider_cancelled),
    });
    let first_invoker = Arc::new(LlmInvoker::new_with_admission(
        Arc::clone(&client),
        InvocationContext::new(
            Arc::<str>::from("stream-run-a"),
            Arc::<str>::from("stream-session-a"),
            CancellationToken::new(),
            None,
            InvocationGovernance::default(),
        ),
        admission(),
    ));
    let second_invoker = Arc::new(LlmInvoker::new_with_admission(
        Arc::clone(&client),
        InvocationContext::new(
            Arc::<str>::from("stream-run-b"),
            Arc::<str>::from("stream-session-b"),
            CancellationToken::new(),
            None,
            InvocationGovernance::default(),
        ),
        admission(),
    ));

    let first = first_invoker
        .complete_streaming(
            &[Message::user("first")],
            None,
            &[],
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let second_call = tokio::spawn(async move {
        second_invoker
            .complete_streaming(
                &[Message::user("second")],
                None,
                &[],
                CancellationToken::new(),
            )
            .await
    });
    for _ in 0..100 {
        let snapshot = scheduler.quota_snapshot(&quota).await.unwrap();
        if snapshot.pending == 1 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(scheduler.quota_snapshot(&quota).await.unwrap().active, 1);
    assert_eq!(scheduler.quota_snapshot(&quota).await.unwrap().pending, 1);

    drop(first);
    tokio::time::timeout(Duration::from_millis(100), provider_cancelled.notified())
        .await
        .expect("dropping the first stream must cancel its provider");
    let second = tokio::time::timeout(Duration::from_millis(200), second_call)
        .await
        .expect("the second stream should acquire after receiver drop")
        .unwrap()
        .unwrap();
    drop(second);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn dropping_proxy_receiver_cancels_pending_provider_stream() {
    let provider_cancelled = Arc::new(tokio::sync::Notify::new());
    let client: Arc<dyn LlmClient> = Arc::new(PendingStreamingClient {
        provider_cancelled: Arc::clone(&provider_cancelled),
    });
    let invocation = InvocationContext::new(
        Arc::<str>::from("run-stream-drop"),
        Arc::<str>::from("session-stream-drop"),
        CancellationToken::new(),
        None,
        InvocationGovernance::default(),
    );
    let invoker = LlmInvoker::new(client, invocation);
    let rx = invoker
        .complete_streaming(
            &[Message::user("hello")],
            None,
            &[],
            CancellationToken::new(),
        )
        .await
        .unwrap();

    drop(rx);

    tokio::time::timeout(
        std::time::Duration::from_millis(100),
        provider_cancelled.notified(),
    )
    .await
    .expect("dropping the consumer must cancel the provider stream");
}

#[tokio::test]
async fn streaming_done_binds_usage_before_the_response_reaches_the_caller() {
    let client: Arc<dyn LlmClient> = Arc::new(CompletedStreamingClient);
    let agent = AgentLoop::new(
        client,
        Arc::new(crate::tools::ToolExecutor::new("/tmp".to_string())),
        crate::tools::ToolContext::new(std::path::PathBuf::from("/tmp")),
        crate::agent::AgentConfig::default(),
    );
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let scoped = agent.scoped_llm_client_for_parts(
        Some("stream-usage-session"),
        &Some(event_tx),
        &CancellationToken::new(),
    );
    let repeated = "private repeated streaming result";
    let messages = [
        Message::user("private streaming input"),
        Message::tool_result("tool-1", repeated, false),
        Message::tool_result("tool-2", repeated, false),
    ];
    let mut stream = scoped
        .complete_streaming(&messages, None, &[], CancellationToken::new())
        .await
        .unwrap();

    assert!(matches!(stream.recv().await, Some(StreamEvent::Done(_))));
    assert!(matches!(
        event_rx.recv().await.unwrap(),
        AgentEvent::RunCapabilityBound { .. }
    ));
    let AgentEvent::ModelPresentationBound {
        snapshot: presentation,
    } = event_rx.recv().await.unwrap()
    else {
        panic!("streaming call must bind its Tool presentation")
    };
    let AgentEvent::ModelInputBound { snapshot: input } = event_rx.recv().await.unwrap() else {
        panic!("streaming call must bind its input")
    };
    presentation.validate_against(&input).unwrap();
    let AgentEvent::ModelUsageBound { snapshot: usage } = event_rx.recv().await.unwrap() else {
        panic!("streaming call must bind its usage before forwarding Done")
    };
    usage.validate_against(&input).unwrap();
    assert_eq!(usage.reported_total_tokens, 20);
    assert_eq!(usage.reported_cache_write_tokens, Some(4));
    assert_eq!(usage.tool_results.total_count, 2);
    assert_eq!(usage.tool_results.unique_count, 1);
    assert_eq!(usage.tool_results.repeated_count, 1);
    assert!(!serde_json::to_string(&usage).unwrap().contains(repeated));
}

#[tokio::test]
async fn caller_cancellation_interrupts_streaming_usage_backpressure() {
    let usage_recorded = Arc::new(tokio::sync::Notify::new());
    let client: Arc<dyn LlmClient> = Arc::new(CompletedStreamingClient);
    let agent = AgentLoop::new(
        client,
        Arc::new(crate::tools::ToolExecutor::new("/tmp".to_string())),
        crate::tools::ToolContext::new(std::path::PathBuf::from("/tmp")),
        crate::agent::AgentConfig {
            budget_guard: Some(Arc::new(UsageRecordedGuard {
                recorded: Arc::clone(&usage_recorded),
            })),
            ..crate::agent::AgentConfig::default()
        },
    );
    let (event_tx, _event_rx) = mpsc::channel(3);
    let scoped = agent.scoped_llm_client_for_parts(
        Some("stream-usage-backpressure-session"),
        &Some(event_tx),
        &CancellationToken::new(),
    );
    let caller_cancellation = CancellationToken::new();
    let mut stream = scoped
        .complete_streaming(
            &[Message::user("private streaming input")],
            None,
            &[],
            caller_cancellation.clone(),
        )
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(1), usage_recorded.notified())
        .await
        .expect("provider usage must be recorded before usage evidence blocks");
    caller_cancellation.cancel();

    let event = tokio::time::timeout(std::time::Duration::from_secs(1), stream.recv())
        .await
        .expect("caller cancellation must release usage-evidence backpressure");
    assert!(event.is_none(), "cancelled streams must not forward Done");
}

#[tokio::test]
async fn scoped_client_forks_provider_for_logical_session() {
    let observed_sessions = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LlmClient> = Arc::new(SessionBindingClient {
        bound_session: None,
        observed_sessions: Arc::clone(&observed_sessions),
    });
    let agent = AgentLoop::new(
        client,
        Arc::new(crate::tools::ToolExecutor::new("/tmp".to_string())),
        crate::tools::ToolContext::new(std::path::PathBuf::from("/tmp")),
        crate::agent::AgentConfig::default(),
    );
    let scoped =
        agent.scoped_llm_client_for_parts(Some("child-session"), &None, &CancellationToken::new());

    scoped
        .complete(&[Message::user("hello")], None, &[])
        .await
        .unwrap();

    assert_eq!(
        *observed_sessions.lock().unwrap(),
        vec!["child-session".to_string()]
    );
}

#[tokio::test]
async fn scoped_client_applies_configured_generation_timeout() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let agent = AgentLoop::new(
        Arc::new(TimeoutAwareClient {
            observed: Arc::clone(&observed),
        }),
        Arc::new(crate::tools::ToolExecutor::new("/tmp".to_string())),
        crate::tools::ToolContext::new(std::path::PathBuf::from("/tmp")),
        crate::agent::AgentConfig {
            llm_api_timeout_ms: Some(37),
            ..crate::agent::AgentConfig::default()
        },
    );

    let scoped = agent.scoped_llm_client_for_parts(
        Some("timeout-session"),
        &None,
        &CancellationToken::new(),
    );
    scoped
        .complete(&[Message::user("hello")], None, &[])
        .await
        .unwrap();

    assert_eq!(
        *observed.lock().unwrap(),
        vec![std::time::Duration::from_millis(37)]
    );
}

#[tokio::test]
async fn governed_client_keeps_provider_session_forking_available() {
    let observed_sessions = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LlmClient> = Arc::new(SessionBindingClient {
        bound_session: None,
        observed_sessions: Arc::clone(&observed_sessions),
    });
    let invocation = InvocationContext::new(
        Arc::<str>::from("parent-run"),
        Arc::<str>::from("parent-session"),
        CancellationToken::new(),
        None,
        InvocationGovernance::default(),
    );
    let governed: Arc<dyn LlmClient> = Arc::new(LlmInvoker::new(client, invocation));
    let forked = governed
        .fork_for_session("nested-child-session")
        .expect("governed client must preserve provider session forking");

    forked
        .complete(&[Message::user("hello")], None, &[])
        .await
        .unwrap();

    assert_eq!(
        *observed_sessions.lock().unwrap(),
        vec!["nested-child-session".to_string()]
    );
}

#[tokio::test]
async fn concurrent_governed_calls_record_inputs_and_deduplicate_capabilities() {
    let observed_sessions = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LlmClient> = Arc::new(SessionBindingClient {
        bound_session: None,
        observed_sessions,
    });
    let agent = AgentLoop::new(
        client,
        Arc::new(crate::tools::ToolExecutor::new("/tmp".to_string())),
        crate::tools::ToolContext::new(std::path::PathBuf::from("/tmp")),
        crate::agent::AgentConfig::default(),
    );
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let scoped = agent.scoped_llm_client_for_parts(
        Some("evidence-session"),
        &Some(event_tx),
        &CancellationToken::new(),
    );
    let messages = vec![Message::user("top-secret-model-input")];
    let tools = vec![ToolDefinition {
        name: "read".to_string(),
        description: "Read a workspace file".to_string(),
        parameters: serde_json::json!({"type": "object"}),
    }];

    let (first, second) = tokio::join!(
        scoped.complete(&messages, Some("private-system"), &tools),
        scoped.complete(&messages, Some("private-system"), &tools),
    );
    first.unwrap();
    second.unwrap();

    let capability = event_rx.recv().await.unwrap();
    let capability_snapshot = match &capability {
        AgentEvent::RunCapabilityBound {
            call_sequence,
            snapshot,
        } => {
            assert!([1, 2].contains(call_sequence));
            snapshot.validate().unwrap();
            snapshot.clone()
        }
        event => panic!("unexpected first evidence event: {event:?}"),
    };
    let mut presentations = HashMap::new();
    let mut inputs = HashMap::new();
    let mut usage_sequences = Vec::new();
    for _ in 0..6 {
        let event = event_rx.recv().await.unwrap();
        match event {
            AgentEvent::ModelPresentationBound { snapshot } => {
                presentations.insert(snapshot.call_sequence, snapshot);
            }
            AgentEvent::ModelInputBound { snapshot } => {
                snapshot.validate_against(&capability_snapshot).unwrap();
                let encoded = serde_json::to_string(&snapshot).unwrap();
                assert!(!encoded.contains("top-secret-model-input"));
                assert!(!encoded.contains("private-system"));
                inputs.insert(snapshot.call_sequence, snapshot);
            }
            AgentEvent::ModelUsageBound { snapshot } => {
                let input = inputs
                    .get(&snapshot.call_sequence)
                    .expect("input evidence must precede its usage evidence");
                snapshot.validate_against(input).unwrap();
                assert_eq!(snapshot.reported_total_tokens, 16);
                usage_sequences.push(snapshot.call_sequence);
            }
            event => panic!("unexpected model evidence event: {event:?}"),
        }
    }
    assert_eq!(presentations.len(), 2);
    for (call_sequence, input) in &inputs {
        presentations
            .get(call_sequence)
            .expect("presentation evidence must precede input evidence")
            .validate_against(input)
            .unwrap();
    }
    let mut input_sequences = inputs.keys().copied().collect::<Vec<_>>();
    input_sequences.sort_unstable();
    usage_sequences.sort_unstable();
    assert_eq!(input_sequences, [1, 2]);
    assert_eq!(usage_sequences, [1, 2]);
    assert!(matches!(
        event_rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn run_bound_helpers_share_one_evidence_sequence_and_capability_state() {
    let observed_sessions = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LlmClient> = Arc::new(SessionBindingClient {
        bound_session: None,
        observed_sessions,
    });
    let agent = AgentLoop::new(
        client,
        Arc::new(crate::tools::ToolExecutor::new("/tmp".to_string())),
        crate::tools::ToolContext::new(std::path::PathBuf::from("/tmp")),
        crate::agent::AgentConfig::default(),
    );
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let event_tx = Some(event_tx);
    let cancellation = CancellationToken::new();
    let invocation = agent.invocation_context(
        "evidence-run",
        Some("evidence-session"),
        event_tx.clone(),
        cancellation.clone(),
    );
    let run_agent = invocation.bind_agent_loop(&agent);

    for prompt in ["first private input", "second private input"] {
        let helper = run_agent.scoped_llm_client_for_parts(
            Some("evidence-session"),
            &event_tx,
            &cancellation,
        );
        helper
            .complete(&[Message::user(prompt)], None, &[])
            .await
            .unwrap();
    }

    let capability = match event_rx.recv().await.unwrap() {
        AgentEvent::RunCapabilityBound { snapshot, .. } => snapshot,
        event => panic!("unexpected first evidence event: {event:?}"),
    };
    let mut presentations = HashMap::new();
    let mut inputs = HashMap::new();
    let mut usage_sequences = Vec::new();
    for _ in 0..6 {
        match event_rx.recv().await.unwrap() {
            AgentEvent::ModelPresentationBound { snapshot } => {
                presentations.insert(snapshot.call_sequence, snapshot);
            }
            AgentEvent::ModelInputBound { snapshot } => {
                snapshot.validate_against(&capability).unwrap();
                inputs.insert(snapshot.call_sequence, snapshot);
            }
            AgentEvent::ModelUsageBound { snapshot } => {
                snapshot
                    .validate_against(
                        inputs
                            .get(&snapshot.call_sequence)
                            .expect("input evidence must precede usage"),
                    )
                    .unwrap();
                usage_sequences.push(snapshot.call_sequence);
            }
            event => panic!("unexpected model evidence event: {event:?}"),
        }
    }
    assert_eq!(presentations.len(), 2);
    for (call_sequence, input) in &inputs {
        presentations
            .get(call_sequence)
            .expect("presentation evidence must precede input evidence")
            .validate_against(input)
            .unwrap();
    }
    let mut input_sequences = inputs.keys().copied().collect::<Vec<_>>();
    input_sequences.sort_unstable();
    assert_eq!(input_sequences, [1, 2]);
    assert_eq!(usage_sequences, [1, 2]);
    assert!(matches!(
        event_rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn detached_helper_does_not_append_evidence_to_the_bound_run() {
    let observed_sessions = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LlmClient> = Arc::new(SessionBindingClient {
        bound_session: None,
        observed_sessions,
    });
    let agent = AgentLoop::new(
        client,
        Arc::new(crate::tools::ToolExecutor::new("/tmp".to_string())),
        crate::tools::ToolContext::new(std::path::PathBuf::from("/tmp")),
        crate::agent::AgentConfig::default(),
    );
    let (event_tx, mut event_rx) = mpsc::channel(8);
    let event_tx = Some(event_tx);
    let cancellation = CancellationToken::new();
    let invocation = agent.invocation_context(
        "evidence-run",
        Some("evidence-session"),
        event_tx.clone(),
        cancellation.clone(),
    );
    let run_agent = invocation.bind_agent_loop(&agent);

    let journaled =
        run_agent.scoped_llm_client_for_parts(Some("evidence-session"), &event_tx, &cancellation);
    journaled
        .complete(&[Message::user("journaled")], None, &[])
        .await
        .unwrap();
    assert!(matches!(
        event_rx.recv().await.unwrap(),
        AgentEvent::RunCapabilityBound { .. }
    ));
    assert!(matches!(
        event_rx.recv().await.unwrap(),
        AgentEvent::ModelPresentationBound { .. }
    ));
    assert!(matches!(
        event_rx.recv().await.unwrap(),
        AgentEvent::ModelInputBound { .. }
    ));
    assert!(matches!(
        event_rx.recv().await.unwrap(),
        AgentEvent::ModelUsageBound { .. }
    ));

    let detached =
        run_agent.scoped_llm_client_for_parts(Some("evidence-session"), &None, &cancellation);
    detached
        .complete(&[Message::user("detached")], None, &[])
        .await
        .unwrap();
    assert!(matches!(
        event_rx.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    let journaled_again =
        run_agent.scoped_llm_client_for_parts(Some("evidence-session"), &event_tx, &cancellation);
    journaled_again
        .complete(&[Message::user("journaled again")], None, &[])
        .await
        .unwrap();
    let AgentEvent::ModelPresentationBound {
        snapshot: presentation,
    } = event_rx.recv().await.unwrap()
    else {
        panic!("journaled model call must emit presentation evidence")
    };
    let AgentEvent::ModelInputBound { snapshot } = event_rx.recv().await.unwrap() else {
        panic!("unchanged capability must not be re-emitted")
    };
    assert_eq!(snapshot.call_sequence, 2);
    presentation.validate_against(&snapshot).unwrap();
    let AgentEvent::ModelUsageBound {
        snapshot: usage_snapshot,
    } = event_rx.recv().await.unwrap()
    else {
        panic!("journaled model call must emit usage evidence")
    };
    usage_snapshot.validate_against(&snapshot).unwrap();
}

#[tokio::test]
async fn run_cancellation_interrupts_evidence_channel_backpressure_before_provider_use() {
    let observed_sessions = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LlmClient> = Arc::new(SessionBindingClient {
        bound_session: None,
        observed_sessions: Arc::clone(&observed_sessions),
    });
    let agent = AgentLoop::new(
        client,
        Arc::new(crate::tools::ToolExecutor::new("/tmp".to_string())),
        crate::tools::ToolContext::new(std::path::PathBuf::from("/tmp")),
        crate::agent::AgentConfig::default(),
    );
    let (event_tx, _event_rx) = mpsc::channel(1);
    let capacity_probe = event_tx.clone();
    let cancellation = CancellationToken::new();
    let scoped = agent.scoped_llm_client_for_parts(
        Some("backpressure-session"),
        &Some(event_tx),
        &cancellation,
    );
    let call = tokio::spawn(async move {
        scoped
            .complete(&[Message::user("private input")], None, &[])
            .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while capacity_probe.capacity() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("capability evidence must fill the bounded channel");
    cancellation.cancel();

    let error = tokio::time::timeout(std::time::Duration::from_secs(1), call)
        .await
        .expect("run cancellation must release evidence backpressure")
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert!(observed_sessions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn run_cancellation_interrupts_usage_backpressure_after_provider_use() {
    let usage_recorded = Arc::new(tokio::sync::Notify::new());
    let observed_sessions = Arc::new(Mutex::new(Vec::new()));
    let client: Arc<dyn LlmClient> = Arc::new(SessionBindingClient {
        bound_session: None,
        observed_sessions: Arc::clone(&observed_sessions),
    });
    let agent = AgentLoop::new(
        client,
        Arc::new(crate::tools::ToolExecutor::new("/tmp".to_string())),
        crate::tools::ToolContext::new(std::path::PathBuf::from("/tmp")),
        crate::agent::AgentConfig {
            budget_guard: Some(Arc::new(UsageRecordedGuard {
                recorded: Arc::clone(&usage_recorded),
            })),
            ..crate::agent::AgentConfig::default()
        },
    );
    let (event_tx, _event_rx) = mpsc::channel(3);
    let cancellation = CancellationToken::new();
    let scoped = agent.scoped_llm_client_for_parts(
        Some("usage-backpressure-session"),
        &Some(event_tx),
        &cancellation,
    );
    let call = tokio::spawn(async move {
        scoped
            .complete(&[Message::user("private input")], None, &[])
            .await
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), usage_recorded.notified())
        .await
        .expect("provider usage must be recorded before usage evidence blocks");
    cancellation.cancel();

    let error = tokio::time::timeout(std::time::Duration::from_secs(1), call)
        .await
        .expect("run cancellation must release usage-evidence backpressure")
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(observed_sessions.lock().unwrap().len(), 1);
}
