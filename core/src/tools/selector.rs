//! Per-turn tool selection for LLM context.

use crate::llm::{ContentBlock, Message, ToolDefinition};

/// Closed set of standalone conversational acknowledgements (exact match
/// after whitespace/punctuation normalization). Frozen under constitution
/// XV: exact-string bookkeeping on a fixed greeting vocabulary — the list
/// does NOT grow to cover a missed phrasing, and the failure direction
/// (an unmatched greeting) simply retains the full tool surface.
const STANDALONE_CONVERSATION: &[&str] = &[
    "hi",
    "hi there",
    "hello",
    "hello there",
    "hey",
    "greetings",
    "good morning",
    "good afternoon",
    "good evening",
    "how are you",
    "how's it going",
    "hows it going",
    "what's up",
    "whats up",
    "thanks",
    "thank you",
    "你好",
    "您好",
    "嗨",
    "哈喽",
    "哈啰",
    "早",
    "早上好",
    "上午好",
    "下午好",
    "晚上好",
    "在吗",
    "你好吗",
    "谢谢",
    "多谢",
];

/// Select the tools that should be exposed to the model for this turn.
///
/// The executor still owns every registered tool. This function only trims the
/// tool definitions sent to the LLM, keeping routine turns small while allowing
/// specialized tools when the prompt asks for them.
pub fn select_tools_for_messages(
    tools: &[ToolDefinition],
    messages: &[Message],
) -> Vec<ToolDefinition> {
    let context = selection_context(messages);
    select_tools_for_prompt(tools, &context)
}

pub fn select_tools_for_prompt(tools: &[ToolDefinition], prompt: &str) -> Vec<ToolDefinition> {
    // Constitution XV (meaning is not matched): specialized tools are never
    // hidden behind a vocabulary classifier. The previous gate included
    // web/git/batch/program/mcp tools only when English+Chinese term lists
    // matched the prompt — a missed phrasing made the tool INVISIBLE to
    // the model, an unrecoverable miss, and the lists had already grown
    // (the Chinese terms were exactly the growth XV forbids). Every
    // registered tool is now visible on every turn; a host that needs a
    // smaller surface declares it as a typed contract (presentation
    // profile mode), never by classifying the prompt's meaning.
    if is_standalone_conversation(prompt) {
        return Vec::new();
    }
    tools.to_vec()
}

/// Return whether a prompt is only a short conversational acknowledgement.
///
/// This is deliberately exact after whitespace and terminal-punctuation
/// normalization. A greeting that also contains an action must retain the
/// ordinary tool surface.
pub(crate) fn is_standalone_conversation(prompt: &str) -> bool {
    let normalized = prompt
        .trim()
        .trim_matches(is_conversational_boundary)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();

    STANDALONE_CONVERSATION.contains(&normalized.as_str())
}

fn is_conversational_boundary(character: char) -> bool {
    character.is_ascii_punctuation()
        || character.is_whitespace()
        || matches!(
            character,
            '。' | '，' | '、' | '！' | '？' | '…' | '～' | '👋'
        )
}

fn selection_context(messages: &[Message]) -> String {
    let mut parts = messages
        .iter()
        .rev()
        .filter_map(|message| {
            if message.role == "tool" {
                return None;
            }
            let text = message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        })
        .take(6)
        .collect::<Vec<_>>();
    parts.reverse();
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs(names: &[&str]) -> Vec<ToolDefinition> {
        names
            .iter()
            .map(|name| ToolDefinition {
                name: (*name).to_string(),
                description: String::new(),
                parameters: serde_json::json!({"type": "object"}),
            })
            .collect()
    }

    fn names(selected: &[ToolDefinition]) -> Vec<&str> {
        selected.iter().map(|tool| tool.name.as_str()).collect()
    }

    fn sample_defs() -> Vec<ToolDefinition> {
        defs(&[
            "read",
            "write",
            "bash",
            "web_search",
            "web_fetch",
            "download",
            "git",
            "batch",
            "program",
            "mcp__github__create_issue",
            "mcp__browser__browser_navigate",
        ])
    }

    #[test]
    fn every_registered_tool_is_visible_on_every_turn() {
        // Constitution XV: a specialized tool is never hidden behind a
        // vocabulary classifier — a phrasing the old term lists missed
        // made the tool invisible to the model, unrecoverably.
        let selected =
            select_tools_for_prompt(&sample_defs(), "fix the failing test in src/lib.rs");
        let names = names(&selected);
        for expected in [
            "read",
            "write",
            "bash",
            "web_search",
            "git",
            "batch",
            "program",
            "mcp__github__create_issue",
        ] {
            assert!(
                names.contains(&expected),
                "{expected} must be visible: {names:?}"
            );
        }
    }

    #[test]
    fn an_unmatched_phrasing_still_exposes_the_specialized_tools() {
        // The V1 regression: none of these phrasings contain a term the
        // old vocabularies knew ("who won the election" matched nothing),
        // yet the web tools must be selectable.
        for prompt in ["who won the election", "查一下最新动态", "google it"] {
            let selected = select_tools_for_prompt(&sample_defs(), prompt);
            assert!(
                names(&selected).contains(&"web_search"),
                "{prompt:?} must still expose web_search"
            );
        }
    }

    #[test]
    fn standalone_greetings_do_not_expose_tools() {
        for greeting in ["hi", "hello", "你好", "thanks"] {
            assert!(
                select_tools_for_prompt(&sample_defs(), greeting).is_empty(),
                "{greeting:?} is a standalone acknowledgement"
            );
        }
    }

    #[test]
    fn greeting_with_an_action_keeps_the_tools() {
        let selected = select_tools_for_prompt(&sample_defs(), "hello, read the config file");
        assert!(names(&selected).contains(&"read"));
        assert!(!names(&selected).contains(&"hi"));
    }

    #[test]
    fn select_tools_for_messages_uses_the_recent_conversation() {
        let messages = vec![
            Message::user("hello"),
            Message::assistant("hi!"),
            Message::user("read the file"),
        ];
        let selected = select_tools_for_messages(&sample_defs(), &messages);
        assert!(
            names(&selected).contains(&"read"),
            "a conversation ending in an action keeps the tools"
        );
    }
}
