//! A `session.stream` run (the FactRun turn engine) emits one OpenTelemetry
//! GenAI `chat` client span per model call, with model, provider, usage, and
//! finish reason, and without prompt or completion text. Auxiliary calls
//! outside the turn engines (here the memory extraction `complete`) emit no
//! span.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{
    ContentBlock, LlmClient, LlmResponse, LlmResponseMeta, Message, StreamEvent, TokenUsage,
    ToolDefinition,
};
use a3s_code_core::{Agent, AgentEvent, PlanningMode, SessionOptions};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
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

/// The FactRun worker runs on pool threads, so the capture is process-global.
fn closed_chat_spans() -> &'static Closed {
    static CLOSED: OnceLock<Closed> = OnceLock::new();
    CLOSED.get_or_init(|| {
        let closed = Closed::default();
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(ChatSpans(Arc::clone(&closed))),
        )
        .expect("global subscriber");
        closed
    })
}

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("fixture/model".into()),
        providers: vec![ProviderConfig {
            name: "fixture".into(),
            api_key: Some("offline".into()),
            base_url: None,
            headers: HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "model".into(),
                name: "Model".into(),
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

#[derive(Default)]
struct AnswerClient {
    turn_calls: AtomicUsize,
}

fn answer() -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Text {
                text: "SECRET ANSWER".into(),
            }],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: Default::default(),
        },
        usage: TokenUsage {
            prompt_tokens: 11,
            completion_tokens: 7,
            total_tokens: 18,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("end_turn".into()),
        token_logprobs: Vec::new(),
        meta: Some(LlmResponseMeta {
            provider: Some("fixture".into()),
            request_model: Some("model".into()),
            response_model: Some("model-2026".into()),
            response_id: Some("resp-7".into()),
            ..Default::default()
        }),
    }
}

#[async_trait]
impl LlmClient for AnswerClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        Ok(answer())
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        self.turn_calls.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = tx.send(StreamEvent::Done(answer())).await;
        });
        Ok(rx)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_stream_emits_one_gen_ai_chat_span_per_model_call() {
    let closed = closed_chat_spans();
    let workspace = tempfile::tempdir().unwrap();
    let client = Arc::new(AnswerClient::default());
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let options = SessionOptions::new()
        .with_llm_client(client.clone())
        .with_planning_mode(PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false);
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .unwrap();

    let (mut events, worker) = session.stream("SECRET PROMPT", None).await.unwrap();
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(20), async {
        while let Some(event) = events.recv().await {
            if let AgentEvent::End { text: end, .. } = event {
                text = end;
            }
        }
    })
    .await
    .expect("run finished");
    session.close().await;
    worker.abort();

    assert!(text.contains("SECRET ANSWER"), "{text}");
    let turn_calls = client.turn_calls.load(Ordering::SeqCst);
    let spans = closed.lock().unwrap().clone();
    assert_eq!(turn_calls, 1);
    assert_eq!(spans.len(), turn_calls, "{spans:?}");
    let span = &spans[0];
    assert_eq!(span["otel.name"], "chat model");
    assert_eq!(span["otel.kind"], "client");
    assert_eq!(span["gen_ai.operation.name"], "chat");
    assert_eq!(span["gen_ai.provider.name"], "fixture");
    assert_eq!(span["gen_ai.request.model"], "model");
    assert_eq!(span["gen_ai.response.model"], "model-2026");
    assert_eq!(span["gen_ai.response.id"], "resp-7");
    assert_eq!(span["gen_ai.response.finish_reasons"], "end_turn");
    assert_eq!(span["gen_ai.usage.input_tokens"], "11");
    assert_eq!(span["gen_ai.usage.output_tokens"], "7");
    assert!(!span.contains_key("error.type"), "{span:?}");
    for span in &spans {
        assert!(
            span.values().all(|value| !value.contains("SECRET")),
            "message text leaked into span attributes: {span:?}"
        );
    }
}
