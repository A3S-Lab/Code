//! Shell command preprocessor for skill prompts
//!
//! Supports two syntaxes in skill markdown content:
//! - Code blocks: ```! command ```
//! - Inline: !`command`
//!
//! ## Usage
//!
//! ```rust
//! use a3s_code_core::skills::ShellPreprocessor;
//!
//! let preprocessor = ShellPreprocessor::new();
//! let result = preprocessor.process("Run !`echo hello`", &context).await?;
//! ```

use crate::error::Result;
use crate::tools::{ToolExecutor, ToolResult};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

/// Shell command output extractor
#[async_trait]
pub trait ShellPreprocessor: Send + Sync {
    /// Process skill content, executing any embedded shell commands
    async fn process(
        &self,
        content: &str,
        workspace: &PathBuf,
        executor: &Arc<ToolExecutor>,
    ) -> Result<String>;
}

/// Default shell preprocessor implementation
pub struct DefaultShellPreprocessor {
    /// Maximum shell output length to substitute (bytes)
    max_output_length: usize,
}

impl DefaultShellPreprocessor {
    pub fn new() -> Self {
        Self {
            max_output_length: 10 * 1024, // 10KB
        }
    }

    /// Process inline shell commands: !`command`
    fn process_inline(&self, content: &str) -> Vec<(String, String)> {
        let mut results = Vec::new();
        // Pattern: !`command` - must be at start of line or after whitespace
        // Cannot use lookbehind, so match the preceding char too and strip it
        let re = regex::Regex::new(r"(?:^|\s)(!`([^`]+)`)").unwrap();

        for cap in re.captures_iter(content) {
            let full_match = cap.get(1).unwrap().as_str();
            let command = cap.get(2).unwrap().as_str().trim();
            results.push((full_match.to_string(), command.to_string()));
        }

        results
    }

    /// Process code block shell commands: ```! command ```
    fn process_blocks(&self, content: &str) -> Vec<(String, String)> {
        let mut results = Vec::new();
        // Pattern: ```! command ```
        let re = regex::Regex::new(r"```!\s*\n?([\s\S]*?)\n?```").unwrap();

        for cap in re.captures_iter(content) {
            let full_match = cap.get(0).unwrap().as_str();
            let command = cap.get(1).unwrap().as_str().trim();
            if !command.is_empty() {
                results.push((full_match.to_string(), command.to_string()));
            }
        }

        results
    }
}

impl Default for DefaultShellPreprocessor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ShellPreprocessor for DefaultShellPreprocessor {
    async fn process(
        &self,
        content: &str,
        workspace: &PathBuf,
        executor: &Arc<ToolExecutor>,
    ) -> Result<String> {
        let mut result = content.to_string();

        // Process code blocks first: ```! command ```
        for (pattern, command) in self.process_blocks(content) {
            let output = self.execute_command(&command, workspace, executor).await?;
            result = result.replace(&pattern, &output);
        }

        // Process inline commands: !`command`
        // Gate on presence of !` to avoid expensive regex on large content
        if result.contains("!`") {
            for (pattern, command) in self.process_inline(&result) {
                let output = self.execute_command(&command, workspace, executor).await?;
                result = result.replace(&pattern, &output);
            }
        }

        Ok(result)
    }
}

impl DefaultShellPreprocessor {
    async fn execute_command(
        &self,
        command: &str,
        workspace: &PathBuf,
        executor: &Arc<ToolExecutor>,
    ) -> Result<String> {
        let args = serde_json::json!({ "command": command });
        let result: ToolResult = executor
            .execute_with_context(
                "bash",
                &args,
                &crate::tools::ToolContext {
                    workspace: workspace.clone(),
                    session_id: None,
                    event_tx: None,
                    agent_event_tx: None,
                    search_config: None,
                    agentic_search_config: None,
                    agentic_parse_config: None,
                    document_parser_config: None,
                    sandbox: None,
                    command_env: None,
                    document_parsers: None,
                    document_pipeline: None,
                },
            )
            .await?;

        if result.exit_code != 0 {
            return Ok(format!("[Error: {}]", result.output));
        }

        let output = result.output.trim().to_string();
        if output.len() > self.max_output_length {
            Ok(format!(
                "{}... [truncated {} bytes]",
                &output[..self.max_output_length],
                output.len() - self.max_output_length
            ))
        } else {
            Ok(output)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_inline_pattern() {
        let preprocessor = DefaultShellPreprocessor::new();
        let matches = preprocessor.process_inline("Run !`echo hello` now");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].0, "!`echo hello`");
        assert_eq!(matches[0].1, "echo hello");
    }

    #[test]
    fn test_process_block_pattern() {
        let preprocessor = DefaultShellPreprocessor::new();
        let content = r#"```!
ls -la
```"#;
        let matches = preprocessor.process_blocks(content);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].1, "ls -la");
    }

    #[test]
    fn test_process_block_pattern_with_newlines() {
        let preprocessor = DefaultShellPreprocessor::new();
        // Block pattern requires ```! (not just ``` followed by !)
        let content = r#"```!
ls -la
```"#;
        let matches = preprocessor.process_blocks(content);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].1, "ls -la");
    }

    #[test]
    fn test_no_false_positives() {
        let preprocessor = DefaultShellPreprocessor::new();
        // $! (shell variable ending in !) should not match since it's not !`
        let matches = preprocessor.process_inline("echo $! and !`echo hi`");
        assert_eq!(matches.len(), 1); // only the actual !`...` should match
        assert_eq!(matches[0].1, "echo hi");
    }

    #[test]
    fn test_process_inline_at_start_of_line() {
        let preprocessor = DefaultShellPreprocessor::new();
        let matches = preprocessor.process_inline("!`pwd`");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].1, "pwd");
    }

    #[test]
    fn test_process_inline_multiple() {
        let preprocessor = DefaultShellPreprocessor::new();
        let matches = preprocessor.process_inline("First !`cmd1` then !`cmd2`");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].1, "cmd1");
        assert_eq!(matches[1].1, "cmd2");
    }

    #[test]
    fn test_process_inline_no_matches() {
        let preprocessor = DefaultShellPreprocessor::new();
        let matches = preprocessor.process_inline("No shell commands here");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_process_inline_backtick_in_string() {
        let preprocessor = DefaultShellPreprocessor::new();
        // Should not match backticks without !
        let matches = preprocessor.process_inline("`echo hello`");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_process_blocks_multiple() {
        let preprocessor = DefaultShellPreprocessor::new();
        let content = r#"```!
ls -la
```
Some text
```!
pwd
```"#;
        let matches = preprocessor.process_blocks(content);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].1, "ls -la");
        assert_eq!(matches[1].1, "pwd");
    }

    #[test]
    fn test_process_blocks_no_matches() {
        let preprocessor = DefaultShellPreprocessor::new();
        let content = "```\nsome code\n```";
        let matches = preprocessor.process_blocks(content);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_process_blocks_empty_command() {
        let preprocessor = DefaultShellPreprocessor::new();
        let content = "```!\n```";
        let matches = preprocessor.process_blocks(content);
        // Empty commands are filtered out
        assert!(matches.is_empty());
    }

    #[test]
    fn test_default_shell_preprocessor_new() {
        let preprocessor = DefaultShellPreprocessor::new();
        assert_eq!(preprocessor.max_output_length, 10 * 1024);
    }
}
