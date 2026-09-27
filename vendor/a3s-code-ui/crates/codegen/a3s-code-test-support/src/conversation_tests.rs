use serde_json::json;

use super::ConversationKey;

#[test]
fn session_id_keys_a_request_ahead_of_the_opening_it_repeats() {
    let cases = [
        (
            Some("session-a"),
            json!({ "messages": [
                { "role": "system", "content": "parent in plan mode" },
                { "role": "user", "content": "hello" }
            ] }),
            ConversationKey::SessionId("session-a".to_owned()),
        ),
        (
            None,
            json!({ "messages": [
                { "role": "system", "content": "parent" },
                { "role": "user", "content": "hello\nchild result" }
            ] }),
            ConversationKey::Opening {
                system_message: "parent".to_owned(),
                tools: Vec::new(),
                opening_turn: Some("hello\nchild result".to_owned()),
            },
        ),
        (
            None,
            json!({ "input": [{ "role": "user", "content": "summary?" }] }),
            ConversationKey::Unkeyed,
        ),
    ];
    for (session_id, body, expected) in cases {
        assert_eq!(
            expected,
            ConversationKey::from_request(session_id, &body),
            "{body}"
        );
    }
}

#[test]
fn child_offered_fewer_tools_keys_apart_from_the_parent_prompt_it_inherited() {
    let parent = json!({ "instructions": "one agent prompt", "tools": [
        { "name": "Read" },
        { "name": "Task" }
    ] });
    let child = json!({ "instructions": "one agent prompt", "tools": [{ "name": "Read" }] });

    assert_ne!(
        ConversationKey::from_request(None, &parent),
        ConversationKey::from_request(None, &child)
    );
}

#[test]
fn two_children_of_one_turn_key_apart_by_the_task_each_was_given() {
    let alpha = json!({ "instructions": "one agent prompt", "messages": [
        { "role": "user", "content": "the same preamble" },
        { "role": "user", "content": "explore the notes" }
    ] });
    let bravo = json!({ "instructions": "one agent prompt", "messages": [
        { "role": "user", "content": "the same preamble" },
        { "role": "user", "content": "summarize the notes" }
    ] });

    assert_ne!(
        ConversationKey::from_request(None, &alpha),
        ConversationKey::from_request(None, &bravo)
    );
}

#[test]
fn later_prompt_stays_in_the_conversation_its_opening_named() {
    let first = json!({ "instructions": "one agent prompt", "messages": [
        { "role": "user", "content": "preamble" },
        { "role": "user", "content": "first task" }
    ] });
    let follow_up = json!({ "instructions": "one agent prompt", "messages": [
        { "role": "user", "content": "preamble" },
        { "role": "user", "content": "first task" },
        { "role": "user", "content": "second task" }
    ] });

    assert_eq!(
        ConversationKey::from_request(None, &first),
        ConversationKey::from_request(None, &follow_up)
    );
}
