//! WC-BG1 hermetic twin: the Run that starts a background task receives its
//! output as facts, keeps the tool protocol adjacent, and settles the task
//! before the turn's successful return — session-level, no LLM. The live
//! twin is `tests/test_background_run_real_llm.rs`. This fixture uses a
//! deterministic extended sandbox and proves the generic protocol; it does
//! not qualify native macOS background containment, which is unsupported.

#![cfg(test)]

use super::*;
use crate::tools::builtin::background::test_backend::DeterministicBackgroundSandbox;
use std::sync::Mutex;

struct BackgroundTwinClient {
    calls: std::sync::atomic::AtomicUsize,
    /// Call index that produced the final text answer (memory extraction
    /// appends one more captured request after it).
    answer_call: std::sync::atomic::AtomicUsize,
    seen_requests: Mutex<Vec<Vec<Message>>>,
}

const TWIN_MARK: &str = "BG1_TWIN_MARK_7f3a";

#[async_trait::async_trait]
impl crate::llm::LlmClient for BackgroundTwinClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.seen_requests.lock().unwrap().push(messages.to_vec());
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // The first scripted-answer call (>= 3) wins; later calls — notably
        // the post-turn memory-extraction call — must not overwrite it.
        if call >= 3 {
            let _ = self.answer_call.compare_exchange(
                usize::MAX,
                call,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            );
        }
        let message = match call {
            0 => Message {
                role: "assistant".into(),
                content: vec![crate::llm::ContentBlock::ToolUse {
                    id: "bg-1".into(),
                    name: "background".into(),
                    input: serde_json::json!({
                        "action": "start",
                        "name": "watch",
                        "command": format!("echo {TWIN_MARK}"),
                    }),
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            },
            1 => Message {
                role: "assistant".into(),
                content: vec![crate::llm::ContentBlock::ToolUse {
                    id: "bg-2".into(),
                    name: "read".into(),
                    input: serde_json::json!({ "path": "README.md" }),
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            },
            2 => Message {
                role: "assistant".into(),
                content: vec![crate::llm::ContentBlock::ToolUse {
                    id: "bg-3".into(),
                    name: "background".into(),
                    input: serde_json::json!({
                        "action": "tail",
                        "name": "watch",
                        "wait_for": TWIN_MARK,
                        "timeout_ms": 10_000,
                    }),
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            },
            _ => Message::assistant(format!("{TWIN_MARK} observed").as_str()),
        };
        Ok(LlmResponse {
            message,
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
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
        anyhow::bail!("streaming not used by this twin")
    }
}

#[tokio::test]
async fn the_run_receives_background_output_and_settles_the_task() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path().to_path_buf();
    std::fs::write(root.join("README.md"), "background settlement twin\n").unwrap();

    let client = Arc::new(BackgroundTwinClient {
        calls: std::sync::atomic::AtomicUsize::new(0),
        answer_call: std::sync::atomic::AtomicUsize::new(usize::MAX),
        seen_requests: Mutex::new(Vec::new()),
    });
    let sandbox = Arc::new(DeterministicBackgroundSandbox::new(
        root.clone(),
        TWIN_MARK,
        0,
        false,
    ));
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().into_owned(),
            Arc::clone(&client) as Arc<dyn crate::llm::LlmClient>,
            &SessionOptions::new()
                .with_session_id("background-settlement-twin")
                .with_sandbox_handle(Arc::clone(&sandbox) as Arc<dyn crate::sandbox::BashSandbox>)
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_manual_delegation_enabled(false),
        )
        .unwrap();

    let result = session
        .send(
            "Start the background watch, read the README, tail until the marker, then report.",
            None,
        )
        .await
        .expect("fact turn");
    assert!(result.text.contains(TWIN_MARK), "answer names the marker");

    // 1. The arrivals are facts on the one log, with the marker and the
    //    final disposition.
    let facts = crate::fact_control::read_workspace_facts(
        &root,
        &crate::fact_control::thread_for_session("background-settlement-twin"),
    )
    .unwrap();
    let marker_fact = facts
        .iter()
        .filter(|fact| fact.kind == "background.output")
        .find(|fact| {
            fact.payload
                .get("line")
                .and_then(|value| value.as_str())
                .is_some_and(|line| line.contains(TWIN_MARK))
        });
    assert!(marker_fact.is_some(), "marker landed as a fact");
    assert_eq!(sandbox.calls(), 1);
    assert!(sandbox.was_released());
    let terminal = facts
        .iter()
        .find(|fact| fact.kind == "background.output" && fact.payload["event"] == "exited")
        .unwrap();
    assert_eq!(terminal.payload["disposition"], "completed");
    assert_eq!(terminal.payload["process_settled"], true);
    assert_eq!(terminal.payload["effects"]["completeness"], "complete");
    assert!(
        facts
            .iter()
            .filter(|fact| fact.kind == "background.output")
            .any(|fact| {
                fact.payload.get("event").and_then(|value| value.as_str()) == Some("exited")
            }),
        "settlement landed the exited fact before send returned"
    );

    // 2. Arrival-reactive: the model input of the final call carries the
    //    rendered arrival — the answer turn saw the marker.
    let requests = client.seen_requests.lock().unwrap().clone();
    for (index, request) in requests.iter().enumerate() {
        eprintln!("== request {index} ==");
        for message in request {
            eprintln!("  [{}] {}", message.role, message.text());
        }
    }
    let answer_call = client.answer_call.load(std::sync::atomic::Ordering::SeqCst);
    let final_input = requests
        .get(answer_call)
        .expect("answer-turn model call captured");
    assert!(
        final_input.iter().any(|message| message.role == "user"
            && message.text().contains("[background:watch]")
            && message.text().contains(TWIN_MARK)),
        "the arrival reached the model as input on a later turn"
    );

    // 3. Protocol adjacency: no background line ever sits between an
    //    assistant tool call and its tool result.
    for (index, request) in requests.iter().enumerate() {
        for (position, message) in request.iter().enumerate() {
            let has_tool_use = message.role == "assistant"
                && message
                    .content
                    .iter()
                    .any(|block| matches!(block, crate::llm::ContentBlock::ToolUse { .. }));
            if !has_tool_use {
                continue;
            }
            let Some(next) = request.get(position + 1) else {
                continue;
            };
            let is_tool_result = next
                .content
                .iter()
                .any(|block| matches!(block, crate::llm::ContentBlock::ToolResult { .. }));
            let is_next_tool_use = next.role == "assistant"
                && next
                    .content
                    .iter()
                    .any(|block| matches!(block, crate::llm::ContentBlock::ToolUse { .. }));
            assert!(
                is_tool_result || is_next_tool_use,
                "request {index} position {position}: a tool call must be followed by its result or the next call, not an interleaved arrival"
            );
        }
    }
}
