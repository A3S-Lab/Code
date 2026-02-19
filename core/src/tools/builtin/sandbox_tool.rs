//! Sandbox tool — execute commands in an A3S Box MicroVM sandbox.
//!
//! This tool is only compiled and registered when the `sandbox` Cargo feature
//! is enabled. It gives the LLM explicit access to a MicroVM sandbox,
//! separate from the `bash` tool's transparent routing path.
//!
//! The sandbox is created lazily on the first call and reused for the
//! lifetime of the session. The workspace directory is automatically mounted
//! at `/workspace` inside the sandbox.

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use a3s_box_sdk::{BoxSdk, MountSpec, Sandbox, SandboxOptions};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

// ============================================================================
// Internal state
// ============================================================================

struct SandboxState {
    sandbox: Option<Sandbox>,
}

// ============================================================================
// SandboxTool
// ============================================================================

/// Built-in tool: execute a command inside an A3S Box MicroVM sandbox.
///
/// The sandbox is created lazily on the first call and reused for subsequent
/// calls within the same session. The session workspace is mounted at
/// `/workspace` inside the sandbox.
pub struct SandboxTool {
    state: Arc<Mutex<SandboxState>>,
}

impl SandboxTool {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(SandboxState { sandbox: None })),
        }
    }
}

impl Drop for SandboxTool {
    fn drop(&mut self) {
        // Best-effort async sandbox cleanup on drop.
        let state = Arc::clone(&self.state);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let mut s = state.lock().await;
                if let Some(sandbox) = s.sandbox.take() {
                    let _ = sandbox.stop().await;
                }
            });
        }
    }
}

#[async_trait]
impl Tool for SandboxTool {
    fn name(&self) -> &str {
        "sandbox"
    }

    fn description(&self) -> &str {
        "Execute a shell command inside an isolated A3S Box MicroVM sandbox. \
         The session workspace is mounted at /workspace. The sandbox is created \
         on the first call and reused for subsequent calls in the same session."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute inside the sandbox"
                },
                "image": {
                    "type": "string",
                    "description": "OCI image for the sandbox (default: alpine:latest). \
                                    Only used when creating the sandbox on the first call."
                },
                "memory_mb": {
                    "type": "integer",
                    "description": "Memory limit in megabytes (default: 512). \
                                    Only used when creating the sandbox on the first call."
                },
                "network": {
                    "type": "boolean",
                    "description": "Enable outbound networking (default: false)"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("command parameter is required")),
        };

        let image = args
            .get("image")
            .and_then(|v| v.as_str())
            .unwrap_or("alpine:latest");
        let memory_mb = args
            .get("memory_mb")
            .and_then(|v| v.as_u64())
            .unwrap_or(512) as u32;
        let network = args
            .get("network")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mut state = self.state.lock().await;

        // Boot the sandbox on first use.
        if state.sandbox.is_none() {
            tracing::info!(image, "Booting A3S Box sandbox for sandbox tool");

            let sdk = BoxSdk::new()
                .await
                .map_err(|e| anyhow::anyhow!("BoxSdk initialization failed: {}", e))?;

            let opts = SandboxOptions {
                image: image.into(),
                memory_mb,
                network,
                mounts: vec![MountSpec {
                    host_path: ctx.workspace.display().to_string(),
                    guest_path: "/workspace".into(),
                    readonly: false,
                }],
                workdir: Some("/workspace".into()),
                ..SandboxOptions::default()
            };

            let sandbox = sdk
                .create(opts)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to create sandbox: {}", e))?;

            tracing::info!("A3S Box sandbox ready");
            state.sandbox = Some(sandbox);
        }

        let sandbox = state.sandbox.as_ref().unwrap();
        let result = sandbox
            .exec("bash", &["-c", command])
            .await
            .map_err(|e| anyhow::anyhow!("Sandbox exec failed: {}", e))?;

        let mut output = result.stdout;
        if !result.stderr.is_empty() {
            output.push_str(&result.stderr);
        }

        Ok(ToolOutput {
            content: output,
            success: result.exit_code == 0,
            metadata: Some(serde_json::json!({ "exit_code": result.exit_code })),
            images: Vec::new(),
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_sandbox_tool_name() {
        let tool = SandboxTool::new();
        assert_eq!(tool.name(), "sandbox");
    }

    #[test]
    fn test_sandbox_tool_description_not_empty() {
        let tool = SandboxTool::new();
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_sandbox_tool_parameters_has_command() {
        let tool = SandboxTool::new();
        let params = tool.parameters();
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("command")));
    }

    #[test]
    fn test_sandbox_tool_parameters_has_optional_fields() {
        let tool = SandboxTool::new();
        let params = tool.parameters();
        let props = params["properties"].as_object().unwrap();
        assert!(props.contains_key("image"));
        assert!(props.contains_key("memory_mb"));
        assert!(props.contains_key("network"));
    }

    #[tokio::test]
    async fn test_sandbox_tool_missing_command() {
        let tool = SandboxTool::new();
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool
            .execute(&serde_json::json!({}), &ctx)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.content.contains("command"));
    }
}
