//! Bash tool - Execute shell commands

use crate::tools::types::{
    Tool, ToolContext, ToolErrorKind, ToolEventSender, ToolOutput, ToolStreamEvent,
};
use crate::workspace::{CommandOutputObserver, CommandOutputSummary, CommandRequest};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
#[cfg(windows)]
use std::ffi::OsStr;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use tokio::process::Command;

#[cfg(windows)]
pub(crate) mod windows;
#[cfg(windows)]
pub(crate) use windows::maybe_execute_simple_windows_http_command;
#[cfg(windows)]
pub(crate) use windows::{build_powershell_command, encode_powershell_command, CREATE_NO_WINDOW};
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
    tx: Option<ToolEventSender>,
    summary: Mutex<Option<CommandOutputSummary>>,
}

#[async_trait]
impl CommandOutputObserver for ToolEventObserver {
    async fn on_output_delta(&self, delta: &str) {
        if let Some(tx) = &self.tx {
            tx.send(ToolStreamEvent::OutputDelta(delta.to_string()))
                .await
                .ok();
        }
    }

    async fn on_output_complete(&self, summary: &CommandOutputSummary) {
        *self.summary.lock().unwrap() = Some(*summary);
    }
}

pub struct BashTool;

#[cfg(windows)]
fn prepare_windows_command(
    command: &mut Command,
    workspace: &std::path::Path,
    command_env: Option<&HashMap<String, String>>,
) {
    command
        .current_dir(workspace)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .creation_flags(CREATE_NO_WINDOW);
    if let Some(env) = command_env {
        command.envs(env);
    }
}

#[cfg(windows)]
fn spawn_windows_shell(
    powershell_program: &OsStr,
    command: &str,
    workspace: &std::path::Path,
    command_env: Option<&HashMap<String, String>>,
) -> std::io::Result<tokio::process::Child> {
    let wrapped_command = build_powershell_command(command);
    let encoded_command = encode_powershell_command(&wrapped_command);
    let mut powershell = Command::new(powershell_program);
    powershell.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-EncodedCommand",
        &encoded_command,
    ]);
    prepare_windows_command(&mut powershell, workspace, command_env);

    powershell.spawn().map_err(|source| {
        std::io::Error::new(
            source.kind(),
            format!(
                "failed to spawn PowerShell executable {powershell_program:?}: {source}; refusing to reinterpret the command with another shell"
            ),
        )
    })
}

/// Spawn a shell command cross-platform.
///
/// - Unix: `bash -c <command>`
/// - Windows: `powershell.exe -EncodedCommand <command>` with hidden console window.
///   Startup failures are returned without reinterpreting the command as cmd.
pub(crate) fn spawn_shell(
    command: &str,
    workspace: &std::path::Path,
    command_env: Option<&HashMap<String, String>>,
) -> std::io::Result<tokio::process::Child> {
    #[cfg(windows)]
    {
        spawn_windows_shell(
            OsStr::new("powershell.exe"),
            command,
            workspace,
            command_env,
        )
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
        crate::tools::process::configure_process_group(&mut cmd);
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
                },
                "sandbox_permissions": {
                    "type": "string",
                    "enum": ["use_default", "require_escalated"],
                    "default": "use_default",
                    "description": "Execution boundary. Omit or use 'use_default' for the configured workspace sandbox. Use 'require_escalated' only after a sandbox denial when host execution is necessary; interactive hosts must authorize that request."
                },
                "justification": {
                    "type": "string",
                    "description": "Required with sandbox_permissions='require_escalated'. Briefly explain why the command cannot run inside the workspace sandbox."
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

    fn requires_confirmation(&self, args: &serde_json::Value) -> bool {
        args.get("sandbox_permissions")
            .and_then(serde_json::Value::as_str)
            == Some("require_escalated")
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let command = match args.get("command").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => return Ok(ToolOutput::error("command parameter is required")),
        };
        let require_escalated = match args
            .get("sandbox_permissions")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("use_default")
        {
            "use_default" => false,
            "require_escalated" => true,
            value => {
                return Ok(ToolOutput::error(format!(
                    "unsupported sandbox_permissions value: {value}"
                )))
            }
        };
        if require_escalated
            && args
                .get("justification")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|value| value.trim().is_empty())
        {
            return Ok(ToolOutput::error(
                "justification is required when sandbox_permissions is require_escalated",
            ));
        }

        let requested_timeout_ms = args
            .get("timeout")
            .and_then(|v| v.as_u64())
            .unwrap_or(DEFAULT_TIMEOUT_MS);
        let timeout_ms = requested_timeout_ms.max(MIN_TIMEOUT_MS);
        let event_observer = Arc::new(ToolEventObserver {
            tx: ctx.event_tx.clone(),
            summary: Mutex::new(None),
        });
        let output_observer = Some(Arc::clone(&event_observer) as Arc<dyn CommandOutputObserver>);

        // The normal path uses the configured workspace sandbox. An explicit
        // escalation is intentionally routed to the host runner only after the
        // session permission layer authorizes the exact invocation.
        if !require_escalated {
            if let Some(ref sandbox) = ctx.sandbox {
                let execution = sandbox.exec(crate::sandbox::SandboxCommandRequest {
                    command: command.to_string(),
                    guest_workspace: "/workspace".to_string(),
                    timeout_ms,
                    output_observer: output_observer.clone(),
                    env: ctx.command_env.clone(),
                });
                let result = match tokio::time::timeout(
                    std::time::Duration::from_millis(timeout_ms),
                    execution,
                )
                .await
                {
                    Ok(result) => result
                        .map_err(|e| anyhow::anyhow!("Sandbox bash execution failed: {}", e))?,
                    Err(_) => {
                        let capture_summary = *event_observer.summary.lock().unwrap();
                        let capture_metadata = capture_summary.map(|summary| {
                            serde_json::json!({
                                "total_bytes": summary.total_bytes,
                                "captured_bytes": summary.captured_bytes,
                                "truncated": summary.truncated,
                                "timed_out": true,
                            })
                        });
                        let mut timed_out = ToolOutput::error(format!(
                            "[Command timed out after {}ms]",
                            timeout_ms
                        ));
                        timed_out.metadata = Some(serde_json::json!({
                            "exit_code": null,
                            "timeout_ms": timeout_ms,
                            "sandboxed": true,
                            "output": capture_metadata,
                        }));
                        timed_out.error_kind = Some(ToolErrorKind::Timeout {
                            op: "bash".to_string(),
                            duration_ms: timeout_ms,
                        });
                        return Ok(timed_out);
                    }
                };

                // Combine stdout and stderr the same way the local path does.
                let mut output = result.stdout;
                if !result.stderr.is_empty() {
                    output.push_str(&result.stderr);
                }

                let capture_summary = *event_observer.summary.lock().unwrap();
                let capture_metadata = capture_summary.map(|summary| {
                    serde_json::json!({
                        "total_bytes": summary.total_bytes,
                        "captured_bytes": summary.captured_bytes,
                        "truncated": summary.truncated,
                        "timed_out": summary.timed_out,
                    })
                });
                if result.timed_out {
                    let mut timed_out = ToolOutput::error(format!(
                        "{}\n\n[Command timed out after {}ms]",
                        output, timeout_ms
                    ));
                    timed_out.metadata = Some(serde_json::json!({
                        "exit_code": result.exit_code,
                        "timeout_ms": timeout_ms,
                        "sandboxed": true,
                        "output": capture_metadata,
                    }));
                    timed_out.error_kind = Some(ToolErrorKind::Timeout {
                        op: "bash".to_string(),
                        duration_ms: timeout_ms,
                    });
                    return Ok(timed_out);
                }

                return Ok(ToolOutput {
                    content: output,
                    success: result.exit_code == 0,
                    metadata: Some(serde_json::json!({
                        "exit_code": result.exit_code,
                        "sandboxed": true,
                        "output": capture_metadata,
                    })),
                    images: vec![],
                    error_kind: None,
                });
            }
        }

        // Capability gating guarantees that `bash` is only registered when the
        // workspace backend provides a command runner, so this unwrap is sound.
        let runner = ctx
            .workspace_services
            .command_runner()
            .expect("bash registered without workspace command runner");
        let result = runner
            .exec(CommandRequest {
                command: command.to_string(),
                timeout_ms,
                output_observer,
                env: ctx.command_env.clone(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("Workspace bash execution failed: {}", e))?;

        let capture_summary = *event_observer.summary.lock().unwrap();
        let capture_metadata = capture_summary.map(|summary| {
            serde_json::json!({
                "total_bytes": summary.total_bytes,
                "captured_bytes": summary.captured_bytes,
                "truncated": summary.truncated,
                "timed_out": summary.timed_out,
            })
        });

        if result.timed_out {
            let mut output = ToolOutput::error(format!(
                "{}\n\n[Command timed out after {}ms]",
                result.output, timeout_ms
            ));
            output.metadata = Some(serde_json::json!({
                "exit_code": result.exit_code,
                "timeout_ms": timeout_ms,
                "sandboxed": false,
                "output": capture_metadata,
            }));
            output.error_kind = Some(ToolErrorKind::Timeout {
                op: "bash".to_string(),
                duration_ms: timeout_ms,
            });
            return Ok(output);
        }

        Ok(ToolOutput {
            content: result.output,
            success: result.exit_code == 0,
            metadata: Some(serde_json::json!({
                "exit_code": result.exit_code,
                "sandboxed": false,
                "output": capture_metadata,
            })),
            images: vec![],
            error_kind: None,
        })
    }
}

#[cfg(test)]
#[path = "bash/tests.rs"]
mod tests;
