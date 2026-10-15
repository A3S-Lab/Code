//! Installed plugin executables from native publications or legacy ACP metadata.
//!
//! Studio binds an exact native selection on `session/new`. Older hosts can
//! still supply `_meta.a3sTools`; native selection sessions refuse that path.
//! Preparing a Tool does not start it. `execute` owns the actual process.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::tools::{
    execute_owned_process, OwnedProcessTermination, Tool, ToolContext, ToolErrorKind, ToolOutput,
};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

const TOOL_LIMIT: usize = 64;
const IO_LIMIT: usize = 64 * 1024;
const RUN_TIMEOUT_MS: u64 = 5_000;
const RUN_TIMEOUT: Duration = Duration::from_millis(RUN_TIMEOUT_MS);

struct InstalledExecutable {
    name: String,
    entry: PathBuf,
    description: String,
    workspace: Option<crate::plugin_workspace::WorkspaceBinding>,
}

/// The `a3sTools` array from a decoded `session/new` meta map.
pub fn entries_from_meta(meta: Option<&serde_json::Map<String, Value>>) -> Vec<Value> {
    meta.and_then(|meta| meta.get("a3sTools"))
        .and_then(Value::as_array)
        .map(|entries| entries.iter().take(TOOL_LIMIT).cloned().collect())
        .unwrap_or_default()
}

/// Tools whose `execute` starts the absolute unpacked executable.
pub fn from_entries(entries: &[Value]) -> Vec<Arc<dyn Tool>> {
    let mut tools = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for entry in entries.iter().take(TOOL_LIMIT) {
        let Some(tool) = parse_tool(entry) else {
            continue;
        };
        if !seen.insert(tool.name.clone()) {
            continue;
        }
        tools.push(Arc::new(tool) as Arc<dyn Tool>);
    }
    tools
}

pub(crate) fn from_publication(
    id: &str,
    entry: PathBuf,
    workspace: crate::plugin_workspace::WorkspaceBinding,
) -> Result<Arc<dyn Tool>> {
    if !tool_name_ok(id) {
        anyhow::bail!("invalid native plugin Tool publication");
    }
    validate_executable(&entry)?;
    Ok(Arc::new(InstalledExecutable {
        name: id.to_owned(),
        entry,
        description: format!("Installed plugin tool {id}."),
        workspace: Some(workspace),
    }))
}

pub(crate) fn validate_executable(entry: &Path) -> Result<()> {
    if !entry.is_absolute() || !std::fs::symlink_metadata(entry)?.is_file() {
        anyhow::bail!("native executable entry is not a regular absolute file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if std::fs::symlink_metadata(entry)?.permissions().mode() & 0o100 == 0 {
            anyhow::bail!("native Tool entry is not executable; synchronize the installed package");
        }
    }
    Ok(())
}

fn parse_tool(value: &Value) -> Option<InstalledExecutable> {
    let name = value.get("id").and_then(Value::as_str)?;
    let entry = value.get("entry").and_then(Value::as_str)?;
    if !tool_name_ok(name) || !absolute_file(entry) {
        return None;
    }
    Some(InstalledExecutable {
        description: format!("Installed plugin tool {name}."),
        workspace: None,
        name: name.to_owned(),
        entry: PathBuf::from(entry),
    })
}

fn tool_name_ok(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    name.len() <= 64
        && first.is_ascii_alphanumeric()
        && chars.all(|character| {
            character.is_ascii_alphanumeric()
                || character == '-'
                || character == '_'
                || character == '.'
        })
}

fn absolute_file(entry: &str) -> bool {
    if entry.is_empty() || entry.contains('\0') {
        return false;
    }
    let path = Path::new(entry);
    path.is_absolute() && path.is_file()
}

#[async_trait]
impl Tool for InstalledExecutable {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": true
        })
    }

    fn validate_execution_available(&self, _args: &Value, ctx: &ToolContext) -> Result<()> {
        if let Some(workspace) = &self.workspace {
            workspace.validate(&ctx.workspace)?;
            // Preparation has not acquired the live Agent Turn yet. The
            // captured backend can already refuse Execute without hooks or
            // process effects; the live scope check below still owns admission.
            anyhow::ensure!(
                ctx.workspace_services.capabilities().exec,
                "Tool not found in active workspace capability scope"
            );
        }
        Ok(())
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        if let Err(error) = self.validate_execution_available(args, ctx) {
            return Ok(ToolOutput::error(error.to_string()));
        }
        let scope_cancellation = if self.workspace.is_some() {
            match ctx
                .workspace_operation(a3s_code_core::capability::WorkspaceOperation::Execute)
                .and_then(|access| access.cancellation())
            {
                Ok(cancellation) => Some(cancellation),
                Err(_) => {
                    return Ok(ToolOutput::error(
                        "Tool not found in active workspace capability scope",
                    ))
                }
            }
        } else {
            None
        };
        let payload = args.to_string();
        if payload.len() > IO_LIMIT {
            return Ok(ToolOutput::error(format!(
                "plugin tool {} arguments exceed {IO_LIMIT} bytes",
                self.name
            )));
        }
        let mut command = tokio::process::Command::new(&self.entry);
        if let Some(parent) = self.entry.parent() {
            if parent.is_dir() {
                command.current_dir(parent);
            }
        }
        let cancellation = ctx.cancellation_token().child_token();
        let process =
            execute_owned_process(&mut command, payload.as_bytes(), RUN_TIMEOUT, &cancellation);
        tokio::pin!(process);
        let result = tokio::select! {
            biased;
            _ = async {
                match scope_cancellation {
                    Some(token) => token.cancelled().await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                cancellation.cancel();
                process.await
            }
            result = &mut process => result,
        };
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                return Ok(ToolOutput::error(format!(
                    "plugin tool {} failed: {error}",
                    self.name
                )));
            }
        };
        match output.termination {
            OwnedProcessTermination::Completed => {}
            OwnedProcessTermination::Cancelled => {
                return Ok(
                    ToolOutput::error(format!("plugin tool {} cancelled", self.name))
                        .with_error_kind(ToolErrorKind::Cancelled {
                            op: self.name.clone(),
                        }),
                );
            }
            OwnedProcessTermination::TimedOut => {
                return Ok(
                    ToolOutput::error(format!("plugin tool {} timed out", self.name))
                        .with_error_kind(ToolErrorKind::Timeout {
                            op: self.name.clone(),
                            duration_ms: RUN_TIMEOUT_MS,
                        }),
                );
            }
        }
        let Some(status) = output.status else {
            return Ok(ToolOutput::error(format!(
                "plugin tool {} returned no exit status",
                self.name
            )));
        };
        if !status.success() {
            let stderr = capped_text(output.stderr.as_bytes());
            return Ok(ToolOutput::error(format!(
                "plugin tool {} exited {}: {stderr}",
                self.name, status
            )));
        }
        Ok(ToolOutput::success(capped_text(output.stdout.as_bytes())))
    }
}

#[cfg(test)]
#[path = "plugin_tools_tests.rs"]
mod tests;

fn capped_text(bytes: &[u8]) -> String {
    let end = bytes.len().min(IO_LIMIT);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}
