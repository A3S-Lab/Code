use super::Fixture;
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_memory::repository::MemoryStatus;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub(super) struct Observation {
    pub(super) query_id: String,
    pub(super) target_visible: bool,
    pub(super) context_nodes: usize,
    pub(super) forbidden_visible: bool,
}

#[derive(Clone)]
pub(super) struct InspectingClient {
    expected: Arc<HashMap<String, (String, String)>>,
    active_contents: Arc<Vec<String>>,
    forbidden_contents: Arc<Vec<String>>,
    observations: Arc<Mutex<Vec<Observation>>>,
}

impl InspectingClient {
    pub(super) fn new(fixture: &Fixture) -> Self {
        let content_by_id = fixture
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node.content.as_str()))
            .collect::<HashMap<_, _>>();
        let expected = fixture
            .queries
            .iter()
            .map(|query| {
                (
                    query.query.clone(),
                    (
                        query.id.clone(),
                        content_by_id
                            .get(query.relevant_node_id.as_str())
                            .expect("relevant multilingual node must exist")
                            .to_string(),
                    ),
                )
            })
            .collect();
        Self {
            expected: Arc::new(expected),
            active_contents: Arc::new(
                fixture
                    .nodes
                    .iter()
                    .filter(|node| node.status == MemoryStatus::Active)
                    .map(|node| node.content.clone())
                    .collect(),
            ),
            forbidden_contents: Arc::new(
                fixture
                    .nodes
                    .iter()
                    .filter(|node| node.status != MemoryStatus::Active)
                    .map(|node| node.content.clone())
                    .chain(std::iter::once(fixture.foreign_node.content.clone()))
                    .collect(),
            ),
            observations: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn observations(&self) -> Vec<Observation> {
        self.observations.lock().unwrap().clone()
    }

    fn respond(&self, messages: &[Message], system: Option<&str>) -> anyhow::Result<LlmResponse> {
        let query = messages
            .iter()
            .rev()
            .find(|message| message.role == "user")
            .map(Message::text)
            .ok_or_else(|| anyhow::anyhow!("multilingual evaluation has no user query"))?;
        let (query_id, target) = self
            .expected
            .get(&query)
            .ok_or_else(|| anyhow::anyhow!("unknown multilingual evaluation query: {query}"))?;
        let system = system.unwrap_or_default();
        let target_visible = system.contains(target);
        let context_nodes = self
            .active_contents
            .iter()
            .filter(|content| system.contains(content.as_str()))
            .count();
        let forbidden_visible = self
            .forbidden_contents
            .iter()
            .any(|content| system.contains(content.as_str()));
        self.observations.lock().unwrap().push(Observation {
            query_id: query_id.clone(),
            target_visible,
            context_nodes,
            forbidden_visible,
        });
        let output = if target_visible && !forbidden_visible {
            format!("PASS:{query_id}")
        } else {
            format!("MISS:{query_id}")
        };
        Ok(response(output))
    }
}

#[async_trait]
impl LlmClient for InspectingClient {
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
        let (sender, receiver) = mpsc::channel(2);
        tokio::spawn(async move {
            let _ = sender.send(StreamEvent::TextDelta(response.text())).await;
            let _ = sender.send(StreamEvent::Done(response)).await;
        });
        Ok(receiver)
    }
}

fn response(text: String) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Text { text }],
            reasoning_content: None,
        },
        usage: TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("end_turn".into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}
