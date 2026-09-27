use std::sync::{Arc, Mutex};

use super::*;

struct RecordingCompactClient {
    reply: String,
    summary: String,
    prompts: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for RecordingCompactClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let rendered = messages
            .iter()
            .map(|message| message.text())
            .collect::<Vec<_>>()
            .join("\n");
        self.prompts.lock().expect("prompts").push(rendered);
        Ok(LlmResponse {
            message: Message::assistant(&self.summary),
            usage: TokenUsage::default(),
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        let reply = self.reply.clone();
        tokio::spawn(async move {
            let _ = tx.send(StreamEvent::TextDelta(reply.clone())).await;
            let _ = tx
                .send(StreamEvent::Done(LlmResponse {
                    message: Message::assistant(&reply),
                    usage: TokenUsage::default(),
                    stop_reason: None,
                    token_logprobs: Vec::new(),
                    meta: None,
                }))
                .await;
        });
        Ok(rx)
    }
}

async fn finish_stream(session: &AgentSession, prompt: &str) {
    let (mut events, worker) = session.stream(prompt, None).await.expect("stream");
    while events.recv().await.is_some() {}
    worker.await.expect("stream worker");
}

#[tokio::test]
async fn compact_conversation_writes_a_summary_fact() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace_path = workspace.path().to_path_buf();
    crate::fact_control::reset_session_fact_log(&workspace_path);
    let client = Arc::new(RecordingCompactClient {
        reply: "noted".into(),
        summary: "## Goal\nship the auth fix\n\n## Current State\nauth middleware updated".into(),
        prompts: Mutex::new(Vec::new()),
    });
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            workspace_path.to_string_lossy().to_string(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("compact-session")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled),
        )
        .unwrap();

    let empty = session.compact_conversation(None).await;
    assert!(empty
        .expect_err("empty session")
        .to_string()
        .contains("Nothing to compact yet"));

    finish_stream(&session, "remember the auth bug in login.rs").await;
    finish_stream(&session, "the fix belongs in the session middleware").await;
    let before = client.prompts.lock().expect("prompts").len();

    session
        .compact_conversation(Some("auth"))
        .await
        .expect("compact");

    let prompts = client.prompts.lock().expect("prompts");
    let focus_prompt = prompts.get(before).expect("compaction called the model");
    assert!(focus_prompt.contains("auth bug"));
    assert!(focus_prompt.contains("Host focus for this compaction"));
    assert!(focus_prompt.contains("auth"));
    drop(prompts);

    let facts = crate::fact_control::read_workspace_facts(
        &workspace_path,
        &crate::fact_control::thread_for_session("compact-session"),
    )
    .unwrap();
    let done = facts.last().expect("compaction fact");
    assert_eq!(done.kind, "compaction.done");
    let summary = done.payload["summary"].as_str().expect("summary");
    assert!(summary.contains("auth middleware updated"));
    assert!(session.history()[0]
        .text()
        .contains("auth middleware updated"));

    let again = session.compact_conversation(None).await;
    assert!(again
        .expect_err("already compacted")
        .to_string()
        .contains("Nothing to compact yet"));
}
