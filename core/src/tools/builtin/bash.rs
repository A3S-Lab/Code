//! Bash tool - Execute shell commands

use crate::tools::types::{Tool, ToolContext, ToolEventSender, ToolOutput, ToolStreamEvent};
use crate::workspace::{CommandOutputObserver, CommandRequest};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::process::Command;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::maybe_execute_simple_windows_http_command;
#[cfg(windows)]
use windows::{build_powershell_command, encode_powershell_command, CREATE_NO_WINDOW};
#[cfg(all(test, windows))]
use windows::{
    normalize_json_like_literal, parse_simple_windows_http_command, preprocess_windows_command,
};

/// Default timeout in milliseconds (2 minutes)
pub(crate) const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MIN_TIMEOUT_MS: u64 = 1_000;

/// Adapter that forwards `CommandOutputObserver` deltas to a tool event channel.
///
/// Keeps `workspace::CommandRequest` free of `ToolEventSender`. Constructed in
/// the bash tool when a session has installed an event channel; backend
/// implementations only see `&dyn CommandOutputObserver`.
struct ToolEventObserver {
    tx: ToolEventSender,
}

#[async_trait]
impl CommandOutputObserver for ToolEventObserver {
    async fn on_output_delta(&self, delta: &str) {
        self.tx
            .send(ToolStreamEvent::OutputDelta(delta.to_string()))
            .await
            .ok();
    }
}

pub struct BashTool;

/// Spawn a shell command cross-platform.
///
/// - Unix: `bash -c <command>`
/// - Windows: `powershell.exe -Command <command>` with hidden console window,
///   falling back to `cmd.exe /C <command>` if PowerShell cannot be started.
pub(crate) fn spawn_shell(
    command: &str,
    workspace: &std::path::Path,
    command_env: Option<&HashMap<String, String>>,
) -> std::io::Result<tokio::process::Child> {
    #[cfg(windows)]
    {
        fn prepare_command(
            cmd: &mut Command,
            workspace: &std::path::Path,
            command_env: Option<&HashMap<String, String>>,
        ) {
            cmd.current_dir(workspace)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .creation_flags(CREATE_NO_WINDOW);
            if let Some(env) = command_env {
                cmd.envs(env);
            }
        }

        let wrapped_command = build_powershell_command(command);
        let encoded_command = encode_powershell_command(&wrapped_command);
        let mut powershell = Command::new("powershell.exe");
        powershell.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-EncodedCommand",
            &encoded_command,
        ]);
        prepare_command(&mut powershell, workspace, command_env);

        match powershell.spawn() {
            Ok(child) => Ok(child),
            Err(ps_error) => {
                let mut cmd = Command::new("cmd.exe");
                cmd.args(["/C", command]);
                prepare_command(&mut cmd, workspace, command_env);
                cmd.spawn().map_err(|cmd_error| {
                    std::io::Error::new(
                        cmd_error.kind(),
                        format!(
                            "failed to spawn powershell.exe ({ps_error}); fallback cmd.exe also failed: {cmd_error}"
                        ),
                    )
                })
            }
        }
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(command)
            .current_dir(workspace)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(env) = command_env {
            cmd.envs(env);
        }
        cmd.spawn()
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a shell command in the workspace directory. On Windows this runs in a hidden PowerShell session, not GNU bash. Use for running commands, installing packages, and running tests."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Required. The exact shell command to execute. Always provide this exact field name: 'command'. On Windows the command must be PowerShell-compatible; the tool provides a small compatibility shim for curl, wget, bare HTTP verbs (GET/POST/PUT/PATCH/DELETE/OPTIONS), which, and head."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional. Timeout in milliseconds. Default: 120000. Values below 1000 are clamped to 1000 to avoid accidental immediate timeouts."
                }
            },
            "required": ["command"],
            "examples": [
                {
                    "command": "cargo test -p a3s-code-core skill::"
                },
                {
                    "command": "npm test",
                    "timeout": 300000
                }
            ]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("command parameter is required")),
        };

        // If a sandbox is configured, route execution through it.
        if let Some(ref sandbox) = ctx.sandbox {
            let result = sandbox
                .exec_command(command, "/workspace")
                .await
                .map_err(|e| anyhow::anyhow!("Sandbox bash execution failed: {}", e))?;

            // Combine stdout and stderr the same way the local path does.
            let mut output = result.stdout;
            if !result.stderr.is_empty() {
                output.push_str(&result.stderr);
            }

            return Ok(ToolOutput {
                content: output,
                success: result.exit_code == 0,
                metadata: Some(serde_json::json!({ "exit_code": result.exit_code })),
                images: vec![],
                error_kind: None,
            });
        }

        // Capability gating guarantees that `bash` is only registered when the
        // workspace backend provides a command runner, so this unwrap is sound.
        let runner = ctx
            .workspace_services
            .command_runner()
            .expect("bash registered without workspace command runner");
        let requested_timeout_ms = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        let timeout_ms = requested_timeout_ms.max(MIN_TIMEOUT_MS);
        let output_observer = ctx
            .event_tx
            .clone()
            .map(|tx| Arc::new(ToolEventObserver { tx }) as Arc<dyn CommandOutputObserver>);
        let result = runner
            .exec(CommandRequest {
                command: command.to_string(),
                timeout_ms,
                output_observer,
                env: ctx.command_env.clone(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Workspace bash execution failed: {}", e))?;

        if result.timed_out {
            return Ok(ToolOutput::error(format!(
                "{}\n\n[Command timed out after {}ms]",
                result.output, timeout_ms
            )));
        }

        Ok(ToolOutput {
            content: result.output,
            success: result.exit_code == 0,
            metadata: Some(serde_json::json!({ "exit_code": result.exit_code })),
            images: vec![],
            error_kind: None,
        })
    }
}

#[cfg(test)]
#[path = "bash/tests.rs"]
mod tests;
