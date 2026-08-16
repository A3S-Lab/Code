use super::*;
use crate::agent::invocation_context::InvocationGovernance;
use std::sync::Mutex;

struct PendingStreamingClient {
    provider_cancelled: Arc<tokio::sync::Notify>,
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
    let first_input = event_rx.recv().await.unwrap();
    let second_input = event_rx.recv().await.unwrap();
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
    let mut sequences = Vec::new();
    for event in [first_input, second_input] {
        match event {
            AgentEvent::ModelInputBound { snapshot } => {
                sequences.push(snapshot.call_sequence);
                snapshot.validate_against(&capability_snapshot).unwrap();
                let encoded = serde_json::to_string(&snapshot).unwrap();
                assert!(!encoded.contains("top-secret-model-input"));
                assert!(!encoded.contains("private-system"));
            }
            event => panic!("unexpected model-input evidence event: {event:?}"),
        }
    }
    sequences.sort_unstable();
    assert_eq!(sequences, [1, 2]);
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
    let mut sequences = Vec::new();
    for _ in 0..2 {
        match event_rx.recv().await.unwrap() {
            AgentEvent::ModelInputBound { snapshot } => {
                snapshot.validate_against(&capability).unwrap();
                sequences.push(snapshot.call_sequence);
            }
            event => panic!("unexpected model-input evidence event: {event:?}"),
        }
    }
    assert_eq!(sequences, [1, 2]);
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
