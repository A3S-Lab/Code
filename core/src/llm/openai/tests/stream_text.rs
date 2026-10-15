#![cfg(test)]

use super::*;
use crate::llm::{LlmClient, StreamEvent};

async fn observe_frames(chunks: Vec<String>) -> (String, String, LlmResponse) {
    let mut events = glm_client(chunks)
        .complete_streaming(
            &[Message::user("transport fidelity")],
            None,
            &[],
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect("open synthetic stream");
    tokio::time::timeout(std::time::Duration::from_secs(2), async move {
        let mut text = String::new();
        let mut reasoning = String::new();
        let mut done = None;
        while let Some(event) = events.recv().await {
            match event {
                StreamEvent::TextDelta(delta) => text.push_str(&delta),
                StreamEvent::ReasoningDelta(delta) => reasoning.push_str(&delta),
                StreamEvent::Done(response) => {
                    assert!(done.is_none(), "one protocol terminal response");
                    done = Some(response);
                }
                _ => {}
            }
        }
        (text, reasoning, done.expect("synthetic stream completed"))
    })
    .await
    .expect("finite synthetic transport terminates")
}

fn data(value: serde_json::Value) -> String {
    format!("data: {value}\n\n")
}

#[tokio::test]
async fn text_deltas_preserve_repeated_and_prefix_shaped_fragments() {
    // Each delta is new data, even when it equals a suffix or starts with
    // all preceding text. Neither content nor language identifies a replay.
    for (fragments, expected) in [
        (vec!["EXTRACT-LIVE-77", "3", "3"], "EXTRACT-LIVE-7733"),
        (vec!["哈", "哈"], "哈哈"),
        (vec!["a", "ab"], "aab"),
        (vec!["abc", "bc"], "abcbc"),
        (vec![" ", " "], "  "),
        (vec!["\n", "\n"], "\n\n"),
    ] {
        let mut chunks = fragments
            .into_iter()
            .map(|content| data(serde_json::json!({"choices":[{"delta":{"content":content}}]})))
            .collect::<Vec<_>>();
        chunks.push("data: [DONE]\n\n".into());
        let (streamed, reasoning, response) = observe_frames(chunks).await;
        assert_eq!(streamed, expected, "host deltas preserve every fragment");
        assert_eq!(response.text(), expected, "Done preserves the same bytes");
        assert!(reasoning.is_empty());
    }
}

#[tokio::test]
async fn reasoning_deltas_preserve_repetition_in_their_own_channel() {
    let mut chunks = ["试", "试", "读", "读", "33", "3"]
        .into_iter()
        .map(|text| data(serde_json::json!({"choices":[{"delta":{"reasoning_content":text}}]})))
        .collect::<Vec<_>>();
    chunks.push("data: [DONE]\n\n".into());
    let (streamed, reasoning, response) = observe_frames(chunks).await;
    assert_eq!(reasoning, "试试读读333");
    assert_eq!(
        response.message.reasoning_content.as_deref(),
        Some("试试读读333")
    );
    assert!(streamed.is_empty());
    assert!(response.text().is_empty());
}

#[tokio::test]
async fn trailing_json_delta_preserves_repetition_at_clean_eof() {
    let chunks = vec![
        data(serde_json::json!({"choices":[{"delta":{"content":"773","reasoning_content":"读"}}]})),
        serde_json::json!({"choices":[{"delta":{"content":"3","reasoning_content":"读"},"finish_reason":"stop"}]}).to_string(),
    ];
    let (streamed, reasoning, response) = observe_frames(chunks).await;
    assert_eq!(streamed, "7733");
    assert_eq!(response.text(), "7733");
    assert_eq!(reasoning, "读读");
    assert_eq!(response.message.reasoning_content.as_deref(), Some("读读"));
    assert_eq!(response.stop_reason.as_deref(), Some("stop"));
}

#[tokio::test]
async fn explicit_message_snapshot_does_not_replay_preceding_deltas() {
    let chunks = vec![
        data(
            serde_json::json!({"choices":[{"delta":{"content":"hello","reasoning_content":"读"}}]}),
        ),
        data(
            serde_json::json!({"choices":[{"message":{"role":"assistant","content":"hello","reasoning_content":"读"},"finish_reason":"stop"}]}),
        ),
        "data: [DONE]\n\n".into(),
    ];
    let (streamed, reasoning, response) = observe_frames(chunks).await;
    assert_eq!(streamed, "hello");
    assert_eq!(response.text(), "hello");
    assert_eq!(reasoning, "读");
    assert_eq!(response.message.reasoning_content.as_deref(), Some("读"));
}
