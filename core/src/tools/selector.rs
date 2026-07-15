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
    if tools.is_empty() {
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
    let mut parts = Vec::new();
    for message in messages.iter().rev().take(6).rev() {
        if message.role == "tool" {
            continue;
        }
        for block in &message.content {
            if let ContentBlock::Text { text } = block {
                parts.push(text.as_str());
            }
        }
    }
    parts.join("\n")
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn prompt_mentions_tool(prompt_lower: &str, tool_name_lower: &str) -> bool {
    prompt_lower.contains(tool_name_lower)
        || tool_name_lower
            .split("__")
            .any(|part| part.len() > 2 && prompt_lower.contains(part))
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
}
