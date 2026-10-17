use super::*;
use crate::agent::tests::MockLlmClient;
use crate::agent::{AgentConfig, AgentLoop};
use crate::llm::LlmResponseMeta;
use crate::tools::{ToolContext, ToolExecutor};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::layer::{Context, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

type Closed = Arc<Mutex<Vec<HashMap<String, String>>>>;

#[derive(Default)]
struct Fields(HashMap<String, String>);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().into(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), value.into());
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().into(), value.to_string());
    }
}

/// Collects the fields of every closed `chat` span.
struct ChatSpans(Closed);

impl<S> Layer<S> for ChatSpans
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.metadata().name() != "chat" {
            return;
        }
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(fields);
        }
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            if let Some(fields) = span.extensions_mut().get_mut::<Fields>() {
                values.record(fields);
            }
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            if let Some(fields) = span.extensions_mut().remove::<Fields>() {
                self.0.lock().unwrap().push(fields.0);
            }
        }
    }
}

fn capture() -> (Closed, impl tracing::Subscriber + Send + Sync) {
    let closed = Closed::default();
    let subscriber = tracing_subscriber::registry().with(ChatSpans(Arc::clone(&closed)));
    (closed, subscriber)
}

fn response_with_meta(text: &str) -> LlmResponse {
    let mut response = MockLlmClient::text_response(text);
    response.stop_reason = Some("end_turn".into());
    response.meta = Some(LlmResponseMeta {
        provider: Some("fixture".into()),
        request_model: Some("fixture-model".into()),
        response_model: Some("fixture-model-2026".into()),
        response_id: Some("resp-1".into()),
        ..Default::default()
    });
    response
}

fn assert_chat_attributes(span: &HashMap<String, String>) {
    assert_eq!(span["otel.name"], "chat fixture-model");
    assert_eq!(span["otel.kind"], "client");
    assert_eq!(span["gen_ai.operation.name"], "chat");
    assert_eq!(span["gen_ai.provider.name"], "fixture");
    assert_eq!(span["gen_ai.request.model"], "fixture-model");
    assert_eq!(span["gen_ai.response.model"], "fixture-model-2026");
    assert_eq!(span["gen_ai.response.id"], "resp-1");
    assert_eq!(span["gen_ai.response.finish_reasons"], "end_turn");
    assert_eq!(span["gen_ai.usage.input_tokens"], "10");
    assert_eq!(span["gen_ai.usage.output_tokens"], "5");
    assert_eq!(span["a3s.llm.total_tokens"], "15");
    assert_eq!(span["a3s.llm.model"], "fixture-model");
    assert!(!span.contains_key("error.type"), "{span:?}");
}

#[test]
fn chat_span_records_semconv_attributes_without_message_text() {
    let (closed, subscriber) = capture();
    tracing::subscriber::with_default(subscriber, || {
        let span = chat_span();
        record_chat_outcome::<()>(&span, &Ok(response_with_meta("SECRET ANSWER")));
    });
    let spans = closed.lock().unwrap().clone();
    assert_eq!(spans.len(), 1);
    assert_chat_attributes(&spans[0]);
    assert!(
        spans[0].values().all(|value| !value.contains("SECRET")),
        "message text leaked into span attributes: {:?}",
        spans[0]
    );
}

#[test]
fn failed_call_records_error_type() {
    let (closed, subscriber) = capture();
    tracing::subscriber::with_default(subscriber, || {
        let span = chat_span();
        record_chat_outcome(&span, &Err::<LlmResponse, _>("refused"));
    });
    let spans = closed.lock().unwrap().clone();
    assert_eq!(spans[0]["error.type"], ERROR_TYPE_OTHER);
    assert_eq!(spans[0]["otel.status_code"], "ERROR");
}

#[tokio::test(flavor = "current_thread")]
async fn agent_loop_model_call_emits_one_chat_span() {
    let (closed, subscriber) = capture();
    let _guard = tracing::subscriber::set_default(subscriber);
    let root = crate::test_support::hermetic_workspace();
    let client = Arc::new(MockLlmClient::new(vec![response_with_meta(
        "SECRET ANSWER",
    )]));
    let agent = AgentLoop::new(
        client,
        Arc::new(ToolExecutor::new(root.display().to_string())),
        ToolContext::new(root.clone()),
        AgentConfig::default(),
    );
    let result = agent.execute(&[], "SECRET PROMPT", None).await.unwrap();
    assert_eq!(result.text, "SECRET ANSWER");

    let spans = closed.lock().unwrap().clone();
    assert_eq!(spans.len(), 1, "{spans:?}");
    assert_chat_attributes(&spans[0]);
    assert!(spans[0].values().all(|value| !value.contains("SECRET")));
}
