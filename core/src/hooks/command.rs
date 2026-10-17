//! Trusted command-hook discovery and execution for A3S Code.
//!
//! User and repository hook definitions are inert until their exact semantic
//! definition hash is trusted. Editing a command, matcher, event, or timeout
//! creates a new pending definition instead of inheriting old authority.
//!
//! shortcut: apps/cli/src/code_hooks.rs is still the copy `a3s code exec` compiles
//! against core 9.1.2. Delete that copy when the CLI pin exports this type.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use super::{HookEvent, HookExecutor, HookOutcome, HookResult};
use anyhow::{bail, Context};
use async_trait::async_trait;
use fs2::FileExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_TRUST_BYTES: u64 = 1024 * 1024;
const MAX_HOOKS: usize = 256;
const MAX_COMMAND_BYTES: usize = 8 * 1024;
const MAX_MATCHER_BYTES: usize = 1024;
const MAX_EVENT_INPUT_BYTES: usize = 1024 * 1024;
const MAX_COMMAND_OUTPUT_BYTES: u64 = 1024 * 1024;
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const MAX_TIMEOUT_SECONDS: u64 = 600;

/// Home used to find `~/.a3s/hooks.json`. Same order as the CLI user home.
pub fn command_hook_home() -> Option<PathBuf> {
    if let Some(home) = nonempty_env("HOME") {
        return Some(home);
    }
    #[cfg(windows)]
    {
        if let Some(profile) = nonempty_env("USERPROFILE") {
            return Some(profile);
        }
        if let (Some(drive), Some(path)) = (nonempty_env("HOMEDRIVE"), nonempty_env("HOMEPATH")) {
            let mut joined = drive.into_os_string();
            joined.push(path);
            return Some(PathBuf::from(joined));
        }
    }
    None
}

/// `<state root>/code/hooks-trust.json`, matching CLI `ComponentPaths` state root.
pub fn command_hook_trust_path(directory: &Path) -> anyhow::Result<PathBuf> {
    command_hook_trust_path_from(
        directory,
        nonempty_env("A3S_STATE_HOME").as_deref(),
        nonempty_env("HOME").as_deref(),
        nonempty_env("XDG_STATE_HOME").as_deref(),
        nonempty_env("LOCALAPPDATA").as_deref(),
    )
}

fn command_hook_trust_path_from(
    directory: &Path,
    state_home: Option<&Path>,
    home: Option<&Path>,
    xdg_state_home: Option<&Path>,
    local_app_data: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let root = if let Some(value) = state_home {
        if value.is_absolute() {
            value.to_path_buf()
        } else {
            directory.join(value)
        }
    } else {
        state_root_without_override(directory, home, xdg_state_home, local_app_data)?
    };
    Ok(root.join("code/hooks-trust.json"))
}

fn state_root_without_override(
    directory: &Path,
    home: Option<&Path>,
    xdg_state_home: Option<&Path>,
    local_app_data: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let _ = (directory, xdg_state_home, local_app_data);
        let home = home.context("A3S_STATE_HOME is not set and no home directory is available")?;
        return Ok(home.join("Library/Application Support/A3S/State"));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = local_app_data;
        if let Some(value) = xdg_state_home {
            let base = if value.is_absolute() {
                value.to_path_buf()
            } else {
                directory.join(value)
            };
            return Ok(base.join("a3s"));
        }
        let home = home.context("A3S_STATE_HOME is not set and no home directory is available")?;
        return Ok(home.join(".local/state/a3s"));
    }
    #[cfg(windows)]
    {
        let _ = (directory, home, xdg_state_home);
        let local = local_app_data
            .context("A3S_STATE_HOME is not set and no home directory is available")?;
        return Ok(local.join("A3S/State"));
    }
}

fn nonempty_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HookFile {
    #[serde(default)]
    description: Option<String>,
    hooks: BTreeMap<String, Vec<MatcherGroup>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MatcherGroup {
    #[serde(default)]
    matcher: Option<String>,
    hooks: Vec<CommandDefinition>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CommandDefinition {
    #[serde(rename = "type")]
    kind: String,
    command: String,
    #[serde(default)]
    timeout: Option<u64>,
    #[serde(default, rename = "statusMessage")]
    status_message: Option<String>,
    #[serde(default, rename = "additionalContextLimit")]
    additional_context_limit: Option<usize>,
}

#[derive(Debug, Clone)]
struct LoadedHook {
    id: String,
    event: String,
    matcher_text: Option<String>,
    matcher: Option<Regex>,
    definition: CommandDefinition,
    source: PathBuf,
    scope: &'static str,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct TrustStore {
    version: u32,
    trusted: BTreeSet<String>,
    disabled: BTreeSet<String>,
}

impl TrustStore {
    fn normalized(mut self) -> Self {
        self.version = 1;
        self
    }
}

/// Command hooks shared by an ACP session and `a3s code exec`.
pub struct CommandHookExecutor {
    workspace: PathBuf,
    home: Option<PathBuf>,
    trust_path: PathBuf,
    hooks: RwLock<Vec<LoadedHook>>,
    trust: RwLock<TrustStore>,
    diagnostics: RwLock<Vec<String>>,
}

impl std::fmt::Debug for CommandHookExecutor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommandHookExecutor")
            .field("workspace", &self.workspace)
            .field("trust_path", &self.trust_path)
            .field("hooks", &read_lock(&self.hooks).len())
            .finish_non_exhaustive()
    }
}

impl CommandHookExecutor {
    pub fn discover(
        workspace: &Path,
        home: Option<&Path>,
        trust_path: PathBuf,
    ) -> anyhow::Result<Arc<Self>> {
        let workspace = workspace
            .canonicalize()
            .with_context(|| format!("cannot resolve hook workspace {}", workspace.display()))?;
        let trust = load_trust_store(&trust_path)?;
        let (hooks, diagnostics) = discover_hooks(&workspace, home)?;
        Ok(Arc::new(Self {
            workspace,
            home: home.map(Path::to_path_buf),
            trust_path,
            hooks: RwLock::new(hooks),
            trust: RwLock::new(trust),
            diagnostics: RwLock::new(diagnostics),
        }))
    }

    pub fn discovered_and_pending(&self) -> (usize, usize) {
        let hooks = read_lock(&self.hooks);
        let trust = read_lock(&self.trust);
        let pending = hooks
            .iter()
            .filter(|hook| hook_status(hook, &trust) == "pending")
            .count();
        (hooks.len(), pending)
    }

    pub fn status_text(&self) -> String {
        let hooks = read_lock(&self.hooks);
        let trust = read_lock(&self.trust);
        let diagnostics = read_lock(&self.diagnostics);
        let mut lines = vec![format!(
            "Hooks: {} discovered · {} trusted · {} pending · {} disabled",
            hooks.len(),
            hooks
                .iter()
                .filter(|hook| hook_status(hook, &trust) == "trusted")
                .count(),
            hooks
                .iter()
                .filter(|hook| hook_status(hook, &trust) == "pending")
                .count(),
            hooks
                .iter()
                .filter(|hook| hook_status(hook, &trust) == "disabled")
                .count(),
        )];
        for hook in hooks.iter() {
            let matcher = hook
                .matcher_text
                .as_deref()
                .map(|value| format!(" matcher={value:?}"))
                .unwrap_or_default();
            lines.push(format!(
                "  {}  {:8} {:18} {}{} · {}",
                short_id(&hook.id),
                hook_status(hook, &trust),
                hook.event,
                hook.definition.command,
                matcher,
                hook.scope,
            ));
        }
        for diagnostic in diagnostics.iter() {
            lines.push(format!("  warning: {diagnostic}"));
        }
        if hooks.is_empty() {
            lines.push(
                "  Define hooks in ~/.a3s/hooks.json or <git-root>/.a3s/hooks.json.".to_string(),
            );
        } else {
            lines.push(
                "  /hooks trust <id|all> · /hooks disable <id> · /hooks enable <id> · /hooks reload"
                    .to_string(),
            );
        }
        lines.join("\n")
    }

    pub fn status_value(&self) -> Value {
        let hooks = read_lock(&self.hooks);
        let trust = read_lock(&self.trust);
        json!({
            "hooks": hooks.iter().map(|hook| json!({
                "id": hook.id,
                "shortId": short_id(&hook.id),
                "event": hook.event,
                "matcher": hook.matcher_text,
                "command": hook.definition.command,
                "source": hook.source,
                "scope": hook.scope,
                "status": hook_status(hook, &trust),
            })).collect::<Vec<_>>(),
            "diagnostics": read_lock(&self.diagnostics).clone(),
            "trustFile": self.trust_path,
        })
    }

    pub fn manage(&self, input: &str) -> anyhow::Result<String> {
        let mut parts = input.split_whitespace();
        match parts.next().unwrap_or("list").to_ascii_lowercase().as_str() {
            "list" | "status" => Ok(self.status_text()),
            "reload" => {
                if parts.next().is_some() {
                    bail!("usage: /hooks reload");
                }
                let trust = load_trust_store(&self.trust_path)?;
                let (hooks, diagnostics) = discover_hooks(&self.workspace, self.home.as_deref())?;
                *write_lock(&self.trust) = trust;
                *write_lock(&self.hooks) = hooks;
                *write_lock(&self.diagnostics) = diagnostics;
                Ok(self.status_text())
            }
            action @ ("trust" | "disable" | "enable") => {
                let id = parts.next().with_context(|| {
                    format!(
                        "usage: /hooks {action} <id{}>",
                        if action == "trust" { "|all" } else { "" }
                    )
                })?;
                if parts.next().is_some() {
                    bail!("usage: /hooks {action} <id>");
                }
                self.change_trust(action, id)?;
                Ok(self.status_text())
            }
            _ => bail!("usage: /hooks [list|reload|trust <id|all>|disable <id>|enable <id>]"),
        }
    }

    fn change_trust(&self, action: &str, requested: &str) -> anyhow::Result<()> {
        let hooks = read_lock(&self.hooks);
        let ids = if action == "trust" && requested.eq_ignore_ascii_case("all") {
            hooks.iter().map(|hook| hook.id.clone()).collect::<Vec<_>>()
        } else {
            let matches = hooks
                .iter()
                .filter(|hook| hook.id.starts_with(requested))
                .map(|hook| hook.id.clone())
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [id] => vec![id.clone()],
                [] => bail!("unknown hook id prefix {requested:?}"),
                _ => bail!("ambiguous hook id prefix {requested:?}"),
            }
        };
        drop(hooks);

        let mut local_trust = write_lock(&self.trust);
        let persisted = update_trust_store(&self.trust_path, |trust| {
            for id in ids {
                match action {
                    "trust" => {
                        trust.trusted.insert(id.clone());
                        trust.disabled.remove(&id);
                    }
                    "disable" => {
                        trust.disabled.insert(id);
                    }
                    "enable" => {
                        trust.disabled.remove(&id);
                    }
                    _ => unreachable!(),
                }
            }
        })?;
        *local_trust = persisted;
        Ok(())
    }

    async fn run_event(
        &self,
        event_name: &str,
        matcher_target: &str,
        payload: Value,
    ) -> HookOutcome {
        let trust = read_lock(&self.trust).clone();
        let mut hooks = read_lock(&self.hooks)
            .iter()
            .filter(|hook| {
                hook.event == event_name
                    && hook_status(hook, &trust) == "trusted"
                    && hook
                        .matcher
                        .as_ref()
                        .is_none_or(|matcher| matcher.is_match(matcher_target))
            })
            .cloned()
            .collect::<Vec<_>>();
        hooks.sort_by(|left, right| left.id.cmp(&right.id));
        if hooks.is_empty() {
            return HookOutcome::Continue(None);
        }

        let results = futures::future::join_all(
            hooks
                .iter()
                .map(|hook| execute_command_hook(hook, &self.workspace, event_name, &payload)),
        )
        .await;
        let mut merged = serde_json::Map::new();
        for result in results {
            match result {
                HookOutcome::Block { reason } => return HookOutcome::Block { reason },
                HookOutcome::Retry {
                    reason,
                    retry_after_ms,
                } => {
                    return HookOutcome::Retry {
                        reason,
                        retry_after_ms,
                    }
                }
                HookOutcome::Escalate { reason, target } => {
                    return HookOutcome::Escalate { reason, target }
                }
                HookOutcome::Continue(Some(Value::Object(object))) => merged.extend(object),
                HookOutcome::Continue(Some(value)) => {
                    merged.insert("value".to_string(), value);
                }
                HookOutcome::Continue(None) | HookOutcome::Skip => {}
            }
        }
        if merged.is_empty() {
            HookOutcome::Continue(None)
        } else {
            HookOutcome::Continue(Some(Value::Object(merged)))
        }
    }
}

#[async_trait]
impl HookExecutor for CommandHookExecutor {
    async fn fire(&self, event: &HookEvent) -> HookResult {
        self.fire_outcome(event).await.into()
    }

    async fn fire_outcome(&self, event: &HookEvent) -> HookOutcome {
        let (name, target, payload) = describe_hook_event(event, &self.workspace);
        self.run_event(name, &target, payload).await
    }

    async fn record_agent_event(&self, event: &crate::AgentEvent, run_id: &str, session_id: &str) {
        let (name, target): (&str, &str) = match event {
            crate::AgentEvent::SubagentStart { agent, .. } => ("SubagentStart", agent.as_str()),
            crate::AgentEvent::SubagentEnd { agent, .. } => ("SubagentStop", agent.as_str()),
            crate::AgentEvent::End { .. } => ("Stop", ""),
            _ => return,
        };
        let payload = json!({
            "hook_event_name": name,
            "session_id": session_id,
            "run_id": run_id,
            "cwd": self.workspace,
            "agent_event": event,
        });
        let _ = self.run_event(name, target, payload).await;
    }

    async fn record_run_cancelled(&self, run_id: &str, session_id: &str, reason: Option<&str>) {
        let payload = json!({
            "hook_event_name": "Stop",
            "session_id": session_id,
            "run_id": run_id,
            "cwd": self.workspace,
            "cancelled": true,
            "reason": reason,
        });
        let _ = self.run_event("Stop", "", payload).await;
    }
}

fn describe_hook_event(event: &HookEvent, workspace: &Path) -> (&'static str, String, Value) {
    let (name, target) = match event {
        HookEvent::PreToolUse(value) => ("PreToolUse", value.tool.clone()),
        HookEvent::PostToolUse(value) => ("PostToolUse", value.tool.clone()),
        HookEvent::PermissionRequest(value) => ("PermissionRequest", value.tool.clone()),
        HookEvent::PreCompact(_) => ("PreCompact", String::new()),
        HookEvent::PostCompact(_) => ("PostCompact", String::new()),
        HookEvent::PrePrompt(_) => ("UserPromptSubmit", String::new()),
        HookEvent::SessionStart(_) => ("SessionStart", String::new()),
        HookEvent::SessionEnd(_) => ("SessionEnd", String::new()),
        _ => ("A3SEvent", event.event_type().to_string()),
    };
    let mut payload = serde_json::to_value(event).unwrap_or(Value::Null);
    if let Value::Object(root) = &mut payload {
        root.insert(
            "hook_event_name".to_string(),
            Value::String(name.to_string()),
        );
        root.insert(
            "cwd".to_string(),
            Value::String(workspace.to_string_lossy().into_owned()),
        );
        let event_payload = root.get("payload").and_then(Value::as_object).cloned();
        if let Some(event_payload) = event_payload {
            if let Some(tool) = event_payload.get("tool").cloned() {
                root.insert("tool_name".to_string(), tool);
            }
            if let Some(args) = event_payload.get("args").cloned() {
                root.insert("tool_input".to_string(), args);
            }
            for (key, value) in event_payload {
                root.entry(key).or_insert(value);
            }
        }
    }
    (name, target, payload)
}

async fn execute_command_hook(
    hook: &LoadedHook,
    workspace: &Path,
    event_name: &str,
    payload: &Value,
) -> HookOutcome {
    let timeout = hook
        .definition
        .timeout
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
        .clamp(1, MAX_TIMEOUT_SECONDS);
    let mut command = platform_shell_command(&hook.definition.command);
    command
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_hook_environment(&mut command, workspace, event_name);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!(hook_id = %hook.id, %error, "Could not start trusted command hook");
            return HookOutcome::Continue(None);
        }
    };
    let input = bounded_event_input(payload);
    let stdin_task = child.stdin.take().map(|mut stdin| {
        tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
            let _ = stdin.shutdown().await;
        })
    });
    let stdout_task = child
        .stdout
        .take()
        .map(|stdout| tokio::spawn(read_bounded_output(stdout, MAX_COMMAND_OUTPUT_BYTES)));
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(read_bounded_output(stderr, MAX_COMMAND_OUTPUT_BYTES)));

    let status = match tokio::time::timeout(Duration::from_secs(timeout), child.wait()).await {
        Ok(Ok(status)) => Some(status),
        Ok(Err(error)) => {
            tracing::warn!(hook_id = %hook.id, %error, "Trusted command hook wait failed");
            None
        }
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            tracing::warn!(hook_id = %hook.id, timeout, "Trusted command hook timed out");
            None
        }
    };
    if let Some(task) = stdin_task {
        let _ = task.await;
    }
    let stdout = join_output_task(stdout_task).await;
    let stderr = join_output_task(stderr_task).await;
    let Some(status) = status else {
        return HookOutcome::Continue(None);
    };
    if status.code() == Some(2) {
        let reason = nonempty_lossy(&stderr)
            .unwrap_or_else(|| format!("Hook {} blocked {event_name}", short_id(&hook.id)));
        return HookOutcome::Block { reason };
    }
    if !status.success() {
        tracing::warn!(
            hook_id = %hook.id,
            exit_code = ?status.code(),
            stderr = %String::from_utf8_lossy(&stderr),
            "Trusted command hook failed without a blocking exit code"
        );
        return HookOutcome::Continue(None);
    }
    cap_additional_context(
        parse_hook_output(&stdout, event_name, &hook.id),
        hook.definition.additional_context_limit,
    )
}

fn parse_hook_output(stdout: &[u8], event_name: &str, hook_id: &str) -> HookOutcome {
    let text = String::from_utf8_lossy(stdout);
    let text = text.trim();
    if text.is_empty() {
        return HookOutcome::Continue(None);
    }
    let value: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(hook_id, %error, "Ignoring non-JSON command hook output");
            return HookOutcome::Continue(None);
        }
    };
    let output = value.get("hookSpecificOutput").unwrap_or(&value);
    let reason = output
        .get("reason")
        .or_else(|| output.get("permissionDecisionReason"))
        .or_else(|| value.get("stopReason"))
        .and_then(Value::as_str)
        .unwrap_or("Command hook blocked the operation")
        .to_string();
    if value.get("continue").and_then(Value::as_bool) == Some(false)
        || output
            .get("decision")
            .and_then(Value::as_str)
            .is_some_and(|decision| {
                matches!(
                    decision.to_ascii_lowercase().as_str(),
                    "block" | "deny" | "reject"
                )
            })
    {
        return HookOutcome::Block { reason };
    }
    let permission = output
        .get("permissionDecision")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if event_name == "PreToolUse"
        && matches!(
            permission.to_ascii_lowercase().as_str(),
            "deny" | "reject" | "block"
        )
    {
        return HookOutcome::Block { reason };
    }
    HookOutcome::Continue(Some(output.clone()))
}

fn cap_additional_context(outcome: HookOutcome, limit: Option<usize>) -> HookOutcome {
    let Some(limit) = limit else {
        return outcome;
    };
    let mut output = match outcome {
        HookOutcome::Continue(Some(Value::Object(output))) => output,
        other => return other,
    };
    for key in ["additionalContext", "additional_context"] {
        let Some(Value::String(value)) = output.get_mut(key) else {
            continue;
        };
        if value.len() <= limit {
            continue;
        }
        let mut end = limit.min(value.len());
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
    }
    HookOutcome::Continue(Some(Value::Object(output)))
}

fn discover_hooks(
    workspace: &Path,
    home: Option<&Path>,
) -> anyhow::Result<(Vec<LoadedHook>, Vec<String>)> {
    let mut sources = Vec::new();
    if let Some(home) = home {
        sources.push((home.join(".a3s/hooks.json"), "user"));
    }
    let project_root = find_project_root(workspace).unwrap_or_else(|| workspace.to_path_buf());
    sources.push((project_root.join(".a3s/hooks.json"), "project"));
    if project_root != workspace {
        sources.push((workspace.join(".a3s/hooks.json"), "workspace"));
    }
    sources.dedup_by(|left, right| left.0 == right.0);

    let mut loaded = Vec::new();
    let mut diagnostics = Vec::new();
    for (path, scope) in sources {
        match load_hook_file(&path, scope, MAX_HOOKS.saturating_sub(loaded.len())) {
            Ok(mut hooks) => loaded.append(&mut hooks),
            Err(error) => diagnostics.push(format!("{}: {error:#}", path.display())),
        }
        if loaded.len() >= MAX_HOOKS {
            diagnostics.push(format!("hook count was capped at {MAX_HOOKS}"));
            break;
        }
    }
    Ok((loaded, diagnostics))
}

fn load_hook_file(
    path: &Path,
    scope: &'static str,
    remaining: usize,
) -> anyhow::Result<Vec<LoadedHook>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("hook configuration must be a regular non-symlink file");
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        bail!(
            "hook configuration exceeds the {} byte limit",
            MAX_CONFIG_BYTES
        );
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let parsed: HookFile = serde_json::from_slice(&bytes).context("invalid hooks.json")?;
    let _ = parsed.description;
    let source = path.canonicalize()?;
    let mut hooks = Vec::new();
    for (event, groups) in parsed.hooks {
        let event = normalize_event_name(&event)
            .with_context(|| format!("unsupported hook event {event:?}"))?;
        for group in groups {
            if group
                .matcher
                .as_ref()
                .is_some_and(|value| value.len() > MAX_MATCHER_BYTES)
            {
                bail!("matcher exceeds the {MAX_MATCHER_BYTES} byte limit");
            }
            let matcher = group
                .matcher
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(|value| {
                    regex::RegexBuilder::new(value)
                        .size_limit(1024 * 1024)
                        .build()
                })
                .transpose()
                .context("invalid hook matcher")?;
            for definition in group.hooks {
                if hooks.len() >= remaining {
                    return Ok(hooks);
                }
                if !definition.kind.eq_ignore_ascii_case("command") {
                    bail!(
                        "unsupported hook type {:?}; only command is accepted",
                        definition.kind
                    );
                }
                if definition.command.trim().is_empty()
                    || definition.command.len() > MAX_COMMAND_BYTES
                {
                    bail!("hook command must contain 1..={MAX_COMMAND_BYTES} bytes");
                }
                if definition.timeout == Some(0)
                    || definition
                        .timeout
                        .is_some_and(|value| value > MAX_TIMEOUT_SECONDS)
                {
                    bail!("hook timeout must be between 1 and {MAX_TIMEOUT_SECONDS} seconds");
                }
                if definition
                    .additional_context_limit
                    .is_some_and(|value| value > MAX_COMMAND_OUTPUT_BYTES as usize)
                {
                    bail!(
                        "additionalContextLimit exceeds the {} byte output limit",
                        MAX_COMMAND_OUTPUT_BYTES
                    );
                }
                let digest_input = serde_json::to_vec(&json!({
                    "event": event,
                    "matcher": group.matcher,
                    "definition": definition,
                    "source": source,
                }))?;
                let id = sha256_hex(&digest_input);
                hooks.push(LoadedHook {
                    id,
                    event: event.to_string(),
                    matcher_text: group.matcher.clone(),
                    matcher: matcher.clone(),
                    definition,
                    source: source.clone(),
                    scope,
                });
            }
        }
    }
    Ok(hooks)
}

fn normalize_event_name(value: &str) -> Option<&'static str> {
    match value
        .trim()
        .to_ascii_lowercase()
        .replace(['_', '-'], "")
        .as_str()
    {
        "pretooluse" => Some("PreToolUse"),
        "permissionrequest" => Some("PermissionRequest"),
        "posttooluse" => Some("PostToolUse"),
        "precompact" => Some("PreCompact"),
        "postcompact" => Some("PostCompact"),
        "userpromptsubmit" | "preprompt" => Some("UserPromptSubmit"),
        "subagentstart" => Some("SubagentStart"),
        "subagentstop" | "subagentend" => Some("SubagentStop"),
        "stop" => Some("Stop"),
        "sessionstart" => Some("SessionStart"),
        "sessionend" => Some("SessionEnd"),
        _ => None,
    }
}

fn find_project_root(workspace: &Path) -> Option<PathBuf> {
    workspace.ancestors().take(256).find_map(|directory| {
        let marker = directory.join(".git");
        std::fs::symlink_metadata(marker)
            .ok()
            .filter(|metadata| {
                !metadata.file_type().is_symlink() && (metadata.is_dir() || metadata.is_file())
            })
            .map(|_| directory.to_path_buf())
    })
}

fn write_trust_atomic(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("hooks-trust.json");
    let temp_path = parent.join(format!(".{name}.{}.{nonce}.tmp", std::process::id()));
    let result = write_trust_temp(&temp_path, path, contents);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn write_trust_temp(temp_path: &Path, path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    #[cfg(unix)]
    let opened = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(temp_path)
    };
    #[cfg(not(unix))]
    let opened = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(temp_path);
    let mut file = opened.with_context(|| format!("failed to create {}", temp_path.display()))?;
    file.write_all(contents)?;
    file.sync_all()?;
    std::fs::rename(temp_path, path)
        .with_context(|| format!("failed to replace {}", path.display()))
}

fn load_trust_store(path: &Path) -> anyhow::Result<TrustStore> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TrustStore::default().normalized())
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_TRUST_BYTES
    {
        bail!("hook trust store must be a bounded regular non-symlink file");
    }
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice::<TrustStore>(&bytes)
        .context("invalid hook trust store")?
        .normalized())
}

fn update_trust_store(
    path: &Path,
    update: impl FnOnce(&mut TrustStore),
) -> anyhow::Result<TrustStore> {
    let parent = path.parent().context("hook trust path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let lock_path = parent.join("hooks-trust.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock.lock_exclusive()?;
    // Another CLI or a live TUI may have updated the store since this executor
    // was discovered. Re-read under the process lock before applying our
    // mutation so independent trust decisions cannot overwrite one another.
    let mut trust = load_trust_store(path)?;
    update(&mut trust);
    let bytes = serde_json::to_vec_pretty(&trust)?;
    let result = write_trust_atomic(path, &bytes);
    let _ = FileExt::unlock(&lock);
    result.map(|()| trust)
}

fn hook_status<'a>(hook: &LoadedHook, trust: &'a TrustStore) -> &'a str {
    if trust.disabled.contains(&hook.id) {
        "disabled"
    } else if trust.trusted.contains(&hook.id) {
        "trusted"
    } else {
        "pending"
    }
}

fn short_id(id: &str) -> &str {
    id.get(..12).unwrap_or(id)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn platform_shell_command(script: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut command = tokio::process::Command::new("cmd.exe");
        command.args(["/D", "/S", "/C", script]);
        command
    }
    #[cfg(not(windows))]
    {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-lc", script]);
        command
    }
}

fn configure_hook_environment(
    command: &mut tokio::process::Command,
    workspace: &Path,
    event_name: &str,
) {
    let preserved = [
        "PATH",
        "PATHEXT",
        "SystemRoot",
        "COMSPEC",
        "HOME",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "TMP",
        "TEMP",
        "LANG",
        "LC_ALL",
    ]
    .into_iter()
    .filter_map(|name| std::env::var_os(name).map(|value| (name, value)))
    .collect::<Vec<_>>();
    command.env_clear();
    command.envs(preserved);
    command.env("A3S_WORKSPACE", workspace);
    command.env("A3S_HOOK_EVENT", event_name);
}

fn bounded_event_input(payload: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(payload).unwrap_or_default();
    if bytes.len() > MAX_EVENT_INPUT_BYTES {
        bytes = serde_json::to_vec(&json!({
            "hook_event_name": payload.get("hook_event_name"),
            "session_id": payload.get("session_id"),
            "truncated": true,
            "reason": "event exceeded the command-hook input limit",
        }))
        .unwrap_or_default();
    }
    bytes.push(b'\n');
    bytes
}

async fn read_bounded_output<R: tokio::io::AsyncRead + Unpin>(reader: R, max: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = reader.take(max + 1).read_to_end(&mut bytes).await;
    if bytes.len() > max as usize {
        bytes.truncate(max as usize);
    }
    bytes
}

async fn join_output_task(task: Option<tokio::task::JoinHandle<Vec<u8>>>) -> Vec<u8> {
    match task {
        Some(task) => task.await.unwrap_or_default(),
        None => Vec::new(),
    }
}

fn nonempty_lossy(bytes: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(bytes).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn read_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn write_lock<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(root: &Path, command: &str) {
        let directory = root.join(".a3s");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("hooks.json"),
            serde_json::to_vec_pretty(&json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "^bash$",
                        "hooks": [{"type": "command", "command": command, "timeout": 15}]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn trust_path_matches_the_cli_state_root() {
        let directory = Path::new("/workspace");
        let home = Path::new("/Users/example");
        #[cfg(target_os = "macos")]
        {
            let path =
                command_hook_trust_path_from(directory, None, Some(home), None, None).unwrap();
            assert_eq!(
                path,
                home.join("Library/Application Support/A3S/State/code/hooks-trust.json")
            );
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let path =
                command_hook_trust_path_from(directory, None, Some(home), None, None).unwrap();
            assert_eq!(path, home.join(".local/state/a3s/code/hooks-trust.json"));
            let xdg = Path::new("/xdg");
            let path =
                command_hook_trust_path_from(directory, None, Some(home), Some(xdg), None).unwrap();
            assert_eq!(path, xdg.join("a3s/code/hooks-trust.json"));
        }
        #[cfg(windows)]
        {
            let local = Path::new("C:/Users/example/AppData/Local");
            let path =
                command_hook_trust_path_from(directory, None, None, None, Some(local)).unwrap();
            assert_eq!(path, local.join("A3S/State/code/hooks-trust.json"));
        }
        let override_root = Path::new("/override/state");
        let path =
            command_hook_trust_path_from(directory, Some(override_root), None, None, None).unwrap();
        assert_eq!(path, override_root.join("code/hooks-trust.json"));
        let path = command_hook_trust_path_from(
            directory,
            Some(Path::new("relative-state")),
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(path, directory.join("relative-state/code/hooks-trust.json"));
    }

    #[test]
    fn definitions_are_pending_until_trusted_and_edits_get_a_new_hash() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join(".git")).unwrap();
        write_config(workspace.path(), "echo first");
        let trust = workspace.path().join("state/hooks-trust.json");
        let executor = CommandHookExecutor::discover(workspace.path(), None, trust).unwrap();
        let first = read_lock(&executor.hooks)[0].id.clone();
        assert!(executor.status_text().contains("pending"));
        executor.change_trust("trust", short_id(&first)).unwrap();
        assert!(executor.status_text().contains("trusted"));

        write_config(workspace.path(), "echo changed");
        executor.manage("reload").unwrap();
        let second = read_lock(&executor.hooks)[0].id.clone();
        assert_ne!(first, second);
        assert_eq!(
            hook_status(&read_lock(&executor.hooks)[0], &read_lock(&executor.trust)),
            "pending"
        );
    }

    #[test]
    fn independent_executors_merge_trust_updates_under_the_file_lock() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join(".git")).unwrap();
        let directory = workspace.path().join(".a3s");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("hooks.json"),
            serde_json::to_vec_pretty(&json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "^read$",
                        "hooks": [
                            {"type": "command", "command": "echo first"},
                            {"type": "command", "command": "echo second"}
                        ]
                    }]
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let trust_path = workspace.path().join("state/hooks-trust.json");
        let first_executor =
            CommandHookExecutor::discover(workspace.path(), None, trust_path.clone()).unwrap();
        let second_executor =
            CommandHookExecutor::discover(workspace.path(), None, trust_path.clone()).unwrap();
        let ids = read_lock(&first_executor.hooks)
            .iter()
            .map(|hook| hook.id.clone())
            .collect::<Vec<_>>();

        first_executor.change_trust("trust", &ids[0]).unwrap();
        second_executor.change_trust("trust", &ids[1]).unwrap();

        let persisted = load_trust_store(&trust_path).unwrap();
        assert_eq!(persisted.trusted, ids.into_iter().collect());
    }

    #[tokio::test]
    async fn trusted_command_process_can_rewrite_tool_input() {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::create_dir(workspace.path().join(".git")).unwrap();
        let hook_directory = workspace.path().join(".a3s");
        std::fs::create_dir_all(&hook_directory).unwrap();
        #[cfg(windows)]
        let (script_name, command, script) = (
            "rewrite.ps1",
            "powershell.exe -NoProfile -NonInteractive -File .a3s/rewrite.ps1",
            "[Console]::Out.Write('{\"hookSpecificOutput\":{\"updatedInput\":{\"path\":\"safe.txt\"}}}')",
        );
        #[cfg(not(windows))]
        let (script_name, command, script) = (
            "rewrite.sh",
            "/bin/sh .a3s/rewrite.sh",
            "printf '%s' '{\"hookSpecificOutput\":{\"updatedInput\":{\"path\":\"safe.txt\"}}}'",
        );
        std::fs::write(hook_directory.join(script_name), script).unwrap();
        write_config(workspace.path(), command);

        let trust = workspace.path().join("state/hooks-trust.json");
        let executor = CommandHookExecutor::discover(workspace.path(), None, trust).unwrap();
        let id = read_lock(&executor.hooks)[0].id.clone();
        executor.change_trust("trust", short_id(&id)).unwrap();
        let outcome = executor
            .fire_outcome(&HookEvent::PreToolUse(crate::hooks::PreToolUseEvent {
                session_id: "session".to_string(),
                tool: "bash".to_string(),
                args: json!({"path": "unsafe.txt"}),
                working_directory: workspace.path().to_string_lossy().into_owned(),
                recent_tools: Vec::new(),
            }))
            .await;

        assert!(matches!(
            outcome,
            HookOutcome::Continue(Some(value)) if value["updatedInput"]["path"] == "safe.txt"
        ));
    }

    #[test]
    fn hook_output_supports_block_and_input_rewrite() {
        assert!(matches!(
            parse_hook_output(br#"{"decision":"block","reason":"no"}"#, "PreToolUse", "id"),
            HookOutcome::Block { reason } if reason == "no"
        ));
        assert!(matches!(
            parse_hook_output(
                br#"{"hookSpecificOutput":{"updatedInput":{"path":"safe"}}}"#,
                "PreToolUse",
                "id"
            ),
            HookOutcome::Continue(Some(value)) if value["updatedInput"]["path"] == "safe"
        ));
    }

    #[test]
    fn additional_context_limit_truncates_on_a_utf8_boundary() {
        let outcome = cap_additional_context(
            HookOutcome::Continue(Some(json!({"additionalContext": "ab界cd"}))),
            Some(4),
        );
        assert!(matches!(
            outcome,
            HookOutcome::Continue(Some(value)) if value["additionalContext"] == "ab"
        ));
    }

    #[test]
    fn matcher_and_event_aliases_are_bounded_and_normalized() {
        assert_eq!(
            normalize_event_name("user_prompt_submit"),
            Some("UserPromptSubmit")
        );
        assert_eq!(normalize_event_name("SubagentEnd"), Some("SubagentStop"));
        assert_eq!(normalize_event_name("unknown"), None);
    }
}
