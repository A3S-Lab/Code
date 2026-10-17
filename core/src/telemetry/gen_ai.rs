//! One span per model call, named and attributed per the OpenTelemetry
//! GenAI semantic conventions (`gen_ai.*`), with the `a3s.llm.*` names
//! recorded on the same span.
//!
//! Prompt and completion text never become attributes.

use crate::llm::LlmResponse;
use tracing::field::Empty;

pub(crate) const OPERATION_CHAT: &str = "chat";
/// `error.type` fallback when the failure has no closed-set classification.
pub(crate) const ERROR_TYPE_OTHER: &str = "_OTHER";

/// Open a client span for one model call. Model and provider are recorded
/// from the response, because the session's client does not expose them.
pub(crate) fn chat_span() -> tracing::Span {
    tracing::info_span!(
        "chat",
        otel.name = Empty,
        otel.kind = "client",
        otel.status_code = Empty,
        gen_ai.operation.name = OPERATION_CHAT,
        gen_ai.provider.name = Empty,
        gen_ai.request.model = Empty,
        gen_ai.response.model = Empty,
        gen_ai.response.id = Empty,
        gen_ai.response.finish_reasons = Empty,
        gen_ai.usage.input_tokens = Empty,
        gen_ai.usage.output_tokens = Empty,
        a3s.llm.provider = Empty,
        a3s.llm.model = Empty,
        a3s.llm.prompt_tokens = Empty,
        a3s.llm.completion_tokens = Empty,
        a3s.llm.total_tokens = Empty,
        a3s.llm.stop_reason = Empty,
        error.type = Empty,
    )
}

/// Record a completed model call.
pub(crate) fn record_chat_response(span: &tracing::Span, response: &LlmResponse) {
    if let Some(meta) = &response.meta {
        if let Some(provider) = &meta.provider {
            span.record("gen_ai.provider.name", provider.as_str());
            span.record(super::ATTR_LLM_PROVIDER, provider.as_str());
        }
        if let Some(model) = &meta.request_model {
            span.record("otel.name", format!("{OPERATION_CHAT} {model}"));
            span.record("gen_ai.request.model", model.as_str());
            span.record(super::ATTR_LLM_MODEL, model.as_str());
        }
        if let Some(model) = &meta.response_model {
            span.record("gen_ai.response.model", model.as_str());
        }
        if let Some(id) = &meta.response_id {
            span.record("gen_ai.response.id", id.as_str());
        }
    }
    if let Some(reason) = &response.stop_reason {
        span.record("gen_ai.response.finish_reasons", reason.as_str());
        span.record(super::ATTR_LLM_STOP_REASON, reason.as_str());
    }
    let usage = &response.usage;
    span.record("gen_ai.usage.input_tokens", usage.prompt_tokens as i64);
    span.record("gen_ai.usage.output_tokens", usage.completion_tokens as i64);
    span.record(super::ATTR_LLM_PROMPT_TOKENS, usage.prompt_tokens as i64);
    span.record(
        super::ATTR_LLM_COMPLETION_TOKENS,
        usage.completion_tokens as i64,
    );
    span.record(super::ATTR_LLM_TOTAL_TOKENS, usage.total_tokens as i64);
}

/// Record a failed model call.
pub(crate) fn record_chat_error(span: &tracing::Span) {
    span.record("error.type", ERROR_TYPE_OTHER);
    span.record("otel.status_code", "ERROR");
}

/// Record either outcome of a model call.
pub(crate) fn record_chat_outcome<E>(span: &tracing::Span, outcome: &Result<LlmResponse, E>) {
    match outcome {
        Ok(response) => record_chat_response(span, response),
        Err(_) => record_chat_error(span),
    }
}

#[cfg(test)]
#[path = "gen_ai/tests.rs"]
mod tests;
