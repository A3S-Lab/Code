//! S-CV-01: repeated turns must drop expired secrets from history and the
//! durable session snapshot. The fixture never echoes the prompt, so a
//! surviving secret is a kernel retention bug, not model wording.

use crate::compaction::KEEP_RECENT_MESSAGES;
use crate::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition};
use crate::memory::MemoryConfig;
use crate::permissions::{PermissionDecision, PermissionPolicy};
use crate::prompts::PlanningMode;
use crate::{Agent, SessionOptions};
use async_trait::async_trait;
use std::path::Path;
use tokio::sync::mpsc;

struct AckClient;

fn ack() -> LlmResponse {
    LlmResponse {
        message: Message::assistant("ack"),
        usage: TokenUsage {
            prompt_tokens: 8,
            completion_tokens: 1,
            total_tokens: 9,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("end_turn".to_string()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

#[async_trait]
impl LlmClient for AckClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(ack())
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (tx, rx) = mpsc::channel(4);
        let response = ack();
        tokio::spawn(async move {
            let _ = tx.send(StreamEvent::Done(response)).await;
        });
        Ok(rx)
    }
}

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        memory: Some(MemoryConfig {
            llm_extraction: false,
            ..MemoryConfig::default()
        }),
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("offline-key".to_string()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude".to_string(),
                family: "claude".to_string(),
                api_key: None,
                base_url: None,
                headers: std::collections::HashMap::new(),
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
        ..CodeConfig::default()
    }
}

fn snapshot_bytes(root: &Path) -> (u64, bool) {
    let mut bytes = 0_u64;
    let mut holds_expired = false;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            bytes = bytes.saturating_add(metadata.len());
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    holds_expired |= text.contains("SECRET-0001");
                }
            }
        }
    }
    (bytes, holds_expired)
}

#[tokio::test]
#[ignore = "S-CV-01 soak: 100 turns with compaction must drop the first secret"]
async fn soak_compaction_drops_expired_secrets() {
    let store = tempfile::tempdir().expect("store");
    let workspace = tempfile::tempdir().expect("workspace");
    let agent = Agent::from_config(offline_config()).await.expect("agent");
    let mut policy = PermissionPolicy::new();
    policy.default_decision = PermissionDecision::Allow;
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("s-cv-01")
                    .with_model("anthropic/claude-sonnet-4-20250514")
                    .with_llm_client(std::sync::Arc::new(AckClient))
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_continuation(false)
                    .with_auto_compact(true)
                    .with_auto_compact_threshold(0.2)
                    .with_max_context_tokens(4_000)
                    .with_file_session_store(store.path())
                    .without_workspace_retrieval()
                    .with_permission_policy(policy),
            ),
        )
        .await
        .expect("session");

    let secret_body = "x".repeat(2_000);
    for turn in 0..100 {
        let prompt = format!("SECRET-{turn:04}-{secret_body}");
        session
            .send(&prompt, None)
            .await
            .unwrap_or_else(|error| panic!("turn {turn} failed: {error}"));
    }

    let history = session.history();
    let rendered = history
        .iter()
        .map(Message::text)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered.contains("SECRET-0001"),
        "turn 1 secret survived compaction; history_len={}",
        history.len()
    );
    assert!(
        history.len() <= KEEP_RECENT_MESSAGES + 4,
        "history length {} exceeds the compaction window",
        history.len()
    );

    let (bytes, holds_expired) = snapshot_bytes(store.path());
    assert!(!holds_expired, "session snapshot still stores SECRET-0001");
    let bound = (KEEP_RECENT_MESSAGES as u64 + 8) * (secret_body.len() as u64 + 64) * 6;
    assert!(
        bytes <= bound,
        "session snapshot {bytes} bytes exceeds window bound {bound}"
    );
}
