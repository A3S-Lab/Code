//! Session export to Markdown
//!
//! Converts session conversation history into readable Markdown format.
//! Supports exporting full sessions with metadata, messages, tool calls,
//! and usage statistics.

use crate::llm::{ContentBlock, Message};
use crate::store::SessionData;
use chrono::{DateTime, Utc};
use std::fmt::Write;

/// Options for controlling Markdown export output
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Include session metadata header (ID, timestamps, model, cost)
    pub include_metadata: bool,
    /// Include tool call details (tool_use / tool_result blocks)
    pub include_tool_calls: bool,
    /// Include usage statistics footer
    pub include_usage: bool,
    /// Include system prompt in output
    pub include_system_prompt: bool,
    /// Maximum length for tool result content (0 = no limit)
    pub tool_result_max_length: usize,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_metadata: true,
            include_tool_calls: true,
            include_usage: true,
            include_system_prompt: false,
            tool_result_max_length: 500,
        }
    }
}

/// Export a session to Markdown format
pub fn session_to_markdown(session: &SessionData, options: &ExportOptions) -> String {
    let mut out = String::new();

    if options.include_metadata {
        write_metadata(&mut out, session);
    }

    if options.include_system_prompt {
        if let Some(ref prompt) = session.config.system_prompt {
            if !prompt.is_empty() {
                writeln!(out, "## System Prompt\n").unwrap();
                writeln!(out, "{}\n", prompt).unwrap();
                writeln!(out, "---\n").unwrap();
            }
        }
    }

    writeln!(out, "## Conversation\n").unwrap();
    write_messages(&mut out, &session.messages, options);

    if options.include_usage {
        write_usage_footer(&mut out, session);
    }

    out
}

/// Export only messages to Markdown (no metadata/footer)
pub fn messages_to_markdown(messages: &[Message], options: &ExportOptions) -> String {
    let mut out = String::new();
    write_messages(&mut out, messages, options);
    out
}

// ============================================================================
// Internal helpers
// ============================================================================

fn write_metadata(out: &mut String, session: &SessionData) {
    writeln!(out, "# Session: {}\n", session.config.name).unwrap();

    writeln!(out, "| Field | Value |").unwrap();
    writeln!(out, "|-------|-------|").unwrap();
    writeln!(out, "| **ID** | `{}` |", session.id).unwrap();

    if let Some(ref model) = session.model_name {
        writeln!(out, "| **Model** | {} |", model).unwrap();
    }

    let created = format_timestamp(session.created_at);
    let updated = format_timestamp(session.updated_at);
    writeln!(out, "| **Created** | {} |", created).unwrap();
    writeln!(out, "| **Updated** | {} |", updated).unwrap();
    writeln!(out, "| **State** | {:?} |", session.state).unwrap();

    if session.total_cost > 0.0 {
        writeln!(out, "| **Cost** | ${:.4} |", session.total_cost).unwrap();
    }

    writeln!(out).unwrap();
}

fn write_messages(out: &mut String, messages: &[Message], options: &ExportOptions) {
    for msg in messages {
        let role_label = match msg.role.as_str() {
            "user" => "👤 User",
            "assistant" => "🤖 Assistant",
            _ => &msg.role,
        };

        // Check if this message only contains tool results (user role with only ToolResult blocks)
        let is_tool_result_only = msg.role == "user"
            && !msg.content.is_empty()
            && msg
                .content
                .iter()
                .all(|b| matches!(b, ContentBlock::ToolResult { .. }));

        if is_tool_result_only && !options.include_tool_calls {
            continue;
        }

        let mut has_visible_content = false;

        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    if !text.is_empty() {
                        if !has_visible_content {
                            writeln!(out, "### {}\n", role_label).unwrap();
                            has_visible_content = true;
                        }
                        writeln!(out, "{}\n", text).unwrap();
                    }
                }
                ContentBlock::ToolUse { name, input, .. } => {
                    if options.include_tool_calls {
                        if !has_visible_content {
                            writeln!(out, "### {}\n", role_label).unwrap();
                            has_visible_content = true;
                        }
                        writeln!(out, "**🔧 Tool Call: `{}`**\n", name).unwrap();
                        let input_str = serde_json::to_string_pretty(input)
                            .unwrap_or_else(|_| input.to_string());
                        writeln!(out, "```json\n{}\n```\n", input_str).unwrap();
                    }
                }
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => {
                    if options.include_tool_calls {
                        if !has_visible_content {
                            writeln!(out, "### {}\n", role_label).unwrap();
                            has_visible_content = true;
                        }
                        let status = if is_error.unwrap_or(false) {
                            "❌ Error"
                        } else {
                            "✅ Result"
                        };
                        writeln!(out, "**{}**\n", status).unwrap();

                        let display_content =
                            truncate_content(content, options.tool_result_max_length);
                        writeln!(out, "```\n{}\n```\n", display_content).unwrap();
                    }
                }
            }
        }
    }
}

fn write_usage_footer(out: &mut String, session: &SessionData) {
    writeln!(out, "---\n").unwrap();
    writeln!(out, "## Usage Statistics\n").unwrap();
    writeln!(out, "| Metric | Value |").unwrap();
    writeln!(out, "|--------|-------|").unwrap();
    writeln!(
        out,
        "| **Prompt Tokens** | {} |",
        session.total_usage.prompt_tokens
    )
    .unwrap();
    writeln!(
        out,
        "| **Completion Tokens** | {} |",
        session.total_usage.completion_tokens
    )
    .unwrap();
    writeln!(
        out,
        "| **Total Tokens** | {} |",
        session.total_usage.total_tokens
    )
    .unwrap();
    writeln!(
        out,
        "| **Context Turns** | {} |",
        session.context_usage.turns
    )
    .unwrap();

    if session.total_cost > 0.0 {
        writeln!(out, "| **Total Cost** | ${:.4} |", session.total_cost).unwrap();
    }

    writeln!(out).unwrap();
}

fn format_timestamp(epoch_secs: i64) -> String {
    DateTime::<Utc>::from_timestamp(epoch_secs, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| format!("{}", epoch_secs))
}

fn truncate_content(content: &str, max_length: usize) -> String {
    if max_length == 0 || content.len() <= max_length {
        content.to_string()
    } else {
        let truncated = &content[..max_length];
        format!(
            "{}...\n\n(truncated, {} total bytes)",
            truncated,
            content.len()
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ContentBlock, Message, TokenUsage};
    use crate::session::{ContextUsage, SessionConfig, SessionState};
    use crate::store::SessionData;

    fn create_test_session() -> SessionData {
        SessionData {
            id: "test-session-123".to_string(),
            config: SessionConfig {
                name: "Test Session".to_string(),
                workspace: "/tmp/test".to_string(),
                system_prompt: Some("You are a helpful assistant.".to_string()),
                max_context_length: 200_000,
                auto_compact: true,
                auto_compact_threshold: 0.8,
                storage_type: crate::config::StorageBackend::Memory,
                queue_config: None,
                confirmation_policy: None,
                permission_policy: None,
                parent_id: None,
                security_config: None,
                hook_engine: None,
                planning_enabled: false,
                goal_tracking: false,
            },
            state: SessionState::Active,
            messages: vec![
                Message::user("Hello, can you help me?"),
                Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "Of course! How can I help you today?".to_string(),
                    }],
                    reasoning_content: None,
                },
                Message::user("Read the file main.rs"),
                Message {
                    role: "assistant".to_string(),
                    content: vec![
                        ContentBlock::Text {
                            text: "I'll read that file for you.".to_string(),
                        },
                        ContentBlock::ToolUse {
                            id: "tool_1".to_string(),
                            name: "read".to_string(),
                            input: serde_json::json!({"file_path": "main.rs"}),
                        },
                    ],
                    reasoning_content: None,
                },
                Message::tool_result("tool_1", "fn main() {\n    println!(\"Hello\");\n}", false),
                Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text: "Here's the content of `main.rs`. It's a simple Hello World program."
                            .to_string(),
                    }],
                    reasoning_content: None,
                },
            ],
            context_usage: ContextUsage {
                used_tokens: 1500,
                max_tokens: 200_000,
                percent: 0.0075,
                turns: 6,
            },
            total_usage: TokenUsage {
                prompt_tokens: 1200,
                completion_tokens: 300,
                total_tokens: 1500,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            total_cost: 0.0045,
            model_name: Some("claude-sonnet-4-20250514".to_string()),
            tool_names: vec!["read".to_string(), "write".to_string()],
            thinking_enabled: false,
            thinking_budget: None,
            created_at: 1700000000,
            updated_at: 1700000100,
            llm_config: None,
            todos: vec![],
            parent_id: None,
            cost_records: Vec::new(),
        }
    }

    #[test]
    fn test_export_default_options() {
        let session = create_test_session();
        let options = ExportOptions::default();
        let md = session_to_markdown(&session, &options);

        // Should contain metadata
        assert!(md.contains("# Session: Test Session"));
        assert!(md.contains("test-session-123"));
        assert!(md.contains("claude-sonnet-4-20250514"));

        // Should contain conversation
        assert!(md.contains("## Conversation"));
        assert!(md.contains("👤 User"));
        assert!(md.contains("🤖 Assistant"));
        assert!(md.contains("Hello, can you help me?"));
        assert!(md.contains("Of course! How can I help you today?"));

        // Should contain tool calls
        assert!(md.contains("🔧 Tool Call: `read`"));
        assert!(md.contains("file_path"));

        // Should contain usage
        assert!(md.contains("## Usage Statistics"));
        assert!(md.contains("1200"));
        assert!(md.contains("$0.0045"));
    }

    #[test]
    fn test_export_no_metadata() {
        let session = create_test_session();
        let options = ExportOptions {
            include_metadata: false,
            ..Default::default()
        };
        let md = session_to_markdown(&session, &options);

        assert!(!md.contains("# Session: Test Session"));
        assert!(!md.contains("test-session-123"));
        assert!(md.contains("## Conversation"));
    }

    #[test]
    fn test_export_no_tool_calls() {
        let session = create_test_session();
        let options = ExportOptions {
            include_tool_calls: false,
            ..Default::default()
        };
        let md = session_to_markdown(&session, &options);

        assert!(!md.contains("🔧 Tool Call"));
        assert!(!md.contains("✅ Result"));
        // Text content should still be present
        assert!(md.contains("Hello, can you help me?"));
        assert!(md.contains("Of course! How can I help you today?"));
    }

    #[test]
    fn test_export_no_usage() {
        let session = create_test_session();
        let options = ExportOptions {
            include_usage: false,
            ..Default::default()
        };
        let md = session_to_markdown(&session, &options);

        assert!(!md.contains("## Usage Statistics"));
        assert!(!md.contains("Prompt Tokens"));
    }

    #[test]
    fn test_export_with_system_prompt() {
        let session = create_test_session();
        let options = ExportOptions {
            include_system_prompt: true,
            ..Default::default()
        };
        let md = session_to_markdown(&session, &options);

        assert!(md.contains("## System Prompt"));
        assert!(md.contains("You are a helpful assistant."));
    }

    #[test]
    fn test_export_without_system_prompt() {
        let session = create_test_session();
        let options = ExportOptions::default();
        let md = session_to_markdown(&session, &options);

        assert!(!md.contains("## System Prompt"));
    }

    #[test]
    fn test_export_tool_result_truncation() {
        let mut session = create_test_session();
        let long_content = "x".repeat(1000);
        session.messages = vec![Message::tool_result("tool_1", &long_content, false)];

        let options = ExportOptions {
            include_metadata: false,
            include_usage: false,
            tool_result_max_length: 100,
            ..Default::default()
        };
        let md = session_to_markdown(&session, &options);

        assert!(md.contains("truncated"));
        assert!(md.contains("1000 total bytes"));
    }

    #[test]
    fn test_export_tool_result_no_truncation() {
        let mut session = create_test_session();
        session.messages = vec![Message::tool_result("tool_1", "short content", false)];

        let options = ExportOptions {
            include_metadata: false,
            include_usage: false,
            tool_result_max_length: 500,
            ..Default::default()
        };
        let md = session_to_markdown(&session, &options);

        assert!(md.contains("short content"));
        assert!(!md.contains("truncated"));
    }

    #[test]
    fn test_export_tool_result_unlimited() {
        let mut session = create_test_session();
        let long_content = "x".repeat(1000);
        session.messages = vec![Message::tool_result("tool_1", &long_content, false)];

        let options = ExportOptions {
            include_metadata: false,
            include_usage: false,
            tool_result_max_length: 0, // no limit
            ..Default::default()
        };
        let md = session_to_markdown(&session, &options);

        assert!(!md.contains("truncated"));
        assert!(md.contains(&long_content));
    }

    #[test]
    fn test_export_error_tool_result() {
        let mut session = create_test_session();
        session.messages = vec![Message::tool_result("tool_1", "command not found", true)];

        let options = ExportOptions {
            include_metadata: false,
            include_usage: false,
            ..Default::default()
        };
        let md = session_to_markdown(&session, &options);

        assert!(md.contains("❌ Error"));
        assert!(md.contains("command not found"));
    }

    #[test]
    fn test_export_empty_messages() {
        let mut session = create_test_session();
        session.messages = vec![];

        let options = ExportOptions::default();
        let md = session_to_markdown(&session, &options);

        assert!(md.contains("## Conversation"));
        // Should still have metadata and usage
        assert!(md.contains("# Session: Test Session"));
        assert!(md.contains("## Usage Statistics"));
    }

    #[test]
    fn test_messages_to_markdown() {
        let messages = vec![
            Message::user("What is Rust?"),
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Rust is a systems programming language.".to_string(),
                }],
                reasoning_content: None,
            },
        ];

        let options = ExportOptions::default();
        let md = messages_to_markdown(&messages, &options);

        assert!(md.contains("👤 User"));
        assert!(md.contains("What is Rust?"));
        assert!(md.contains("🤖 Assistant"));
        assert!(md.contains("Rust is a systems programming language."));
        // Should NOT contain metadata or usage
        assert!(!md.contains("# Session"));
        assert!(!md.contains("## Usage Statistics"));
    }

    #[test]
    fn test_format_timestamp() {
        let ts = format_timestamp(1700000000);
        assert!(ts.contains("2023"));
        assert!(ts.contains("UTC"));
    }

    #[test]
    fn test_format_timestamp_zero() {
        let ts = format_timestamp(0);
        assert!(ts.contains("1970"));
    }

    #[test]
    fn test_truncate_content_short() {
        let result = truncate_content("hello", 100);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_truncate_content_exact() {
        let result = truncate_content("hello", 5);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_truncate_content_long() {
        let result = truncate_content("hello world", 5);
        assert!(result.starts_with("hello"));
        assert!(result.contains("truncated"));
        assert!(result.contains("11 total bytes"));
    }

    #[test]
    fn test_truncate_content_unlimited() {
        let long = "x".repeat(10000);
        let result = truncate_content(&long, 0);
        assert_eq!(result, long);
    }

    #[test]
    fn test_export_metadata_no_cost() {
        let mut session = create_test_session();
        session.total_cost = 0.0;

        let options = ExportOptions {
            include_metadata: true,
            include_tool_calls: false,
            include_usage: false,
            include_system_prompt: false,
            tool_result_max_length: 500,
        };
        let md = session_to_markdown(&session, &options);

        // Cost row should not appear when cost is 0
        assert!(!md.contains("**Cost**"));
    }

    #[test]
    fn test_export_metadata_no_model() {
        let mut session = create_test_session();
        session.model_name = None;

        let options = ExportOptions {
            include_metadata: true,
            include_tool_calls: false,
            include_usage: false,
            include_system_prompt: false,
            tool_result_max_length: 500,
        };
        let md = session_to_markdown(&session, &options);

        // Model row should not appear when model is None
        assert!(!md.contains("**Model**"));
    }

    #[test]
    fn test_export_options_default() {
        let options = ExportOptions::default();
        assert!(options.include_metadata);
        assert!(options.include_tool_calls);
        assert!(options.include_usage);
        assert!(!options.include_system_prompt);
        assert_eq!(options.tool_result_max_length, 500);
    }

    #[test]
    fn test_export_mixed_content_blocks() {
        let mut session = create_test_session();
        session.messages = vec![Message {
            role: "assistant".to_string(),
            content: vec![
                ContentBlock::Text {
                    text: "Let me check that.".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "ls -la"}),
                },
                ContentBlock::Text {
                    text: "Here are the results.".to_string(),
                },
            ],
            reasoning_content: None,
        }];

        let options = ExportOptions {
            include_metadata: false,
            include_usage: false,
            ..Default::default()
        };
        let md = session_to_markdown(&session, &options);

        assert!(md.contains("Let me check that."));
        assert!(md.contains("🔧 Tool Call: `bash`"));
        assert!(md.contains("ls -la"));
        assert!(md.contains("Here are the results."));
    }
}
