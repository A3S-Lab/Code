use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub(super) struct ModelObservation {
    pub(super) success: bool,
    pub(super) input_tokens: usize,
    pub(super) output_tokens: usize,
    pub(super) memory_context_tokens: usize,
}

#[derive(Clone)]
pub(super) struct RecallEvalClient {
    expected_by_query: Arc<HashMap<String, (String, String)>>,
    memory_contents: Arc<Vec<String>>,
    observations: Arc<Mutex<Vec<ModelObservation>>>,
}

impl RecallEvalClient {
    pub(super) fn new(
        expected_by_query: HashMap<String, (String, String)>,
        memory_contents: Vec<String>,
    ) -> Self {
        Self {
            expected_by_query: Arc::new(expected_by_query),
            memory_contents: Arc::new(memory_contents),
            observations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn respond(&self, messages: &[Message], system: Option<&str>) -> anyhow::Result<LlmResponse> {
        let query = messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(Message::text)
            .ok_or_else(|| anyhow::anyhow!("evaluation request has no user query"))?;
        let (query_id, expected_content) = self
            .expected_by_query
            .get(&query)
            .ok_or_else(|| anyhow::anyhow!("unknown evaluation query: {query}"))?;
        let system = system.unwrap_or_default();
        let success = system.contains(expected_content);
        let output = if success {
            format!("PASS:{query_id}")
        } else {
            format!("MISS:{query_id}")
        };
        let input_tokens = model_input_tokens(messages, system);
        let output_tokens = token_estimate(&output);
        let memory_context_tokens = self
            .memory_contents
            .iter()
            .filter(|content| system.contains(content.as_str()))
            .map(|content| token_estimate(content))
            .sum();
        self.observations.lock().unwrap().push(ModelObservation {
            success,
            input_tokens,
            output_tokens,
            memory_context_tokens,
        });
        Ok(model_response(output, input_tokens, output_tokens))
    }

    pub(super) fn observations(&self) -> Vec<ModelObservation> {
        self.observations.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmClient for RecallEvalClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.respond(messages, system)
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let response = self.respond(messages, system)?;
        Ok(stream_response(response))
    }
}

#[derive(Clone, Debug)]
pub(super) struct CaptureCall {
    pub(super) input_tokens: usize,
    pub(super) output_tokens: usize,
}

#[derive(Clone)]
pub(super) struct CaptureEvalClient {
    main_response: Arc<str>,
    extraction_response: Arc<str>,
    extraction_prompt: Arc<Mutex<Option<String>>>,
    calls: Arc<Mutex<Vec<CaptureCall>>>,
}

impl CaptureEvalClient {
    pub(super) fn new(main_response: &str, extraction_response: String) -> Self {
        Self {
            main_response: Arc::from(main_response),
            extraction_response: Arc::from(extraction_response),
            extraction_prompt: Arc::new(Mutex::new(None)),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn respond(&self, messages: &[Message], system: Option<&str>) -> LlmResponse {
        let extraction = system.is_some_and(|value| {
            value.contains("You extract durable, reusable memory for a coding agent")
        });
        let output = if extraction {
            let prompt = messages.first().map(Message::text).unwrap_or_default();
            *self.extraction_prompt.lock().unwrap() = Some(prompt);
            self.extraction_response.to_string()
        } else {
            self.main_response.to_string()
        };
        let input_tokens = model_input_tokens(messages, system.unwrap_or_default());
        let output_tokens = token_estimate(&output);
        self.calls.lock().unwrap().push(CaptureCall {
            input_tokens,
            output_tokens,
        });
        model_response(output, input_tokens, output_tokens)
    }

    pub(super) fn extraction_prompt(&self) -> Option<String> {
        self.extraction_prompt.lock().unwrap().clone()
    }

    pub(super) fn calls(&self) -> Vec<CaptureCall> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl LlmClient for CaptureEvalClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(self.respond(messages, system))
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        Ok(stream_response(self.respond(messages, system)))
    }
}

fn model_response(text: String, input_tokens: usize, output_tokens: usize) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Text { text }],
            reasoning_content: None,
        },
        usage: TokenUsage {
            prompt_tokens: input_tokens,
            completion_tokens: output_tokens,
            total_tokens: input_tokens + output_tokens,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("end_turn".into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn stream_response(response: LlmResponse) -> mpsc::Receiver<StreamEvent> {
    let (sender, receiver) = mpsc::channel(4);
    tokio::spawn(async move {
        let text = response.text();
        if !text.is_empty() {
            let _ = sender.send(StreamEvent::TextDelta(text)).await;
        }
        let _ = sender.send(StreamEvent::Done(response)).await;
    });
    receiver
}

fn token_estimate(value: &str) -> usize {
    value.chars().count().div_ceil(4).max(1)
}

fn model_input_tokens(messages: &[Message], system: &str) -> usize {
    token_estimate(system)
        + messages
            .iter()
            .map(|message| token_estimate(&message.text()))
            .sum::<usize>()
}
