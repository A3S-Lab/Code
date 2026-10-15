//! Fact-spine memory telemetry contract.
//!
//! `MemoryRecalled` / `MemoriesSearched` must reach the host event channel
//! on the production fact-stream path, not only on the legacy loop path:
//! the fact actor's model-context assembly used to pass `&None` as the
//! event sender, so hosts could not surface recall telemetry even though
//! the recalled memory still reached the model prompt.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_code_core::permissions::PermissionPolicy;
use a3s_code_core::{Agent, AgentEvent, SessionOptions};
use a3s_memory::{InMemoryStore, MemoryItem, MemoryType};
use anyhow::Result;
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

const TEST_TIMEOUT: Duration = Duration::from_secs(20);

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("fixture/controlled".into()),
        providers: vec![ProviderConfig {
            name: "fixture".into(),
            api_key: Some("offline".into()),
            base_url: None,
            headers: HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "controlled".into(),
                name: "Controlled".into(),
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

struct PlainResponseClient;

fn acknowledged() -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Text {
                text: "acknowledged".into(),
            }],
            reasoning_content: None,
            transcript_text: None,
            transcript_visibility: Default::default(),
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

#[async_trait]
impl LlmClient for PlainResponseClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
    ) -> Result<LlmResponse> {
        Ok(acknowledged())
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
        _cancellation: CancellationToken,
    ) -> Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.send(StreamEvent::Done(acknowledged())).await.ok();
        Ok(rx)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fact_stream_surfaces_memory_recall_events_to_the_host() {
    let workspace = tempfile::tempdir().expect("workspace");
    let store = Arc::new(InMemoryStore::new());
    a3s_memory::MemoryStore::store(
        store.as_ref(),
        MemoryItem::new("The release verification codename for status reports is TIDELOCK-551.")
            .with_type(MemoryType::Semantic)
            .with_importance(0.95),
    )
    .await
    .expect("seed memory");

    let agent = Agent::from_config(offline_config()).await.expect("agent");
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("fact-memory-events")
                    .with_llm_client(Arc::new(PlainResponseClient))
                    .with_memory(store)
                    .with_permission_policy(PermissionPolicy::new().allow("ls(*)"))
                    .with_planning(false)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_continuation(false)
                    .with_max_tool_rounds(1),
            ),
        )
        .await
        .expect("session");

    let (mut events, worker) = session
        .stream(
            "For the status report: what is the release verification codename?",
            None,
        )
        .await
        .expect("start fact stream");

    let mut recalled_contents = Vec::new();
    let mut searched = None;
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            match events.recv().await.expect("event stream open") {
                AgentEvent::MemoryRecalled { content, .. } => {
                    recalled_contents.push(content);
                }
                AgentEvent::MemoriesSearched { result_count, .. } => {
                    searched = Some(result_count);
                }
                AgentEvent::End { .. } => break,
                AgentEvent::Error { message } => panic!("fact turn failed: {message}"),
                _ => {}
            }
        }
    })
    .await
    .expect("fact turn timed out");
    worker.await.expect("worker joins");
    session.close().await;

    assert!(
        recalled_contents
            .iter()
            .any(|content| content.contains("TIDELOCK-551")),
        "the fact spine must surface MemoryRecalled to the host: {recalled_contents:?}"
    );
    assert_eq!(
        searched,
        Some(1),
        "the MemoriesSearched telemetry must accompany the recall"
    );
}
