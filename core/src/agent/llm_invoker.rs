//! Per-run LLM invocation gateway.
//!
//! Every provider call made on behalf of an agent run passes through this
//! facade. It applies cancellation and budget accounting at the provider-call
//! boundary, including retries, structured repair calls, streaming calls, and
//! helper operations such as compaction.

use super::{AgentEvent, AgentLoop, InvocationContext};
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::harness_evidence::{
    ModelCallObservation, ModelInputKindV1, ModelPresentationApplicationV1, ModelUsageBinding,
    ModelUsageSnapshotV1,
};
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
    presentation_application: ModelPresentationApplicationV1,
}

/// The non-streaming model-call shapes handled by the middleware boundary.
///
/// Keeping this discriminator next to the request prevents each caller from
/// independently choosing evidence and budget semantics for a provider call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelCallKind {
    Completion,
    Structured,
}

/// Typed input to the run-bound model middleware.
///
/// This is intentionally borrowed: the middleware never owns prompt content
/// and therefore cannot outlive the request that is being admitted. Streaming
/// has a separate transport contract because its provider lifetime is managed
/// by the returned proxy receiver.
struct ModelCallRequest<'a> {
    kind: ModelCallKind,
    messages: &'a [Message],
    system: Option<&'a str>,
    tools: &'a [ToolDefinition],
    directive: Option<&'a StructuredDirective>,
}

impl<'a> ModelCallRequest<'a> {
    fn completion(
        messages: &'a [Message],
        system: Option<&'a str>,
        tools: &'a [ToolDefinition],
    ) -> Self {
        Self {
            kind: ModelCallKind::Completion,
            messages,
            system,
            tools,
            directive: None,
        }
    }

    fn structured(
        messages: &'a [Message],
        system: Option<&'a str>,
        tools: &'a [ToolDefinition],
        directive: &'a StructuredDirective,
    ) -> Self {
        Self {
            kind: ModelCallKind::Structured,
            messages,
            system,
            tools,
            directive: Some(directive),
        }
    }

    fn observation(
        &self,
        presentation_application: ModelPresentationApplicationV1,
    ) -> ModelCallObservation<'a> {
        let kind = match self.kind {
            ModelCallKind::Completion => ModelInputKindV1::Completion,
            ModelCallKind::Structured => ModelInputKindV1::Structured,
        };
        ModelCallObservation::with_presentation_application(
            kind,
            self.messages,
            self.system,
            self.tools,
            self.directive,
            estimate_prompt_tokens(self.messages, self.system, self.tools),
            presentation_application,
        )
    }

    fn idempotency_identity(
        &self,
        scope: &str,
    ) -> Result<
        crate::execution_identity::ExecutionIdentityV1,
        crate::execution_identity::ExecutionIdentityError,
    > {
        model_request_identity(
            scope,
            self.kind,
            self.messages,
            self.system,
            self.tools,
            self.directive,
        )
    }
}

/// Typed output of one admitted non-streaming model call.
///
/// Usage is copied at the middleware boundary so future cost/retry metadata
/// can be added without changing provider response types or evidence wiring.
#[derive(Debug, Clone)]
struct ModelCallOutcome {
    response: LlmResponse,
    usage: TokenUsage,
}

impl ModelCallOutcome {
    fn from_response(response: LlmResponse) -> Self {
        Self {
            usage: response.usage.clone(),
            response,
        }
    }

    fn into_response(self) -> LlmResponse {
        let Self { response, usage } = self;
        debug_assert_eq!(usage.total_tokens, response.usage.total_tokens);
        response
    }
}

/// The streaming model-call shapes handled by the middleware boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelStreamKind {
    Streaming,
    StreamingStructured,
}

/// Typed input to the run-bound streaming middleware.
struct ModelStreamRequest<'a> {
    kind: ModelStreamKind,
    messages: &'a [Message],
    system: Option<&'a str>,
    tools: &'a [ToolDefinition],
    directive: Option<&'a StructuredDirective>,
    caller_cancellation: CancellationToken,
}

impl<'a> ModelStreamRequest<'a> {
    fn completion(
        messages: &'a [Message],
        system: Option<&'a str>,
        tools: &'a [ToolDefinition],
        caller_cancellation: CancellationToken,
    ) -> Self {
        Self {
            kind: ModelStreamKind::Streaming,
            messages,
            system,
            tools,
            directive: None,
            caller_cancellation,
        }
    }

    fn structured(
        messages: &'a [Message],
        system: Option<&'a str>,
        tools: &'a [ToolDefinition],
        directive: &'a StructuredDirective,
        caller_cancellation: CancellationToken,
    ) -> Self {
        Self {
            kind: ModelStreamKind::StreamingStructured,
            messages,
            system,
            tools,
            directive: Some(directive),
            caller_cancellation,
        }
    }

    fn observation(
        &self,
        presentation_application: ModelPresentationApplicationV1,
    ) -> ModelCallObservation<'a> {
        let kind = match self.kind {
            ModelStreamKind::Streaming => ModelInputKindV1::Streaming,
            ModelStreamKind::StreamingStructured => ModelInputKindV1::StreamingStructured,
        };
        ModelCallObservation::with_presentation_application(
            kind,
            self.messages,
            self.system,
            self.tools,
            self.directive,
            estimate_prompt_tokens(self.messages, self.system, self.tools),
            presentation_application,
        )
    }

    fn idempotency_identity(
        &self,
        scope: &str,
    ) -> Result<
        crate::execution_identity::ExecutionIdentityV1,
        crate::execution_identity::ExecutionIdentityError,
    > {
        model_request_identity(
            scope,
            self.kind,
            self.messages,
            self.system,
            self.tools,
            self.directive,
        )
    }
}

/// Typed output of the streaming middleware. The receiver owns the proxy
/// lifetime; dropping it is the explicit signal to cancel provider work.
struct ModelStreamOutcome {
    receiver: mpsc::Receiver<StreamEvent>,
}

impl ModelStreamOutcome {
    fn into_receiver(self) -> mpsc::Receiver<StreamEvent> {
        self.receiver
    }
}

fn model_request_identity(
    scope: &str,
    kind: impl std::fmt::Debug,
    messages: &[Message],
    system: Option<&str>,
    tools: &[ToolDefinition],
    directive: Option<&StructuredDirective>,
) -> Result<
    crate::execution_identity::ExecutionIdentityV1,
    crate::execution_identity::ExecutionIdentityError,
> {
    let directive = directive.map(|directive| {
        let response_format = directive
            .response_format
            .as_ref()
            .map(|format| match format {
                crate::llm::structured::ResponseFormat::JsonObject => {
                    serde_json::json!({"kind": "json_object"})
                }
                crate::llm::structured::ResponseFormat::JsonSchema { name, schema } => {
                    serde_json::json!({"kind": "json_schema", "name": name, "schema": schema})
                }
            });
        serde_json::json!({
            "force_tool": directive.force_tool,
            "response_format": response_format,
            "validation_schema": directive.validation_schema,
        })
    });
    crate::execution_identity::ExecutionIdentityV1::derive(
        crate::execution_identity::MODEL_CALL_IDENTITY_DOMAIN_V1,
        &serde_json::json!({
            "scope": scope,
            "kind": format!("{kind:?}"),
            "messages": messages,
            "system": system,
            "tools": tools,
            "directive": directive,
        }),
    )
}

impl LlmInvoker {
    fn new(inner: Arc<dyn LlmClient>, invocation: InvocationContext) -> Self {
        Self {
            inner,
            invocation,
            presentation_application: ModelPresentationApplicationV1::Auxiliary,
        }
    }

    fn profiled(inner: Arc<dyn LlmClient>, invocation: InvocationContext) -> Self {
        Self {
            inner,
            invocation,
            presentation_application: ModelPresentationApplicationV1::Profiled,
        }
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
        let usage_binding = self.record_model_evidence(observation).await?;

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
        record_model_usage(
            &self.invocation,
            usage_binding.as_ref(),
            &response.usage,
            self.invocation.cancellation(),
        )
        .await?;
        Ok(response)
    }

    /// Execute one typed non-streaming model request through the common
    /// admission, cancellation, evidence, usage, and error phases.
    async fn invoke_model(
        &self,
        request: ModelCallRequest<'_>,
    ) -> anyhow::Result<ModelCallOutcome> {
        let identity = request
            .idempotency_identity(self.invocation.session_id())
            .map_err(|error| anyhow::anyhow!("derive model call identity: {error}"))?;
        debug_assert!(identity.validate().is_ok());
        let observation = request.observation(self.presentation_application);
        let response = match request.kind {
            ModelCallKind::Completion => {
                self.invoke_response(
                    observation,
                    self.inner
                        .complete(request.messages, request.system, request.tools),
                )
                .await?
            }
            ModelCallKind::Structured => {
                let directive = request.directive.ok_or_else(|| {
                    anyhow::anyhow!("structured model request is missing its directive")
                })?;
                self.invoke_response(
                    observation,
                    self.inner.complete_structured(
                        request.messages,
                        request.system,
                        request.tools,
                        directive,
                    ),
                )
                .await?
            }
        };
        Ok(ModelCallOutcome::from_response(response))
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
        let usage_binding = self.record_model_evidence(observation).await?;

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

        Ok(self.proxy_stream(
            inner_rx,
            provider_cancellation,
            cancellation_watcher,
            usage_binding,
        ))
    }

    async fn record_model_evidence(
        &self,
        observation: ModelCallObservation<'_>,
    ) -> anyhow::Result<Option<ModelUsageBinding>> {
        let Some(tx) = self.invocation.event_tx() else {
            return Ok(None);
        };
        let Some(evidence) = self.invocation.capture_model_evidence(observation)? else {
            return Ok(None);
        };
        let usage_binding =
            ModelUsageBinding::from_input(&evidence.input, evidence.tool_result_context);
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
            result = tx.send(AgentEvent::ModelPresentationBound {
                snapshot: evidence.presentation,
            }) => result,
        };
        let _ = send_result;
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
        Ok(Some(usage_binding))
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
        usage_binding: Option<ModelUsageBinding>,
    ) -> mpsc::Receiver<StreamEvent> {
        let (tx, rx) = mpsc::channel(64);
        let budget_guard = self.invocation.governance().budget_guard().cloned();
        let session_id = self.invocation.session_id().to_string();
        let invocation = self.invocation.clone();

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
                    if let Err(error) = record_model_usage(
                        &invocation,
                        usage_binding.as_ref(),
                        &response.usage,
                        &provider_cancellation,
                    )
                    .await
                    {
                        if provider_cancellation.is_cancelled() {
                            break;
                        }
                        tracing::warn!(
                            error = %error,
                            call_sequence = usage_binding.as_ref().map(|binding| binding.call_sequence()),
                            "Failed to record model usage evidence"
                        );
                    }
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

    /// Execute one typed streaming request through the common middleware
    /// phases while retaining receiver-owned provider lifetime semantics.
    async fn invoke_stream_model(
        &self,
        request: ModelStreamRequest<'_>,
    ) -> anyhow::Result<ModelStreamOutcome> {
        let identity = request
            .idempotency_identity(self.invocation.session_id())
            .map_err(|error| anyhow::anyhow!("derive streaming model call identity: {error}"))?;
        debug_assert!(identity.validate().is_ok());
        let observation = request.observation(self.presentation_application);
        let caller_cancellation = request.caller_cancellation.clone();
        let receiver = match request.kind {
            ModelStreamKind::Streaming => {
                self.invoke_stream(observation, caller_cancellation, |provider_token| {
                    self.inner.complete_streaming(
                        request.messages,
                        request.system,
                        request.tools,
                        provider_token,
                    )
                })
                .await?
            }
            ModelStreamKind::StreamingStructured => {
                let directive = request.directive.ok_or_else(|| {
                    anyhow::anyhow!("structured streaming request is missing its directive")
                })?;
                self.invoke_stream(observation, caller_cancellation, |provider_token| {
                    self.inner.complete_streaming_structured(
                        request.messages,
                        request.system,
                        request.tools,
                        directive,
                        provider_token,
                    )
                })
                .await?
            }
        };
        Ok(ModelStreamOutcome { receiver })
    }
}

#[async_trait]
impl LlmClient for LlmInvoker {
    fn model_generation_concurrency(&self) -> ModelGenerationConcurrency {
        self.inner.model_generation_concurrency()
    }

    fn fork_for_session(&self, session_id: &str) -> Option<Arc<dyn LlmClient>> {
        self.inner.fork_for_session(session_id).map(|inner| {
            Arc::new(Self {
                inner,
                invocation: self.invocation.clone(),
                presentation_application: self.presentation_application,
            }) as Arc<dyn LlmClient>
        })
    }

    fn with_active_generation_timeout(&self, timeout: Duration) -> Option<Arc<dyn LlmClient>> {
        self.inner
            .with_active_generation_timeout(timeout)
            .map(|inner| {
                Arc::new(Self {
                    inner,
                    invocation: self.invocation.clone(),
                    presentation_application: self.presentation_application,
                }) as Arc<dyn LlmClient>
            })
    }

    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.invoke_model(ModelCallRequest::completion(messages, system, tools))
            .await
            .map(ModelCallOutcome::into_response)
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.invoke_stream_model(ModelStreamRequest::completion(
            messages,
            system,
            tools,
            cancel_token,
        ))
        .await
        .map(ModelStreamOutcome::into_receiver)
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
        self.invoke_model(ModelCallRequest::structured(
            messages, system, tools, directive,
        ))
        .await
        .map(ModelCallOutcome::into_response)
    }

    async fn complete_streaming_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        directive: &StructuredDirective,
        cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.invoke_stream_model(ModelStreamRequest::structured(
            messages,
            system,
            tools,
            directive,
            cancel_token,
        ))
        .await
        .map(ModelStreamOutcome::into_receiver)
    }
}

impl AgentLoop {
    pub(super) fn scoped_llm_client(&self, invocation: &InvocationContext) -> Arc<dyn LlmClient> {
        let provider_client = self
            .llm_client
            .fork_for_session(invocation.session_id())
            .unwrap_or_else(|| Arc::clone(&self.llm_client));
        let provider_client = self
            .config
            .llm_api_timeout_ms
            .and_then(|timeout_ms| {
                provider_client.with_active_generation_timeout(Duration::from_millis(timeout_ms))
            })
            .unwrap_or(provider_client);
        Arc::new(LlmInvoker::new(provider_client, invocation.clone()))
    }

    fn scoped_profiled_llm_client(&self, invocation: &InvocationContext) -> Arc<dyn LlmClient> {
        let provider_client = self
            .llm_client
            .fork_for_session(invocation.session_id())
            .unwrap_or_else(|| Arc::clone(&self.llm_client));
        let provider_client = self
            .config
            .llm_api_timeout_ms
            .and_then(|timeout_ms| {
                provider_client.with_active_generation_timeout(Duration::from_millis(timeout_ms))
            })
            .unwrap_or(provider_client);
        Arc::new(LlmInvoker::profiled(provider_client, invocation.clone()))
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

    /// Build the run-owned provider facade for a main agent turn whose Tool
    /// definitions were produced by the frozen presentation profile.
    pub(super) fn scoped_profiled_llm_client_for_parts(
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
            return self.scoped_profiled_llm_client(invocation);
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
        self.scoped_profiled_llm_client(&invocation)
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

async fn record_model_usage(
    invocation: &InvocationContext,
    binding: Option<&ModelUsageBinding>,
    usage: &TokenUsage,
    cancellation: &CancellationToken,
) -> anyhow::Result<()> {
    let (Some(tx), Some(binding)) = (invocation.event_tx(), binding) else {
        return Ok(());
    };
    let snapshot = ModelUsageSnapshotV1::from_binding(binding, usage)?;
    let send_result = tokio::select! {
        biased;
        _ = cancellation.cancelled() => {
            anyhow::bail!("Operation cancelled")
        }
        result = tx.send(AgentEvent::ModelUsageBound { snapshot }) => result,
    };
    let _ = send_result;
    Ok(())
}

#[cfg(test)]
mod tests;
