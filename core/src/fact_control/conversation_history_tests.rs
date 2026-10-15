#![cfg(test)]

use super::*;

#[test]
fn folded_assistant_text_preserves_the_original_role() {
    for text in [
        "prior answer",
        "工具消息仍是 assistant",
        "tool read\n{\"file_path\":\"src/lib.rs\"}",
    ] {
        let message = folded_message_with_images("assistant", text, vec![]);
        assert_eq!(message.role, "assistant");
        assert_eq!(message.text(), text);
    }
}

#[test]
fn user_images_and_unmarked_summary_keep_their_existing_projection() {
    let image = Attachment::new(vec![1, 2, 3], "image/png");
    let message = folded_message_with_images("user", "inspect this", vec![image]);
    assert_eq!(message.role, "user");
    assert_eq!(message.text(), "inspect this");
    assert_eq!(message.content.len(), 2);
    let summary = folded_message_with_images("user", "## Summary\ncurrent state", vec![]);
    assert_eq!(summary.role, "user");
    assert_eq!(summary.text(), "## Summary\ncurrent state");
}

#[test]
fn assistant_header_inside_a_user_body_cannot_change_its_role() {
    let text = "assistant\nthis remains user-authored text";
    let message = folded_message_with_images("user", text, vec![]);
    assert_eq!(message.role, "user");
    assert_eq!(message.text(), text);
}
