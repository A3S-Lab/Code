//! Shared, conservative risk classification for interactive Code hosts.
//!
//! This module deliberately recognizes a small safe subset. Unknown or complex
//! invocations require confirmation; only operations with catastrophic blast
//! radius are denied outright. Hosts can layer their own mode semantics over the
//! resulting allow/ask/deny decision without duplicating command heuristics.

use serde::{Deserialize, Serialize};

use super::{PermissionChecker, PermissionDecision};

/// How an interactive host treats operations that would normally require HITL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractiveApprovalMode {
    /// Allow known-safe operations and prompt for ordinary side effects.
    Default,
    /// Allow known-safe operations and prompt for side effects.
    Plan,
    /// Streamline bounded workspace side effects while retaining HITL elsewhere.
    Auto,
}

impl InteractiveApprovalMode {
    pub fn from_name(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "plan" => Self::Plan,
            "auto" => Self::Auto,
            _ => Self::Default,
        }
    }

    fn apply(
        self,
        tool_name: &str,
        args: &serde_json::Value,
        decision: PermissionDecision,
    ) -> PermissionDecision {
        match (self, decision) {
            (_, PermissionDecision::Deny) => PermissionDecision::Deny,
            // Planning stays risk-aware rather than hiding useful escalation:
            // side effects still require an explicit user decision.
            (Self::Plan, PermissionDecision::Ask) => PermissionDecision::Ask,
            (Self::Auto, PermissionDecision::Ask) if auto_mode_may_approve(tool_name, args) => {
                PermissionDecision::Allow
            }
            (_, decision) => decision,
        }
    }
}

/// Shared Codex-style guardrail used by the terminal and web Code products.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractiveToolGuardrail {
    mode: InteractiveApprovalMode,
    workspace: Option<std::path::PathBuf>,
}

impl InteractiveToolGuardrail {
    pub const fn new(mode: InteractiveApprovalMode) -> Self {
        Self {
            mode,
            workspace: None,
        }
    }

    pub fn for_mode(mode: &str) -> Self {
        Self::new(InteractiveApprovalMode::from_name(mode))
    }

    /// Add a local workspace root so existing symlink components can be checked.
    pub fn with_workspace(mut self, workspace: impl Into<std::path::PathBuf>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }

    /// Return the conservative lexical risk decision before host mode semantics.
    pub fn risk_decision(tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
        classify_tool(tool_name, args)
    }

    fn workspace_boundary_decision(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Option<PermissionDecision> {
        let root = self.workspace.as_deref()?;
        invocation_crosses_local_symlink(root, tool_name, args).then_some(PermissionDecision::Deny)
    }
}

impl Default for InteractiveToolGuardrail {
    fn default() -> Self {
        Self::new(InteractiveApprovalMode::Default)
    }
}

impl PermissionChecker for InteractiveToolGuardrail {
    fn check(&self, tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
        if let Some(decision) = self.workspace_boundary_decision(tool_name, args) {
            return decision;
        }
        self.mode
            .apply(tool_name, args, classify_tool(tool_name, args))
    }
}

fn invocation_crosses_local_symlink(
    root: &std::path::Path,
    tool_name: &str,
    args: &serde_json::Value,
) -> bool {
    if tool_name.eq_ignore_ascii_case("batch") {
        return args
            .get("invocations")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|invocations| {
                invocations.iter().any(|invocation| {
                    let Some(tool) = invocation.get("tool").and_then(serde_json::Value::as_str)
                    else {
                        return false;
                    };
                    let Some(tool_args) = invocation.get("args") else {
                        return false;
                    };
                    invocation_crosses_local_symlink(root, tool, tool_args)
                })
            });
    }

    let tool = tool_name.to_ascii_lowercase();
    if tool == "bash" {
        return shell_path_crosses_symlink(root, args);
    }
    let field = match tool.as_str() {
        "read" | "write" | "edit" | "patch" => "file_path",
        "grep" | "glob" | "ls" | "code_symbols" | "code_navigation" | "code_diagnostics" => "path",
        _ => return false,
    };
    let Some(path) = args.get(field).and_then(serde_json::Value::as_str) else {
        return false;
    };
    local_path_crosses_symlink(root, path)
}

fn shell_path_crosses_symlink(root: &std::path::Path, args: &serde_json::Value) -> bool {
    args.get("command")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|command| {
            command
                .split_whitespace()
                .map(clean_shell_token)
                .filter(|token| !token.is_empty() && !token.starts_with('-'))
                .any(|token| local_path_crosses_symlink(root, token))
        })
}

fn local_path_crosses_symlink(root: &std::path::Path, path: &str) -> bool {
    if path_is_outside_workspace(path) {
        return false;
    }
    let mut current = root.to_path_buf();
    for component in std::path::Path::new(path).components() {
        match component {
            std::path::Component::CurDir => continue,
            std::path::Component::Normal(component) => current.push(component),
            _ => return true,
        }
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
            Err(_) => return true,
        }
    }
    false
}

fn auto_mode_may_approve(tool_name: &str, args: &serde_json::Value) -> bool {
    match tool_name.to_ascii_lowercase().as_str() {
        // Workspace-confined edits and ordinary structured Git changes are the
        // bounded operations that auto mode exists to streamline.
        "write" | "edit" => bounded_file_target(args),
        "patch" => bounded_file_target(args),
        "git" => {
            classify_git(args) == PermissionDecision::Ask
                && git_call_is_known_bounded_mutation(args)
        }
        "batch" => batch_auto_approvable(args),
        // Shell, delegation, runtime, dynamic scripts, skills, and unknown/MCP
        // tools retain HITL because their side effects cannot be bounded here.
        _ => false,
    }
}

fn bounded_file_target(args: &serde_json::Value) -> bool {
    args.get("file_path")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|path| !path.trim().is_empty() && !path_is_outside_workspace(path))
}

fn batch_auto_approvable(args: &serde_json::Value) -> bool {
    let Some(invocations) = args
        .get("invocations")
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    !invocations.is_empty()
        && invocations.iter().all(|invocation| {
            let Some(tool) = invocation.get("tool").and_then(serde_json::Value::as_str) else {
                return false;
            };
            let Some(tool_args) = invocation.get("args") else {
                return false;
            };
            match classify_tool(tool, tool_args) {
                PermissionDecision::Deny => false,
                PermissionDecision::Allow => true,
                PermissionDecision::Ask => auto_mode_may_approve(tool, tool_args),
            }
        })
}

fn git_call_is_known_bounded_mutation(args: &serde_json::Value) -> bool {
    if git_requires_explicit_confirmation(args) {
        return false;
    }
    match args.get("command").and_then(serde_json::Value::as_str) {
        Some("branch") => valid_non_option_string(args, "name"),
        Some("checkout") => valid_non_option_string(args, "ref"),
        Some("stash") => {
            args.get("message")
                .and_then(serde_json::Value::as_str)
                .is_some()
                || args
                    .get("include_untracked")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false)
        }
        Some("remote") => args
            .get("remote_name")
            .and_then(serde_json::Value::as_str)
            .is_some(),
        Some("worktree") => matches!(
            args.get("subcommand").and_then(serde_json::Value::as_str),
            Some("add")
        ),
        _ => false,
    }
}

fn git_requires_explicit_confirmation(args: &serde_json::Value) -> bool {
    args.get("force").is_some_and(|value| value != false)
}

fn classify_tool(tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
    match tool_name.to_ascii_lowercase().as_str() {
        "read" => classify_scoped_path(args, "file_path", PermissionDecision::Allow),
        "grep" | "glob" | "ls" | "code_symbols" | "code_navigation" | "code_diagnostics" => {
            classify_scoped_path(args, "path", PermissionDecision::Allow)
        }
        "web_search" | "web_fetch" | "search_skills" | "generate_object" => {
            PermissionDecision::Allow
        }
        "write" | "edit" => classify_scoped_path(args, "file_path", PermissionDecision::Ask),
        // Patch carries its target in a separate top-level field. A missing or
        // boundary-crossing target must never be silently approved.
        "patch" => classify_scoped_path(args, "file_path", PermissionDecision::Ask),
        "bash" => classify_bash(args),
        "git" => classify_git(args),
        "batch" => classify_batch(args),
        // Delegation, scripts, skills, runtime calls, dynamic and MCP tools can
        // perform nested or external side effects, so they need authorization.
        _ => PermissionDecision::Ask,
    }
}

fn classify_scoped_path(
    args: &serde_json::Value,
    field: &str,
    safe_decision: PermissionDecision,
) -> PermissionDecision {
    let Some(path) = args.get(field).and_then(serde_json::Value::as_str) else {
        // Some read-only tools have an optional path that defaults to the
        // workspace root. A missing write target remains malformed and asks.
        return if field == "path" {
            safe_decision
        } else {
            PermissionDecision::Ask
        };
    };
    if path.trim().is_empty() {
        return if field == "path" {
            safe_decision
        } else {
            PermissionDecision::Ask
        };
    }
    if path_is_outside_workspace(path) {
        PermissionDecision::Deny
    } else {
        safe_decision
    }
}

fn classify_batch(args: &serde_json::Value) -> PermissionDecision {
    let Some(invocations) = args
        .get("invocations")
        .and_then(serde_json::Value::as_array)
    else {
        return PermissionDecision::Ask;
    };
    if invocations.is_empty() {
        return PermissionDecision::Ask;
    }

    let mut aggregate = PermissionDecision::Allow;
    for invocation in invocations {
        let Some(tool) = invocation.get("tool").and_then(serde_json::Value::as_str) else {
            return PermissionDecision::Ask;
        };
        let Some(tool_args) = invocation.get("args") else {
            return PermissionDecision::Ask;
        };
        if !tool_args.is_object() {
            return PermissionDecision::Ask;
        }
        let decision = if tool.eq_ignore_ascii_case("batch") {
            classify_batch(tool_args)
        } else {
            classify_tool(tool, tool_args)
        };
        match decision {
            PermissionDecision::Deny => return PermissionDecision::Deny,
            PermissionDecision::Ask => aggregate = PermissionDecision::Ask,
            PermissionDecision::Allow => {}
        }
    }
    aggregate
}

fn classify_git(args: &serde_json::Value) -> PermissionDecision {
    let Some(command) = args.get("command").and_then(serde_json::Value::as_str) else {
        return PermissionDecision::Ask;
    };
    if args
        .get("force")
        .is_some_and(|value| value.as_bool() != Some(false))
    {
        return PermissionDecision::Ask;
    }

    match command {
        "status" if only_git_keys(args, &["command"]) => PermissionDecision::Allow,
        "log"
            if only_git_keys(args, &["command", "limit", "max_count", "cursor"])
                && valid_optional_positive_integer(args, "limit")
                && valid_optional_positive_integer(args, "max_count")
                && valid_optional_string(args, "cursor") =>
        {
            PermissionDecision::Allow
        }
        "diff"
            if only_git_keys(args, &["command", "target", "byte_offset", "max_bytes"])
                && valid_optional_non_option_string(args, "target")
                && valid_optional_nonnegative_integer(args, "byte_offset")
                && valid_optional_positive_integer(args, "max_bytes") =>
        {
            PermissionDecision::Allow
        }
        "remote"
            if only_git_keys(args, &["command", "remote_name", "cursor"])
                && valid_optional_string(args, "remote_name")
                && valid_optional_string(args, "cursor") =>
        {
            PermissionDecision::Allow
        }
        "branch"
            if args.get("name").is_none()
                && only_git_keys(args, &["command", "limit", "max_count", "cursor"])
                && valid_optional_positive_integer(args, "limit")
                && valid_optional_positive_integer(args, "max_count")
                && valid_optional_string(args, "cursor") =>
        {
            PermissionDecision::Allow
        }
        "stash"
            if args.get("message").is_none()
                && args.get("include_untracked").is_none()
                && only_git_keys(args, &["command", "cursor"])
                && valid_optional_string(args, "cursor") =>
        {
            PermissionDecision::Allow
        }
        "worktree"
            if args
                .get("subcommand")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("list")
                == "list"
                && only_git_keys(args, &["command", "subcommand", "cursor"])
                && valid_optional_string(args, "subcommand")
                && valid_optional_string(args, "cursor") =>
        {
            PermissionDecision::Allow
        }
        _ => PermissionDecision::Ask,
    }
}

fn only_git_keys(args: &serde_json::Value, allowed: &[&str]) -> bool {
    args.as_object().is_some_and(|object| {
        object
            .keys()
            .all(|key| allowed.iter().any(|allowed| key == allowed))
    })
}

fn valid_non_option_string(args: &serde_json::Value, field: &str) -> bool {
    args.get(field)
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| {
            let value = value.trim();
            !value.is_empty() && !value.starts_with('-')
        })
}

fn valid_optional_non_option_string(args: &serde_json::Value, field: &str) -> bool {
    args.get(field)
        .is_none_or(|_| valid_non_option_string(args, field))
}

fn valid_optional_string(args: &serde_json::Value, field: &str) -> bool {
    args.get(field).is_none_or(serde_json::Value::is_string)
}

fn valid_optional_positive_integer(args: &serde_json::Value, field: &str) -> bool {
    args.get(field)
        .is_none_or(|value| value.as_u64().is_some_and(|number| number > 0))
}

fn valid_optional_nonnegative_integer(args: &serde_json::Value, field: &str) -> bool {
    args.get(field).is_none_or(|value| value.as_u64().is_some())
}

fn classify_bash(args: &serde_json::Value) -> PermissionDecision {
    let Some(command) = args.get("command").and_then(serde_json::Value::as_str) else {
        return PermissionDecision::Ask;
    };
    let command = command.trim();
    if command.is_empty() {
        return PermissionDecision::Ask;
    }
    if is_catastrophic_bash_command(command) {
        PermissionDecision::Deny
    } else if is_read_only_bash_command(command) {
        PermissionDecision::Allow
    } else {
        PermissionDecision::Ask
    }
}

fn is_catastrophic_bash_command(command: &str) -> bool {
    let lower = normalize_shell(command).to_ascii_lowercase();
    if lower == "sudo"
        || lower.starts_with("sudo ")
        || lower.starts_with("doas ")
        || lower == "su"
        || lower.starts_with("su ")
        || lower.starts_with("su -")
    {
        return true;
    }
    if lower.contains("mkfs")
        || lower.contains("diskutil erase")
        || lower.contains(":(){")
        || lower.contains("kill -9 -1")
        || lower.starts_with("shutdown")
        || lower.starts_with("reboot")
    {
        return true;
    }
    if (lower.contains("curl ") || lower.contains("wget "))
        && ["| sh", "|sh", "| bash", "|bash", "| zsh", "|zsh"]
            .iter()
            .any(|pipe| lower.contains(pipe))
    {
        return true;
    }
    if (lower.starts_with("dd ") || lower.contains(" dd "))
        && (lower.contains(" of=/dev/") || lower.contains("of=/dev/"))
    {
        return true;
    }

    lower.contains("rm -rf /")
        || lower.contains("rm -fr /")
        || lower.contains("rm -rf ~")
        || lower.contains("rm -fr ~")
        || lower.contains("rm -rf $home")
        || lower.contains("rm -fr $home")
        || lower.contains("rm -rf *")
        || lower.contains("rm -fr *")
        || lower == "rm -rf ."
        || lower == "rm -fr ."
}

fn is_read_only_bash_command(command: &str) -> bool {
    // The allow-list intentionally rejects shell quoting, expansion, globs, and
    // non-space control whitespace. A tokenizer-aware sandbox can broaden this
    // later; a string heuristic must fail closed.
    if command
        .chars()
        .any(|character| character.is_whitespace() && character != ' ')
        || command.contains(['\'', '"', '*', '?', '[', ']', '{', '}'])
        || contains_unsafe_shell_syntax(command)
    {
        return false;
    }
    command
        .split('|')
        .all(|segment| is_read_only_bash_segment(segment.trim()))
}

fn contains_unsafe_shell_syntax(command: &str) -> bool {
    command.contains("&&")
        || command.contains("||")
        || command.contains(';')
        || command.contains('>')
        || command.contains('<')
        || command.contains('`')
        || command.contains("$(")
        || command.contains('&')
        || command.contains('\n')
        || command.contains('\r')
        || command.contains('$')
        || has_unscoped_path_token(command)
}

fn has_unscoped_path_token(command: &str) -> bool {
    command
        .split_whitespace()
        .map(clean_shell_token)
        .filter(|token| !token.is_empty())
        .any(path_is_outside_workspace)
}

fn clean_shell_token(token: &str) -> &str {
    token.trim_matches(|character: char| {
        matches!(
            character,
            '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ':'
        )
    })
}

fn path_is_outside_workspace(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let path = normalized.trim();
    let bytes = path.as_bytes();
    if (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || path.starts_with("//")
        || path.starts_with('/')
        || path.starts_with('~')
        || path.starts_with("$HOME")
        || path.starts_with("${HOME}")
    {
        return true;
    }

    let mut depth = 0_i32;
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." if depth == 0 => return true,
            ".." => depth -= 1,
            _ => depth += 1,
        }
    }
    false
}

fn is_read_only_bash_segment(segment: &str) -> bool {
    let tokens: Vec<&str> = segment.split_whitespace().collect();
    let Some(command) = tokens.first().copied().map(clean_shell_token) else {
        return false;
    };
    let lower = segment.to_ascii_lowercase();

    match command {
        "pwd" | "cat" | "head" | "tail" | "wc" | "stat" | "file" | "cut" | "tr" | "whoami" => {
            tokens
                .iter()
                .skip(1)
                .all(|value| !option_executes_or_writes(value))
        }
        "ls" => tokens.iter().skip(1).all(|value| {
            !option_executes_or_writes(value)
                && !short_option_contains(value, 'L')
                && !short_option_contains(value, 'R')
                && !matches!(*value, "--dereference" | "--recursive")
        }),
        "rg" => tokens.iter().skip(1).all(|value| {
            !option_executes_or_writes(value)
                && !matches!(*value, "--pre" | "--hostname-bin" | "-L" | "--follow")
                && !value.starts_with("--pre=")
                && !value.starts_with("--hostname-bin=")
        }),
        "grep" => tokens.iter().skip(1).all(|value| {
            !option_executes_or_writes(value)
                && !matches!(
                    *value,
                    "-R" | "-r"
                        | "--recursive"
                        | "--dereference-recursive"
                        | "--include"
                        | "--exclude-from"
                )
        }),
        "du" => tokens.iter().skip(1).all(|value| {
            !short_option_contains(value, 'L')
                && !matches!(*value, "--dereference")
                && !option_executes_or_writes(value)
        }),
        "df" => tokens
            .iter()
            .skip(1)
            .all(|value| !option_executes_or_writes(value)),
        "date" => tokens.iter().skip(1).all(|value| {
            !matches!(*value, "-s" | "--set")
                && !value.starts_with("--set=")
                && !option_executes_or_writes(value)
        }),
        "uname" => tokens
            .iter()
            .skip(1)
            .all(|value| !option_executes_or_writes(value)),
        "sort" => tokens.iter().skip(1).all(|value| {
            !matches!(*value, "-o" | "--output" | "--compress-program")
                && !value.starts_with("--output=")
                && !value.starts_with("--compress-program=")
                && !option_executes_or_writes(value)
        }),
        // uniq writes when a second positional operand is present. Conservatively
        // allow only options and at most one positional input.
        "uniq" => positional_argument_count(&tokens[1..]) <= 1,
        // Keep only plain formatting output in the silent subset. Shell
        // builtins can still carry surprising option semantics, so options ask.
        "printf" | "echo" => tokens.iter().skip(1).all(|value| !value.starts_with('-')),
        "find" => {
            !tokens
                .iter()
                .skip(1)
                .any(|value| matches!(*value, "-L" | "-H"))
                && ![
                    " -delete",
                    " -exec",
                    " -execdir",
                    " -ok",
                    " -okdir",
                    " -fprint",
                    " -fprint0",
                    " -fprintf",
                    " -follow",
                    " -lname",
                ]
                .iter()
                .any(|action| lower.contains(action))
        }
        "sed" => !tokens.iter().skip(1).any(|value| {
            *value == "-i"
                || value.starts_with("-i")
                || value.starts_with("--in-place")
                || option_executes_or_writes(value)
        }),
        "git" => is_read_only_git_segment(&tokens),
        _ => false,
    }
}

fn short_option_contains(value: &str, flag: char) -> bool {
    value.starts_with('-')
        && !value.starts_with("--")
        && value.chars().skip(1).any(|candidate| candidate == flag)
}

fn option_executes_or_writes(value: &str) -> bool {
    matches!(
        value,
        "--output" | "--exec" | "--command" | "--config" | "--files-from"
    ) || value.starts_with("--output=")
        || value.starts_with("--exec=")
        || value.starts_with("--command=")
        || value.starts_with("--config=")
        || value.starts_with("--files-from=")
}

fn positional_argument_count(tokens: &[&str]) -> usize {
    tokens
        .iter()
        .filter(|value| !value.starts_with('-'))
        .count()
}

fn is_read_only_git_segment(tokens: &[&str]) -> bool {
    if tokens.first().copied() != Some("git") {
        return false;
    }
    let mut index = 1;
    while index < tokens.len() {
        match tokens[index] {
            "--no-pager" | "-P" | "--no-optional-locks" => index += 1,
            // `-C` changes the filesystem boundary and is therefore never in
            // the silent allow-list. Other global config/execution options are
            // likewise left to confirmation.
            value if value.starts_with('-') => return false,
            _ => break,
        }
    }

    let Some(subcommand) = tokens.get(index).copied() else {
        return false;
    };
    let args = &tokens[index + 1..];
    if args.iter().any(|value| {
        matches!(
            *value,
            "--ext-diff" | "--textconv" | "--exec-path" | "--config-env"
        ) || value.starts_with("--exec-path=")
            || value.starts_with("--config-env=")
    }) {
        return false;
    }

    match subcommand {
        "status" | "diff" | "log" | "show" | "blame" | "grep" | "ls-files" | "rev-parse" => {
            !args.iter().any(|value| {
                option_executes_or_writes(value)
                    || matches!(*value, "--paginate" | "-p" | "--ext-diff" | "--textconv")
                    || value.starts_with("--format=") && value.contains("%(rest)")
            })
        }
        "remote" => match args.first() {
            Some(value) => matches!(*value, "-v" | "show"),
            None => true,
        },
        "branch" => args.iter().all(|value| {
            matches!(
                *value,
                "--all" | "-a" | "--list" | "--show-current" | "--verbose" | "-v" | "-vv"
            )
        }),
        _ => false,
    }
}

fn normalize_shell(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}
