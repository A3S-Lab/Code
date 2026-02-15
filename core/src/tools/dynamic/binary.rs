//! Binary tool - Execute external binaries

use super::{read_process_output, substitute_template_args};
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::process::Command;

/// Tool that executes an external binary
pub struct BinaryTool {
    name: String,
    description: String,
    parameters: serde_json::Value,
    /// URL to download the binary (for skill-based tools)
    url: Option<String>,
    /// Local path to the binary
    path: Option<String>,
    /// Arguments template with ${arg_name} substitution
    args_template: Option<String>,
}

impl BinaryTool {
    pub fn new(
        name: String,
        description: String,
        parameters: serde_json::Value,
        url: Option<String>,
        path: Option<String>,
        args_template: Option<String>,
    ) -> Self {
        Self {
            name,
            description,
            parameters,
            url,
            path,
            args_template,
        }
    }

    /// Get the binary path, downloading if necessary
    async fn get_binary_path(&self, ctx: &ToolContext) -> Result<String> {
        // If we have a local path, use it
        if let Some(path) = &self.path {
            return Ok(path.clone());
        }

        // If we have a URL, check cache or download
        if let Some(url) = &self.url {
            let cache_dir = ctx.workspace.join(".a3s/cache/tools");
            let binary_name = url.split('/').next_back().unwrap_or(&self.name);
            let cached_path = cache_dir.join(binary_name);

            if cached_path.exists() {
                return Ok(cached_path.to_string_lossy().to_string());
            }

            // Download the binary
            tracing::info!("Downloading tool binary from: {}", url);
            tokio::fs::create_dir_all(&cache_dir).await?;

            let response = reqwest::get(url)
                .await
                .with_context(|| format!("Failed to download binary from {}", url))?;

            if !response.status().is_success() {
                anyhow::bail!("Failed to download binary: HTTP {}", response.status());
            }

            let bytes = response.bytes().await?;
            tokio::fs::write(&cached_path, &bytes).await?;

            // Make executable on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = tokio::fs::metadata(&cached_path).await?.permissions();
                perms.set_mode(0o755);
                tokio::fs::set_permissions(&cached_path, perms).await?;
            }

            return Ok(cached_path.to_string_lossy().to_string());
        }

        anyhow::bail!("No binary path or URL specified for tool: {}", self.name)
    }
}

#[async_trait]
impl Tool for BinaryTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let binary_path = self.get_binary_path(ctx).await?;

        tracing::debug!("Executing binary: {}", binary_path);

        let mut cmd = Command::new(&binary_path);
        cmd.current_dir(&ctx.workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Add arguments from template or pass as JSON
        if let Some(template) = &self.args_template {
            let args_str = substitute_template_args(template, args);
            // Split by whitespace, respecting quotes
            for arg in shell_words::split(&args_str).unwrap_or_default() {
                cmd.arg(arg);
            }
        } else {
            // Pass arguments as environment variables
            if let Some(obj) = args.as_object() {
                for (key, value) in obj {
                    let env_key = format!("TOOL_ARG_{}", key.to_uppercase());
                    let env_value = match value {
                        serde_json::Value::String(s) => s.clone(),
                        _ => value.to_string(),
                    };
                    cmd.env(env_key, env_value);
                }
            }
            // Also pass full args as JSON
            cmd.env("TOOL_ARGS", args.to_string());
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn binary: {}", binary_path))?;

        let (output, timed_out) = read_process_output(&mut child, 60).await;

        if timed_out {
            return Ok(ToolOutput::error(format!(
                "{}\n\n[Binary execution timed out after 60s]",
                output
            )));
        }

        let status = child.wait().await?;
        let exit_code = status.code().unwrap_or(-1);

        Ok(ToolOutput {
            content: output,
            success: exit_code == 0,
            metadata: Some(serde_json::json!({ "exit_code": exit_code })),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_substitute_args() {
        let args = serde_json::json!({
            "message": "hello",
            "count": 42
        });

        let result = substitute_template_args("${message} ${count}", &args);
        assert_eq!(result, "hello 42");
    }

    #[tokio::test]
    async fn test_binary_tool_echo() {
        let tool = BinaryTool::new(
            "echo".to_string(),
            "Echo tool".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {"type": "string"}
                }
            }),
            None,
            Some("/bin/echo".to_string()),
            Some("${message}".to_string()),
        );

        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool
            .execute(&serde_json::json!({"message": "hello world"}), &ctx)
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("hello world"));
    }

    #[test]
    fn test_substitute_args_with_number() {
        let args = serde_json::json!({
            "count": 123,
            "price": 45.67
        });

        let result = substitute_template_args("count=${count} price=${price}", &args);
        assert_eq!(result, "count=123 price=45.67");
    }

    #[test]
    fn test_substitute_args_with_bool() {
        let args = serde_json::json!({
            "enabled": true,
            "disabled": false
        });

        let result = substitute_template_args("enabled=${enabled} disabled=${disabled}", &args);
        assert_eq!(result, "enabled=true disabled=false");
    }

    #[test]
    fn test_substitute_args_with_complex_json() {
        let args = serde_json::json!({
            "data": {"nested": "value"}
        });

        let result = substitute_template_args("data=${data}", &args);
        assert!(result.contains("nested"));
    }

    #[test]
    fn test_substitute_args_no_placeholders() {
        let args = serde_json::json!({"key": "value"});
        let result = substitute_template_args("no placeholders here", &args);
        assert_eq!(result, "no placeholders here");
    }

    #[test]
    fn test_substitute_args_missing_key() {
        let args = serde_json::json!({"key": "value"});
        let result = substitute_template_args("${missing} ${key}", &args);
        assert_eq!(result, "${missing} value");
    }

    #[test]
    fn test_substitute_args_empty_object() {
        let args = serde_json::json!({});
        let result = substitute_template_args("${key}", &args);
        assert_eq!(result, "${key}");
    }

    #[test]
    fn test_substitute_args_non_object() {
        let args = serde_json::json!("string value");
        let result = substitute_template_args("${key}", &args);
        assert_eq!(result, "${key}");
    }

    #[test]
    fn test_binary_tool_name() {
        let tool = BinaryTool::new(
            "my-tool".to_string(),
            "description".to_string(),
            serde_json::json!({}),
            None,
            Some("/bin/test".to_string()),
            None,
        );
        assert_eq!(tool.name(), "my-tool");
    }

    #[test]
    fn test_binary_tool_description() {
        let tool = BinaryTool::new(
            "tool".to_string(),
            "My tool description".to_string(),
            serde_json::json!({}),
            None,
            Some("/bin/test".to_string()),
            None,
        );
        assert_eq!(tool.description(), "My tool description");
    }

    #[test]
    fn test_binary_tool_parameters() {
        let params = serde_json::json!({
            "type": "object",
            "properties": {
                "arg": {"type": "string"}
            }
        });
        let tool = BinaryTool::new(
            "tool".to_string(),
            "description".to_string(),
            params.clone(),
            None,
            Some("/bin/test".to_string()),
            None,
        );
        assert_eq!(tool.parameters(), params);
    }

    #[test]
    fn test_binary_tool_with_url() {
        let tool = BinaryTool::new(
            "remote-tool".to_string(),
            "description".to_string(),
            serde_json::json!({}),
            Some("https://example.com/tool".to_string()),
            None,
            None,
        );
        assert_eq!(tool.name(), "remote-tool");
    }

    #[test]
    fn test_binary_tool_with_args_template() {
        let tool = BinaryTool::new(
            "tool".to_string(),
            "description".to_string(),
            serde_json::json!({}),
            None,
            Some("/bin/test".to_string()),
            Some("--arg ${value}".to_string()),
        );
        assert_eq!(tool.name(), "tool");
    }

    #[test]
    fn test_substitute_args_multiple_same_placeholder() {
        let args = serde_json::json!({"name": "test"});
        let result = substitute_template_args("${name} and ${name} again", &args);
        assert_eq!(result, "test and test again");
    }

    #[test]
    fn test_substitute_args_special_characters() {
        let args = serde_json::json!({"path": "/tmp/test file.txt"});
        let result = substitute_template_args("${path}", &args);
        assert_eq!(result, "/tmp/test file.txt");
    }

    #[tokio::test]
    async fn test_binary_tool_with_multiple_args() {
        let tool = BinaryTool::new(
            "echo".to_string(),
            "Echo tool".to_string(),
            serde_json::json!({}),
            None,
            Some("/bin/echo".to_string()),
            Some("${arg1} ${arg2} ${arg3}".to_string()),
        );

        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool
            .execute(
                &serde_json::json!({"arg1": "hello", "arg2": "world", "arg3": "test"}),
                &ctx,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert!(result.content.contains("hello"));
        assert!(result.content.contains("world"));
        assert!(result.content.contains("test"));
    }
}
