//! Per-run LLM invocation gateway.
//!
//! Every provider call made on behalf of an agent run passes through this
//! facade. It applies cancellation and budget accounting at the provider-call
//! boundary, including retries, structured repair calls, streaming calls, and
//! helper operations such as compaction.

use super::{AgentEvent, AgentLoop, InvocationContext};
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::llm::structured::{NativeStructuredSupport, StructuredDirective};
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition};
use crate::token_estimate::estimate_prompt_tokens;
use async_trait::async_trait;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// A provider facade bound to exactly one [`InvocationContext`].
struct LlmInvoker {
    inner: Arc<dyn LlmClient>,
    invocation: InvocationContext,
}

impl LlmInvoker {
    fn new(inner: Arc<dyn LlmClient>, invocation: InvocationContext) -> Self {
        Self { inner, invocation }
    }

    async fn invoke_response<F>(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        invocation: F,
    ) -> anyhow::Result<LlmResponse>
    where
        F: Future<Output = anyhow::Result<LlmResponse>> + Send,
    {
        check_before_llm(
            self.invocation.governance().budget_guard(),
            self.invocation.session_id(),
            estimate_prompt_tokens(messages, system, tools),
            self.invocation.event_tx(),
            self.invocation.cancellation(),
        )
        .await?;

        let response = tokio::select! {
            biased;
            _ = self.invocation.cancellation().cancelled() => {
                anyhow::bail!("Operation cancelled by user")
            }
            response = invocation => response?,
        };
        record_after_llm(
            self.invocation.governance().budget_guard(),
            self.invocation.session_id(),
            &response.usage,
        )
        .await;
        Ok(response)
    }

    async fn invoke_stream<F, Fut>(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        caller_cancellation: CancellationToken,
        setup: F,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>>
    where
        F: FnOnce(CancellationToken) -> Fut + Send,
        Fut: Future<Output = anyhow::Result<mpsc::Receiver<StreamEvent>>> + Send,
    {
        check_before_llm(
            self.invocation.governance().budget_guard(),
            self.invocation.session_id(),
            estimate_prompt_tokens(messages, system, tools),
            self.invocation.event_tx(),
            self.invocation.cancellation(),
        )
        .await?;

        let caller_signal = caller_cancellation.clone();
        let (provider_cancellation, cancellation_watcher) =
            self.combine_cancellation(caller_cancellation);
        let setup = setup(provider_cancellation.clone());
        let setup_result = tokio::select! {
            biased;
            _ = self.invocation.cancellation().cancelled() => {
                Err(anyhow::anyhow!("Operation cancelled by user"))
            }
            _ = caller_signal.cancelled() => {
                Err(anyhow::anyhow!("Operation cancelled by caller"))
            }
            result = setup => result,
        };
        let inner_rx = match setup_result {
            Ok(rx) => rx,
            Err(error) => {
                provider_cancellation.cancel();
                cancellation_watcher.abort();
                return Err(error);
            }
        };

        Ok(self.proxy_stream(inner_rx, provider_cancellation, cancellation_watcher))
    }

    fn combine_cancellation(
        &self,
        caller_cancellation: CancellationToken,
    ) -> (CancellationToken, JoinHandle<()>) {
        let run_cancellation = self.invocation.cancellation().clone();
        let provider_cancellation = CancellationToken::new();
        let signal = provider_cancellation.clone();
        let watcher = tokio::spawn(async move {
            tokio::select! {
                _ = run_cancellation.cancelled() => {}
                _ = caller_cancellation.cancelled() => {}
            }
            signal.cancel();
        });
        (provider_cancellation, watcher)
    }

    fn proxy_stream(
        &self,
        mut inner_rx: mpsc::Receiver<StreamEvent>,
        provider_cancellation: CancellationToken,
        cancellation_watcher: JoinHandle<()>,
    ) -> mpsc::Receiver<StreamEvent> {
        let (tx, rx) = mpsc::channel(64);
        let budget_guard = self.invocation.governance().budget_guard().cloned();
        let session_id = self.invocation.session_id().to_string();

        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    biased;
                    _ = provider_cancellation.cancelled() => break,
                    _ = tx.closed() => break,
                    event = inner_rx.recv() => event,
                };
                let Some(event) = event else {
                    break;
                };
                if let StreamEvent::Done(response) = &event {
                    record_after_llm(budget_guard.as_ref(), &session_id, &response.usage).await;
                }
                let finished = matches!(event, StreamEvent::Done(_));
                if tx.send(event).await.is_err() || finished {
                    break;
                }
            }
            provider_cancellation.cancel();
            cancellation_watcher.abort();
        });
        rx
    }
}

#[async_trait]
impl LlmClient for LlmInvoker {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.invoke_response(
            messages,
            system,
            tools,
            self.inner.complete(messages, system, tools),
        )
        .await
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.invoke_stream(messages, system, tools, cancel_token, |provider_token| {
            self.inner
                .complete_streaming(messages, system, tools, provider_token)
        })
        .await
    }

    fn native_structured_support(&self) -> NativeStructuredSupport {
        self.inner.native_structured_support()
    }

    async fn complete_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        directive: &StructuredDirective,
    ) -> anyhow::Result<LlmResponse> {
        self.invoke_response(
            messages,
            system,
            tools,
            self.inner
                .complete_structured(messages, system, tools, directive),
        )
        .await
    }

    async fn complete_streaming_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        directive: &StructuredDirective,
        cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.invoke_stream(messages, system, tools, cancel_token, |provider_token| {
            self.inner.complete_streaming_structured(
                messages,
                system,
                tools,
                directive,
                provider_token,
            )
        })
        .await
    }
}

impl AgentLoop {
    pub(super) fn scoped_llm_client(&self, invocation: &InvocationContext) -> Arc<dyn LlmClient> {
        let provider_client = self
            .llm_client
            .fork_for_session(invocation.session_id())
            .unwrap_or_else(|| Arc::clone(&self.llm_client));
        Arc::new(LlmInvoker::new(provider_client, invocation.clone()))
    }

    /// Compatibility helper for internal paths not yet carrying the aggregate
    /// context explicitly. The resulting client still snapshots one immutable
    /// scope and applies the same provider boundary.
    pub(crate) fn scoped_llm_client_for_parts(
        &self,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &CancellationToken,
    ) -> Arc<dyn LlmClient> {
        let run_id = self
            .checkpoint_run_id
            .clone()
            .unwrap_or_else(|| format!("standalone-{}", uuid::Uuid::new_v4()));
        let invocation =
            self.invocation_context(run_id, session_id, event_tx.clone(), cancel_token.clone());
        self.scoped_llm_client(&invocation)
    }
}

async fn check_before_llm(
    budget_guard: Option<&Arc<dyn BudgetGuard>>,
    session_id: &str,
    estimated_prompt_tokens: usize,
    event_tx: &Option<mpsc::Sender<AgentEvent>>,
    cancel_token: &CancellationToken,
) -> anyhow::Result<()> {
    let Some(guard) = budget_guard else {
        if cancel_token.is_cancelled() {
            anyhow::bail!("Operation cancelled by user");
        }
        return Ok(());
    };

    let decision = tokio::select! {
        biased;
        _ = cancel_token.cancelled() => anyhow::bail!("Operation cancelled by user"),
        decision = guard.check_before_llm(session_id, estimated_prompt_tokens) => decision,
    };

    match decision {
        BudgetDecision::Allow => Ok(()),
        BudgetDecision::SoftLimit {
            resource,
            consumed,
            limit,
            message,
        } => {
            if let Some(tx) = event_tx {
                let _ = tx
                    .send(AgentEvent::BudgetThresholdHit {
                        resource,
                        kind: "soft".to_string(),
                        consumed,
                        limit,
                        message,
                    })
                    .await;
            }
            Ok(())
        }
        BudgetDecision::Deny { resource, reason } => {
            if let Some(tx) = event_tx {
                let _ = tx
                    .send(AgentEvent::BudgetThresholdHit {
                        resource: resource.clone(),
                        kind: "hard".to_string(),
                        consumed: 0.0,
                        limit: 0.0,
                        message: Some(reason.clone()),
                    })
                    .await;
            }
            Err(anyhow::Error::new(
                crate::error::CodeError::BudgetExhausted { resource, reason },
            ))
        }
    }
}

async fn record_after_llm(
    budget_guard: Option<&Arc<dyn BudgetGuard>>,
    session_id: &str,
    usage: &TokenUsage,
) {
    if let Some(guard) = budget_guard {
        guard.record_after_llm(session_id, usage).await;
    }
}

#[cfg(test)]
mod tests {
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
        let scoped = agent.scoped_llm_client_for_parts(
            Some("child-session"),
            &None,
            &CancellationToken::new(),
        );

        scoped
            .complete(&[Message::user("hello")], None, &[])
            .await
            .unwrap();

        assert_eq!(
            *observed_sessions.lock().unwrap(),
            vec!["child-session".to_string()]
        );
    }
}
