//! LLM client abstraction layer
//!
//! Provides a unified interface for interacting with LLM providers
//! (Anthropic Claude, OpenAI, Zhipu AI GLM, and OpenAI-compatible providers).

mod admission;
pub mod anthropic;
pub mod codex_login;
mod error;
pub mod factory;
pub mod http;
pub mod openai;
pub mod structured;
mod token_estimation;
mod types;
pub mod zhipu;

// Re-export public types
pub use admission::{
    ModelGenerationAdmission, ModelGenerationAdmissionError, ModelGenerationConcurrency,
    ModelGenerationPermit, ModelGenerationPool, ModelGenerationPoolError,
    ModelGenerationPoolHealthSnapshot,
};
pub use anthropic::AnthropicClient;
pub use codex_login::CodexLoginClient;
pub(crate) use error::non_retryable_llm_error_message;
pub use error::NonRetryableLlmError;
pub use factory::{create_client_with_config, LlmConfig};
pub use http::{
    clear_http_metrics_callback, default_http_client, set_http_metrics_callback, HttpClient,
    HttpClientError, HttpMetricsCallback, HttpMetricsRecord, HttpResponse, StreamingHttpResponse,
};
pub use openai::OpenAiClient;
pub(crate) use token_estimation::{
    estimate_message_tokens, estimate_prompt_tokens, estimate_tool_result_contents_tokens,
};
pub use types::*;
pub use zhipu::ZhipuClient;

use anyhow::Result;
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// LLM client trait
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Report the client's explicitly supported active-generation capacity.
    ///
    /// The conservative default is single-flight. Providers that can safely
    /// serve more active generations must override this with a typed contract;
    /// callers must not infer concurrency from provider names or endpoint
    /// strings.
    fn model_generation_concurrency(&self) -> ModelGenerationConcurrency {
        ModelGenerationConcurrency::single_flight()
    }

    /// Describe the non-secret provider/model capacity pool shared by this
    /// client. The default is `None` for custom clients that do not expose a
    /// stable routing identity; they retain the existing per-client gate.
    fn model_generation_pool(&self) -> Option<ModelGenerationPool> {
        None
    }

    /// Rebind a governed model facade to an invocation-owned generation gate.
    ///
    /// This is primarily used when a nested workflow supplies a tighter
    /// admission gate than the surrounding session (for example, a Flow step
    /// with its own `maxConcurrentGenerations` limit). Raw provider clients do
    /// not need to implement this hook; the agent runtime can wrap them. A
    /// client that already owns model-generation admission should preserve the
    /// provider transport while replacing the facade's gate and, when given,
    /// consuming the one pre-admitted permit exactly once.
    fn bind_model_generation_admission(
        &self,
        _admission: ModelGenerationAdmission,
        _preadmitted: Option<std::sync::Arc<ModelGenerationPermit>>,
    ) -> Option<std::sync::Arc<dyn LlmClient>> {
        None
    }

    /// Whether this client already applies the model-generation admission
    /// contract around every provider call. Built-in run-bound clients use
    /// this marker so structured tools do not acquire the same permit twice.
    fn model_generation_is_managed(&self) -> bool {
        false
    }

    /// Take queue wait accumulated by a managed client since the previous
    /// observation. Unmanaged clients report zero.
    fn take_model_generation_queue_wait(&self) -> Duration {
        Duration::ZERO
    }

    /// Derive a provider client bound to one logical agent session.
    ///
    /// Stateless providers can keep the default and share the existing client.
    /// Account-backed providers whose transport uses a live session identity
    /// should return an independent client so parallel child agents do not
    /// contend for the parent's active operation.
    fn fork_for_session(&self, _session_id: &str) -> Option<std::sync::Arc<dyn LlmClient>> {
        None
    }

    /// Return a view of this client configured for one active generation
    /// deadline. The caller still owns and enforces the outer deadline.
    ///
    /// Composite and account-backed clients can use this budget to configure
    /// their underlying transport without inferring timeout intent from error
    /// text. Stateless clients may keep the default.
    fn with_active_generation_timeout(
        &self,
        _timeout: Duration,
    ) -> Option<std::sync::Arc<dyn LlmClient>> {
        None
    }

    /// Complete a conversation (non-streaming)
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse>;

    /// Complete a conversation with streaming
    /// Returns a receiver for streaming events.
    /// The cancel_token is checked during the HTTP request; if cancelled, the request is aborted.
    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>>;

    /// Report the strongest provider-native structured-output enforcement this
    /// client supports. Used by [`structured`] to decide whether to force a
    /// tool call, request a native `response_format`, or fall back to
    /// prompt-and-parse. Defaults to no native support.
    fn native_structured_support(&self) -> structured::NativeStructuredSupport {
        structured::NativeStructuredSupport::None
    }

    /// Report whether [`LlmClient::complete_structured`] uses a transport that
    /// is independent from the streaming implementation.
    ///
    /// The conservative default is false because several account-backed
    /// clients implement `complete` by opening a stream and waiting for its
    /// terminal event. Composite reliability layers use this capability to
    /// avoid presenting the same streaming failure mode as a non-streaming
    /// fallback.
    fn has_distinct_non_streaming_transport(&self) -> bool {
        false
    }

    /// Complete a conversation while honoring a structured-output directive
    /// (forced `tool_choice` and/or native `response_format`).
    ///
    /// The default implementation ignores the directive and behaves exactly
    /// like [`LlmClient::complete`], so existing clients keep working unchanged;
    /// providers that support native structured output override this.
    async fn complete_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        _directive: &structured::StructuredDirective,
    ) -> Result<LlmResponse> {
        self.complete(messages, system, tools).await
    }

    /// Streaming counterpart of [`LlmClient::complete_structured`]. Defaults to
    /// [`LlmClient::complete_streaming`], ignoring the directive.
    async fn complete_streaming_structured(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
        _directive: &structured::StructuredDirective,
        cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.complete_streaming(messages, system, tools, cancel_token)
            .await
    }
}

// Include test modules — these reference internal types via crate paths
#[cfg(test)]
#[path = "tests.rs"]
mod tests_file;
