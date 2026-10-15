#![cfg(test)]

use super::*;
use crate::llm::{ContentBlock, ToolResultContentField};

fn fact(kind: &str, payload: serde_json::Value) -> a3s_effect::Fact {
    a3s_effect::Fact {
        seq: 1,
        kind: kind.into(),
        key: String::new(),
        cause: None,
        payload,
    }
}

#[test]
fn checkpoint_keeps_one_batch_and_pairs_results_by_recorded_identity() {
    let facts = vec![
        fact(
            "model.turn",
            serde_json::json!({
                "kind":"tool", "call": {
                    "id":"first", "name":"read", "args":{"path":"a"},
                    "needs_confirmation":false, "text":"Inspect both", "reasoning":"recorded reasoning"
                }, "also":[{
                    "id":"second", "name":"read", "args":{"path":"b"},
                    "needs_confirmation":false, "text":"Inspect both", "reasoning":"recorded reasoning"
                }]
            }),
        ),
        fact(
            "tool.result",
            serde_json::json!({"toolCallId":"second", "output":"B", "ok":false}),
        ),
        fact(
            "tool.result",
            serde_json::json!({"toolCallId":"first", "output":"A", "ok":true}),
        ),
    ];
    let messages = checkpoint_messages_from_facts(&facts);
    assert_eq!(messages.len(), 3);
    assert_eq!(
        messages[0].reasoning_content.as_deref(),
        Some("recorded reasoning")
    );
    assert_eq!(messages[0].content.len(), 3);
    assert!(matches!(&messages[0].content[0], ContentBlock::Text {text} if text == "Inspect both"));
    assert!(
        matches!(&messages[0].content[1], ContentBlock::ToolUse {id, input, ..} if id == "first" && input == &serde_json::json!({"path":"a"}))
    );
    assert!(
        matches!(&messages[0].content[2], ContentBlock::ToolUse {id, input, ..} if id == "second" && input == &serde_json::json!({"path":"b"}))
    );
    assert!(
        matches!(&messages[1].content[0], ContentBlock::ToolResult {tool_use_id, content, is_error, ..} if tool_use_id == "second" && content.as_text() == "B" && *is_error == Some(true))
    );
    assert!(
        matches!(&messages[2].content[0], ContentBlock::ToolResult {tool_use_id, content, ..} if tool_use_id == "first" && content.as_text() == "A")
    );
}

#[test]
fn checkpoint_preserves_user_and_tool_image_bytes() {
    let image = crate::llm::Attachment::new(vec![1, 2, 3], "image/png");
    let user = user_message_fact(
        "user:1".into(),
        "Inspect image",
        std::slice::from_ref(&image),
    );
    let output = fact_tool_runner_value("Image result", true, &[image]);
    let messages = checkpoint_messages_from_facts(&[
        fact(&user.kind, user.payload),
        fact(
            "tool.result",
            serde_json::json!({"toolCallId":"image-call", "output":output, "ok":true}),
        ),
    ]);
    assert!(messages[0]
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::Image {source} if source.data == "AQID")));
    assert!(
        matches!(&messages[1].content[0], ContentBlock::ToolResult {tool_use_id, content: ToolResultContentField::Blocks(blocks), is_error, ..} if tool_use_id == "image-call" && *is_error == Some(true) && blocks.iter().any(|block| matches!(block, crate::llm::ToolResultContent::Image {source} if source.data == "AQID")))
    );
}

#[test]
fn checkpoint_starts_at_last_compaction_and_does_not_invent_result_ids() {
    let messages = checkpoint_messages_from_facts(&[
        fact("user.message", serde_json::json!({"text":"discarded"})),
        fact(
            "compaction.done",
            serde_json::json!({"summary":"original summary", "kept":["user\nkept"]}),
        ),
        fact(
            "tool.result",
            serde_json::json!({"output":"no identity", "ok":true}),
        ),
        fact("user.message", serde_json::json!({"text":"new message"})),
    ]);
    assert_eq!(messages.len(), 3);
    assert!(messages[0].text().ends_with("original summary"));
    assert_eq!(messages[1].text(), "kept");
    assert_eq!(messages[2].text(), "new message");
}

#[test]
fn checkpoint_omits_unexecuted_batch_calls_without_inventing_results() {
    let messages = checkpoint_messages_from_facts(&[
        fact(
            "model.turn",
            serde_json::json!({"kind":"tool", "call":{"id":"denied", "name":"write", "args":{}, "needs_confirmation":true}, "also":[{"id":"done", "name":"read", "args":{}, "needs_confirmation":false}, {"id":"pending", "name":"write", "args":{}, "needs_confirmation":false}]}),
        ),
        fact(
            "confirmation.answered",
            serde_json::json!({"tool_call_id":"denied", "approved":false}),
        ),
        fact(
            "tool.result",
            serde_json::json!({"toolCallId":"done", "output":"known failure", "ok":false}),
        ),
        fact(
            "tool.prepared",
            serde_json::json!({"identity":{"tool_call_id":"pending"}}),
        ),
    ]);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content.len(), 1);
    assert!(matches!(&messages[0].content[0], ContentBlock::ToolUse {id, ..} if id == "done"));
    assert!(
        matches!(&messages[1].content[0], ContentBlock::ToolResult {tool_use_id, is_error, ..} if tool_use_id == "done" && *is_error == Some(true))
    );
}

#[test]
fn checkpoint_does_not_pair_reused_ids_across_model_turns() {
    let messages = checkpoint_messages_from_facts(&[
        fact(
            "model.turn",
            serde_json::json!({"kind":"tool", "call":{"id":"reused", "name":"read", "args":{"path":"pending"}, "needs_confirmation":false}}),
        ),
        fact(
            "model.turn",
            serde_json::json!({"kind":"tool", "call":{"id":"reused", "name":"read", "args":{"path":"committed"}, "needs_confirmation":false}}),
        ),
        fact(
            "tool.result",
            serde_json::json!({"toolCallId":"reused", "output":"actual output", "ok":true}),
        ),
    ]);
    assert_eq!(messages.len(), 2);
    assert!(
        matches!(&messages[0].content[0], ContentBlock::ToolUse {id, input, ..} if id == "reused" && input == &serde_json::json!({"path":"committed"}))
    );
    assert!(
        matches!(&messages[1].content[0], ContentBlock::ToolResult {content, ..} if content.as_text() == "actual output")
    );
}
