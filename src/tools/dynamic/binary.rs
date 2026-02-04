//! Binary tool - Execute external binaries

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::MAX_OUTPUT_SIZE;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
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

    /// Substitute ${arg_name} placeholders in template with actual values
    fn substitute_args(&self, template: &str, args: &serde_json::Value) -> String {
        let mut result = template.to_string();

        if let Some(obj) = args.as_object() {
            for (key, value) in obj {
                let placeholder = format!("${{{}}}", key);
                let replacement = match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => value.to_string(),
                };
                result = result.replace(&placeholder, &replacement);
            }
        }

        result
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
            let args_str = self.substitute_args(template, args);
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

        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let mut output = String::new();
        let mut total_size = 0usize;

        // Read output with timeout (60 seconds for binary tools)
        let timeout = tokio::time::Duration::from_secs(60);
        let result = tokio::time::timeout(timeout, async {
            loop {
                tokio::select! {
                    line = stdout_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if total_size < MAX_OUTPUT_SIZE {
                                    output.push_str(&line);
                                    output.push('\n');
                                    total_size += line.len() + 1;
                                }
                            }
                            Ok(None) => break,
                            Err(_) => break,
                        }
                    }
                    line = stderr_reader.next_line() => {
                        match line {
                            Ok(Some(line)) => {
                                if total_size < MAX_OUTPUT_SIZE {
                                    output.push_str(&line);
                                    output.push('\n');
                                    total_size += line.len() + 1;
                                }
                            }
                            Ok(None) => {}
                            Err(_) => {}
                        }
                    }
                }
            }
        })
        .await;

        if result.is_err() {
            child.kill().await.ok();
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
        let tool = BinaryTool::new(
            "test".to_string(),
            "test".to_string(),
            serde_json::json!({}),
            None,
            Some("/bin/echo".to_string()),
            Some("${message} ${count}".to_string()),
        );

        let args = serde_json::json!({
            "message": "hello",
            "count": 42
        });

        let result = tool.substitute_args("${message} ${count}", &args);
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
}
