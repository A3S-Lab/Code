//! Per-turn tool selection for LLM context.

use crate::llm::{ContentBlock, Message, ToolDefinition};
use std::collections::HashSet;

const CORE_TOOLS: &[&str] = &[
    "read",
    "write",
    "edit",
    "patch",
    "bash",
    "grep",
    "glob",
    "ls",
    "code_symbols",
    "code_navigation",
    "code_diagnostics",
    "task",
    "parallel_task",
    "Skill",
    "search_skills",
];

const WEB_TERMS: &[&str] = &[
    "http", "https", "url", "web", "website", "internet", "search", "browse", "latest", "recent",
    "news", "today", "online", "网页", "网站", "联网", "搜索", "检索", "最新", "新闻", "今天",
];

const FETCH_TERMS: &[&str] = &[
    "http", "https", "url", "fetch", "open", "article", "page", "网页", "网站", "文章", "链接",
    "打开",
];

const GIT_TERMS: &[&str] = &[
    "git", "commit", "branch", "diff", "status", "log", "tag", "release", "push", "pull", "merge",
    "rebase", "github", "提交", "分支", "标签", "发布", "推送",
];

const BATCH_TERMS: &[&str] = &[
    "batch",
    "parallel",
    "concurrent",
    "multiple",
    "fan out",
    "批量",
    "并行",
    "同时",
    "多个",
];

const PROGRAM_TERMS: &[&str] = &[
    "program",
    "programmatic",
    "ptc",
    "repo map",
    "repository map",
    "code search",
    "program_code_search",
    "program_repo_map",
    "程序",
    "仓库地图",
    "代码搜索",
];

const MCP_TERMS: &[&str] = &["mcp", "external tool", "external server", "外部工具"];

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
    if tools.is_empty() || is_standalone_conversation(prompt) {
        return Vec::new();
    }

    let prompt_lower = prompt.to_lowercase();
    let wants_web = contains_any(&prompt_lower, WEB_TERMS);
    let wants_fetch = contains_any(&prompt_lower, FETCH_TERMS);
    let wants_git = contains_any(&prompt_lower, GIT_TERMS);
    let wants_batch = contains_any(&prompt_lower, BATCH_TERMS);
    let wants_program = contains_any(&prompt_lower, PROGRAM_TERMS);
    let wants_mcp = contains_any(&prompt_lower, MCP_TERMS);

    let core: HashSet<&str> = CORE_TOOLS.iter().copied().collect();
    let mut selected = Vec::new();

    for tool in tools {
        let name = tool.name.as_str();
        let name_lower = name.to_lowercase();

        let include = core.contains(name)
            || (name == "web_search" && wants_web)
            || (name == "web_fetch" && (wants_web || wants_fetch))
            || (name == "git" && wants_git)
            || (name == "batch" && wants_batch)
            || (name == "program" && wants_program)
            || should_include_mcp_tool(name, &name_lower, &prompt_lower, wants_mcp)
            || (!is_known_special_tool(name) && !name.starts_with("mcp__"));

        if include {
            selected.push(tool.clone());
        }
    }

    selected
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

fn should_include_mcp_tool(
    name: &str,
    name_lower: &str,
    prompt_lower: &str,
    wants_mcp: bool,
) -> bool {
    if !name.starts_with("mcp__") {
        return false;
    }

    if prompt_mentions_tool(prompt_lower, name_lower) {
        return true;
    }

    wants_mcp
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

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn prompt_mentions_tool(prompt_lower: &str, tool_name_lower: &str) -> bool {
    if prompt_lower.contains(tool_name_lower) {
        return true;
    }

    // MCP names are `mcp__server__tool_name`. Match the action vocabulary
    // using natural-language tokens so "open the page" can select
    // `browser_open` without requiring the model to already know the exact
    // underscored name. Skip the namespace/server segment and generic domain
    // words; matching only "browser" or "use" would otherwise expose an
    // entire large server catalog.
    let mut segments = tool_name_lower.split("__");
    let _protocol = segments.next();
    if segments.next().is_some_and(|server| {
        server
            .split(|character: char| !character.is_alphanumeric())
            .filter(|token| token.len() > 2 && !is_generic_mcp_namespace_token(token))
            .any(|token| prompt_lower.contains(token))
    }) {
        return true;
    }

    segments
        .flat_map(|segment| segment.split(|character: char| !character.is_alphanumeric()))
        .filter(|token| token.len() > 2 && !is_generic_mcp_action_token(token))
        .any(|token| prompt_lower.contains(token))
}

fn is_generic_mcp_namespace_token(token: &str) -> bool {
    matches!(
        token,
        "agent" | "browser" | "office" | "officecli" | "mcp" | "tool" | "use"
    )
}

fn is_generic_mcp_action_token(token: &str) -> bool {
    is_generic_mcp_namespace_token(token)
        || matches!(
            token,
            "call"
                | "create"
                | "delete"
                | "get"
                | "list"
                | "read"
                | "run"
                | "set"
                | "update"
                | "write"
        )
}

fn is_known_special_tool(name: &str) -> bool {
    matches!(
        name,
        "web_search" | "web_fetch" | "git" | "batch" | "program"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn defs(names: &[&str]) -> Vec<ToolDefinition> {
        names
            .iter()
            .map(|name| ToolDefinition {
                name: (*name).to_string(),
                description: format!("{name} tool"),
                parameters: json!({"type": "object"}),
            })
            .collect()
    }

    #[test]
    fn default_turn_keeps_core_and_hides_special_tools() {
        let selected = select_tools_for_prompt(
            &defs(&[
                "read",
                "write",
                "code_symbols",
                "code_navigation",
                "code_diagnostics",
                "web_search",
                "web_fetch",
                "git",
                "batch",
                "program",
                "task",
                "parallel_task",
                "Skill",
                "search_skills",
                "mcp__github__create_issue",
            ]),
            "fix the failing parser tests",
        );
        let names: Vec<_> = selected.iter().map(|t| t.name.as_str()).collect();

        assert!(names.contains(&"read"));
        assert!(names.contains(&"code_symbols"));
        assert!(names.contains(&"code_navigation"));
        assert!(names.contains(&"code_diagnostics"));
        assert!(names.contains(&"task"));
        assert!(names.contains(&"Skill"));
        assert!(names.contains(&"search_skills"));
        assert!(!names.contains(&"web_search"));
        assert!(!names.contains(&"web_fetch"));
        assert!(!names.contains(&"git"));
        assert!(!names.contains(&"batch"));
        assert!(!names.contains(&"program"));
        assert!(!names.contains(&"mcp__github__create_issue"));
    }

    #[test]
    fn standalone_greetings_do_not_expose_tools() {
        let tools = defs(&["read", "grep", "bash", "web_search", "task"]);

        for prompt in [
            "hi",
            "Hello!",
            "how are you?",
            "你好",
            "您好！",
            "在吗？",
            "谢谢",
        ] {
            assert!(
                select_tools_for_prompt(&tools, prompt).is_empty(),
                "standalone greeting exposed tools: {prompt}"
            );
        }
    }

    #[test]
    fn greeting_with_an_action_keeps_relevant_tools() {
        let selected = select_tools_for_prompt(
            &defs(&["read", "grep", "web_search"]),
            "Hello! Inspect this repository for the parser implementation.",
        );
        let names: Vec<_> = selected.iter().map(|tool| tool.name.as_str()).collect();

        assert!(names.contains(&"read"));
        assert!(names.contains(&"grep"));
    }

    #[test]
    fn program_terms_enable_program_tool() {
        let selected = select_tools_for_prompt(
            &defs(&["read", "grep", "program"]),
            "build a repo map before changing the module",
        );
        let names: Vec<_> = selected.iter().map(|t| t.name.as_str()).collect();

        assert!(names.contains(&"read"));
        assert!(names.contains(&"grep"));
        assert!(names.contains(&"program"));
    }

    #[test]
    fn web_and_git_terms_enable_relevant_tools() {
        let selected = select_tools_for_prompt(
            &defs(&["read", "web_search", "web_fetch", "git"]),
            "look up the latest release notes and commit the fix",
        );
        let names: Vec<_> = selected.iter().map(|t| t.name.as_str()).collect();

        assert!(names.contains(&"web_search"));
        assert!(names.contains(&"web_fetch"));
        assert!(names.contains(&"git"));
    }

    #[test]
    fn mcp_tools_need_mcp_intent_or_direct_match() {
        let selected = select_tools_for_prompt(
            &defs(&[
                "read",
                "mcp__github__create_issue",
                "mcp__linear__create_ticket",
            ]),
            "create a github issue for this bug",
        );
        let names: Vec<_> = selected.iter().map(|t| t.name.as_str()).collect();

        assert!(names.contains(&"mcp__github__create_issue"));
        assert!(!names.contains(&"mcp__linear__create_ticket"));
    }

    #[test]
    fn natural_language_action_tokens_select_use_mcp_tools() {
        let selected = select_tools_for_prompt(
            &defs(&[
                "mcp__use_report__fixture_tool",
                "mcp__use_browser__browser_open",
                "mcp__use_office__office_write_cell",
            ]),
            "Call the report fixture and return what it observed",
        );
        let names: Vec<_> = selected.iter().map(|tool| tool.name.as_str()).collect();

        assert_eq!(names, vec!["mcp__use_report__fixture_tool"]);
    }

    #[test]
    fn tool_heavy_history_keeps_the_original_textual_intent() {
        let tool = "mcp__use_browser__agent_browser_open";
        let mut messages = vec![Message::user(&format!("Call {tool} after diagnostics"))];
        for index in 0..3 {
            let id = format!("call-{index}");
            messages.push(Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: id.clone(),
                    name: format!("diagnostic-{index}"),
                    input: json!({}),
                }],
                reasoning_content: None,
            });
            messages.push(Message::tool_result(&id, "ok", false));
        }

        let selected = select_tools_for_messages(&defs(&[tool]), &messages);
        assert_eq!(
            selected
                .iter()
                .map(|definition| definition.name.as_str())
                .collect::<Vec<_>>(),
            vec![tool]
        );
    }
}
