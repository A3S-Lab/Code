//! Shared, conservative risk classification for interactive Code hosts.
//!
//! This module deliberately recognizes a small safe subset. Unknown or complex
//! invocations require confirmation; only operations with catastrophic blast
//! radius are denied outright. Hosts can layer their own mode semantics over the
//! resulting allow/ask/deny decision without duplicating command heuristics.

mod assessment;

use serde::{Deserialize, Serialize};

use super::{
    EnvironmentSensitivity, ImpactScope, OperationTarget, PermissionChecker, PermissionDecision,
    Reversibility, ToolRiskAction, ToolRiskAssessment, ToolRiskLevel, ToolRiskReason,
};
use assessment::{assess_tool, assessment_permission, critical_assessment, tool_risk_type};

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

    /// Apply the mode decision matrix to an explainable risk assessment.
    ///
    /// Routine calls are quiet in every mode. Default and plan require human
    /// confirmation for bounded mutations, while auto streamlines them. High
    /// risk is marked as a constrained-review candidate and falls back to HITL
    /// through the legacy permission interface. Critical rule denials are
    /// non-bypassable.
    pub const fn action_for(self, assessment: &ToolRiskAssessment) -> ToolRiskAction {
        match (self, assessment.level) {
            (_, ToolRiskLevel::Routine) => ToolRiskAction::Allow,
            (Self::Auto, ToolRiskLevel::Bounded) => ToolRiskAction::Allow,
            (_, ToolRiskLevel::Bounded) => ToolRiskAction::RequireConfirmation,
            (_, ToolRiskLevel::High) => ToolRiskAction::ReviewByLlm,
            (_, ToolRiskLevel::Critical) => ToolRiskAction::RuleDeny,
        }
    }

    fn apply(self, assessment: &ToolRiskAssessment) -> PermissionDecision {
        match self.action_for(assessment) {
            ToolRiskAction::Allow => PermissionDecision::Allow,
            ToolRiskAction::RequireConfirmation | ToolRiskAction::ReviewByLlm => {
                // PermissionDecision remains backward compatible. Hosts that
                // understand ToolRiskAction can distinguish human confirmation
                // from LLM review through `InteractiveToolGuardrail::assess`.
                PermissionDecision::Ask
            }
            ToolRiskAction::RuleDeny => PermissionDecision::Deny,
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

    /// Return the explainable risk assessment before host mode semantics.
    pub fn risk_assessment(tool_name: &str, args: &serde_json::Value) -> ToolRiskAssessment {
        assess_tool(tool_name, args)
    }

    /// Return the conservative legacy permission decision before mode semantics.
    ///
    /// This projection preserves existing host integrations. New hosts should
    /// consume [`Self::risk_assessment`] and the mode's decision matrix when they
    /// need to distinguish human confirmation from LLM review.
    pub fn risk_decision(tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
        assessment_permission(&assess_tool(tool_name, args))
    }

    /// Return whether an exact Bash command matches the deterministic,
    /// non-bypassable catastrophic-operation floor.
    ///
    /// Sandboxed hosts use this separately from the conservative lexical risk
    /// projection: unknown shell syntax may be safe inside an enforced OS
    /// boundary, while destructive system commands remain denied in every
    /// execution mode.
    pub fn is_catastrophic_bash_command(command: &str) -> bool {
        is_catastrophic_bash_command(command)
    }

    /// Assess an invocation, including workspace symlink boundary checks.
    pub fn assess(&self, tool_name: &str, args: &serde_json::Value) -> ToolRiskAssessment {
        if let Some(assessment) = self.workspace_boundary_assessment(tool_name, args) {
            return assessment;
        }
        assess_tool(tool_name, args)
    }

    /// Return the explicit routing action selected for this guardrail mode.
    pub fn risk_action(&self, tool_name: &str, args: &serde_json::Value) -> ToolRiskAction {
        self.mode.action_for(&self.assess(tool_name, args))
    }

    fn workspace_boundary_assessment(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Option<ToolRiskAssessment> {
        let root = self.workspace.as_deref()?;
        invocation_crosses_local_symlink(root, tool_name, args).then(|| {
            critical_assessment(
                tool_risk_type(tool_name),
                OperationTarget::OutsideWorkspace,
                ImpactScope::Host,
                Reversibility::Unknown,
                EnvironmentSensitivity::Host,
                ToolRiskReason::SymlinkBoundaryEscape,
            )
        })
    }
}

impl Default for InteractiveToolGuardrail {
    fn default() -> Self {
        Self::new(InteractiveApprovalMode::Default)
    }
}

impl PermissionChecker for InteractiveToolGuardrail {
    fn check(&self, tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
        self.mode.apply(&self.assess(tool_name, args))
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
        "read" | "write" | "edit" | "patch" | "download" => "file_path",
        "search" | "ls" | "code_symbols" | "code_navigation" | "code_diagnostics" => "path",
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
                .any(|token| shell_token_path_crosses_symlink(root, token))
        })
}

fn local_path_crosses_symlink(root: &std::path::Path, path: &str) -> bool {
    path_crosses_symlink(root, path, false)
}

fn shell_token_path_crosses_symlink(root: &std::path::Path, path: &str) -> bool {
    path_crosses_symlink(root, path, true)
}

fn path_crosses_symlink(root: &std::path::Path, path: &str, stop_at_shell_glob: bool) -> bool {
    if path_is_outside_workspace(path) {
        return false;
    }
    let mut current = root.to_path_buf();
    for component in std::path::Path::new(path).components() {
        match component {
            std::path::Component::CurDir => continue,
            std::path::Component::Normal(component) => {
                if stop_at_shell_glob
                    && component
                        .to_string_lossy()
                        .contains(['*', '?', '[', ']', '{', '}'])
                {
                    // A glob is not a literal filesystem component. Prefixes
                    // already visited above remain checked, while the lexical
                    // Bash classifier routes the unresolved expansion to HITL.
                    return false;
                }
                current.push(component);
            }
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

pub(super) fn atomic_tool_is_bounded(tool_name: &str, args: &serde_json::Value) -> bool {
    match tool_name.to_ascii_lowercase().as_str() {
        // Workspace-confined edits and ordinary structured Git changes are the
        // bounded operations that auto mode exists to streamline.
        "write" | "edit" | "patch" => bounded_file_target(args),
        // A missing destination is still bounded because download derives and
        // sanitizes a workspace-relative filename from the response metadata.
        "download" => args.get("file_path").is_none() || bounded_file_target(args),
        "git" => {
            classify_git(args) == PermissionDecision::Ask
                && git_call_is_known_bounded_mutation(args)
        }
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

pub(super) fn classify_atomic_tool(
    tool_name: &str,
    args: &serde_json::Value,
) -> PermissionDecision {
    match tool_name.to_ascii_lowercase().as_str() {
        "read" => classify_scoped_path(args, "file_path", PermissionDecision::Allow),
        "search" | "ls" | "code_symbols" | "code_navigation" | "code_diagnostics" => {
            classify_scoped_path(args, "path", PermissionDecision::Allow)
        }
        "web_search" | "web_fetch" | "search_skills" | "generate_object" => {
            PermissionDecision::Allow
        }
        "write" | "edit" => classify_scoped_path(args, "file_path", PermissionDecision::Ask),
        "download" if args.get("file_path").is_none() => PermissionDecision::Ask,
        "download" => classify_scoped_path(args, "file_path", PermissionDecision::Ask),
        // Patch carries its target in a separate top-level field. A missing or
        // boundary-crossing target must never be silently approved.
        "patch" => classify_scoped_path(args, "file_path", PermissionDecision::Ask),
        "bash" => classify_bash(args),
        "git" => classify_git(args),
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
    if shell_invokes_mkfs(command)
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

fn shell_invokes_mkfs(command: &str) -> bool {
    command
        .split(['|', ';', '&'])
        .filter_map(|segment| segment.split_whitespace().next())
        .map(clean_shell_token)
        .filter_map(|executable| executable.rsplit('/').next())
        .any(|executable| executable == "mkfs" || executable.starts_with("mkfs."))
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
                && !short_option_contains(value, 'f')
                && !matches!(
                    *value,
                    "-R" | "-r"
                        | "--recursive"
                        | "--dereference-recursive"
                        | "--include"
                        | "--exclude-from"
                        | "--file"
                )
                && !value.starts_with("--exclude-from=")
                && !value.starts_with("--file=")
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
                && !short_option_contains(value, 's')
                && !value.starts_with("--set=")
                && !option_executes_or_writes(value)
        }),
        "uname" => tokens
            .iter()
            .skip(1)
            .all(|value| !option_executes_or_writes(value)),
        "sort" => tokens.iter().skip(1).all(|value| {
            !matches!(
                *value,
                "-o" | "-T" | "--output" | "--temporary-directory" | "--compress-program"
            ) && !short_option_contains(value, 'o')
                && !short_option_contains(value, 'T')
                && !value.starts_with("--output=")
                && !value.starts_with("--temporary-directory=")
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
                    " -fls",
                    " -follow",
                    " -lname",
                ]
                .iter()
                .any(|action| lower.contains(action))
        }
        // Sed scripts can write files (`w`) or execute commands (`e`) without
        // an option-level signal. Keep them behind HITL until a real parser can
        // prove the script is read-only.
        "sed" => false,
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
