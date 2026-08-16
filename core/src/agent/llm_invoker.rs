//! Per-run LLM invocation gateway.
//!
//! Every provider call made on behalf of an agent run passes through this
//! facade. It applies cancellation and budget accounting at the provider-call
//! boundary, including retries, structured repair calls, streaming calls, and
//! helper operations such as compaction.

use super::{AgentEvent, AgentLoop, InvocationContext};
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::harness_evidence::{ModelCallObservation, ModelInputKindV1};
use crate::llm::structured::{NativeStructuredSupport, StructuredDirective};
use crate::llm::{
    estimate_prompt_tokens, LlmClient, LlmResponse, Message, ModelGenerationConcurrency,
    StreamEvent, TokenUsage, ToolDefinition,
};
use async_trait::async_trait;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
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
        observation: ModelCallObservation<'_>,
        invocation: F,
    ) -> anyhow::Result<LlmResponse>
    where
        F: Future<Output = anyhow::Result<LlmResponse>> + Send,
    {
        check_before_llm(
            self.invocation.governance().budget_guard(),
            self.invocation.session_id(),
            observation.estimated_prompt_tokens,
            self.invocation.event_tx(),
            self.invocation.cancellation(),
        )
        .await?;
        self.record_model_evidence(observation).await?;

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
        observation: ModelCallObservation<'_>,
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
            observation.estimated_prompt_tokens,
            self.invocation.event_tx(),
            self.invocation.cancellation(),
        )
        .await?;
        self.record_model_evidence(observation).await?;

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

    async fn record_model_evidence(
        &self,
        observation: ModelCallObservation<'_>,
    ) -> anyhow::Result<()> {
        let Some(tx) = self.invocation.event_tx() else {
            return Ok(());
        };
        let Some(evidence) = self.invocation.capture_model_evidence(observation)? else {
            return Ok(());
        };
        if !self
            .invocation
            .send_capability_if_changed(tx, evidence.input.call_sequence, evidence.capability)
            .await
        {
            anyhow::bail!("Operation cancelled by user");
        }
        let send_result = tokio::select! {
            biased;
            _ = self.invocation.cancellation().cancelled() => {
                anyhow::bail!("Operation cancelled by user")
            }
            result = tx.send(AgentEvent::ModelInputBound {
                snapshot: evidence.input,
            }) => result,
        };
        let _ = send_result;
        Ok(())
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
    fn model_generation_concurrency(&self) -> ModelGenerationConcurrency {
        self.inner.model_generation_concurrency()
    }

    fn fork_for_session(&self, session_id: &str) -> Option<Arc<dyn LlmClient>> {
        self.inner
            .fork_for_session(session_id)
            .map(|inner| Arc::new(Self::new(inner, self.invocation.clone())) as Arc<dyn LlmClient>)
    }

    fn with_active_generation_timeout(&self, timeout: Duration) -> Option<Arc<dyn LlmClient>> {
        self.inner
            .with_active_generation_timeout(timeout)
            .map(|inner| Arc::new(Self::new(inner, self.invocation.clone())) as Arc<dyn LlmClient>)
    }

    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let observation = ModelCallObservation::new(
            ModelInputKindV1::Completion,
            messages,
            system,
            tools,
            None,
            estimate_prompt_tokens(messages, system, tools),
        );
        self.invoke_response(observation, self.inner.complete(messages, system, tools))
            .await
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let observation = ModelCallObservation::new(
            ModelInputKindV1::Streaming,
            messages,
            system,
            tools,
            None,
            estimate_prompt_tokens(messages, system, tools),
        );
        self.invoke_stream(observation, cancel_token, |provider_token| {
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
        let observation = ModelCallObservation::new(
            ModelInputKindV1::Structured,
            messages,
            system,
            tools,
            Some(directive),
            estimate_prompt_tokens(messages, system, tools),
        );
        self.invoke_response(
            observation,
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
        let observation = ModelCallObservation::new(
            ModelInputKindV1::StreamingStructured,
            messages,
            system,
            tools,
            Some(directive),
            estimate_prompt_tokens(messages, system, tools),
        );
        self.invoke_stream(observation, cancel_token, |provider_token| {
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
    /// context explicitly. A run-bound loop reuses its aggregate invocation so
    /// helpers share one evidence sequence; a standalone loop snapshots a new
    /// scope and applies the same provider boundary.
    pub(crate) fn scoped_llm_client_for_parts(
        &self,
        session_id: Option<&str>,
        event_tx: &Option<mpsc::Sender<AgentEvent>>,
        cancel_token: &CancellationToken,
    ) -> Arc<dyn LlmClient> {
        if let Some(invocation) = self
            .bound_invocation
            .as_ref()
            .filter(|invocation| invocation.matches_parts(session_id, event_tx))
        {
            return self.scoped_llm_client(invocation);
        }
        let run_id = self.bound_invocation.as_ref().map_or_else(
            || {
                self.checkpoint_run_id
                    .clone()
                    .unwrap_or_else(|| format!("standalone-{}", uuid::Uuid::new_v4()))
            },
            |bound| format!("{}-aux-{}", bound.run_id(), uuid::Uuid::new_v4()),
        );
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
mod tests;
