use super::*;
use crate::agent::invocation_context::InvocationGovernance;
use std::collections::HashMap;
use std::sync::Mutex;

struct PendingStreamingClient {
    provider_cancelled: Arc<tokio::sync::Notify>,
}

struct CompletedStreamingClient;

struct UsageRecordedGuard {
    recorded: Arc<tokio::sync::Notify>,
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
    let AgentEvent::ModelInputBound { snapshot: input } = event_rx.recv().await.unwrap() else {
        panic!("streaming call must bind its input")
    };
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
    let (event_tx, _event_rx) = mpsc::channel(2);
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
    let mut inputs = HashMap::new();
    let mut usage_sequences = Vec::new();
    for _ in 0..4 {
        let event = event_rx.recv().await.unwrap();
        match event {
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
    let mut inputs = HashMap::new();
    let mut usage_sequences = Vec::new();
    for _ in 0..4 {
        match event_rx.recv().await.unwrap() {
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
    let AgentEvent::ModelInputBound { snapshot } = event_rx.recv().await.unwrap() else {
        panic!("unchanged capability must not be re-emitted")
    };
    assert_eq!(snapshot.call_sequence, 2);
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
    let (event_tx, _event_rx) = mpsc::channel(2);
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
