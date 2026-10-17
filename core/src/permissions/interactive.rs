//! Shared, conservative risk classification for interactive Code hosts.
//!
//! This module deliberately recognizes a small safe subset. Unknown or complex
//! invocations require confirmation; only operations with catastrophic blast
//! radius are denied outright. Hosts can layer their own mode semantics over the
//! resulting allow/ask/deny decision without duplicating command heuristics.

mod assessment;

use serde::{Deserialize, Serialize};

use super::{
    shell, EnvironmentSensitivity, ImpactScope, OperationTarget, PermissionChecker,
    PermissionDecision, Reversibility, ToolRiskAction, ToolRiskAssessment, ToolRiskLevel,
    ToolRiskReason,
};
use assessment::{assess_tool, assessment_permission, critical_assessment, tool_risk_type};

/// How an interactive host treats operations that would normally require HITL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractiveApprovalMode {
    /// Allow known-safe operations and prompt for ordinary side effects.
    Default,
    /// Allow known-safe reads and deny workspace mutations. Not an alias of default.
    Plan,
    /// Streamline bounded workspace side effects while retaining HITL elsewhere.
    Auto,
    /// Cursor-like `--force`/`--yolo`: allow high-risk review candidates without
    /// HITL. Critical rule denials remain non-bypassable.
    Force,
}

impl InteractiveApprovalMode {
    /// Parse the closed set of interactive approval modes. An unknown name
    /// is REFUSED with the set named — never silently coerced: a typo of
    /// "plan" must not widen plan's denials into default's confirmations.
    pub fn parse(value: &str) -> anyhow::Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "default" => Ok(Self::Default),
            "plan" => Ok(Self::Plan),
            "auto" => Ok(Self::Auto),
            "force" | "yolo" => Ok(Self::Force),
            other => anyhow::bail!(
                "interactive approval mode must be one of the closed set \
                 [default, plan, auto, force, yolo]; got {other:?}"
            ),
        }
    }

    /// The canonical name (`"yolo"` parses to `Force` but prints `"force"`).
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
            Self::Auto => "auto",
            Self::Force => "force",
        }
    }

    /// Apply the mode decision matrix to an explainable risk assessment.
    ///
    /// Routine calls are quiet in every mode. Plan denies bounded and high
    /// mutations. Default requires confirmation for them, while auto
    /// streamlines bounded mutations. Force also streamlines high-risk review
    /// candidates. Critical rule denials are non-bypassable in every mode.
    pub const fn action_for(self, assessment: &ToolRiskAssessment) -> ToolRiskAction {
        match (self, assessment.level) {
            (_, ToolRiskLevel::Routine) => ToolRiskAction::Allow,
            (Self::Plan, ToolRiskLevel::Bounded | ToolRiskLevel::High) => ToolRiskAction::RuleDeny,
            (Self::Auto | Self::Force, ToolRiskLevel::Bounded) => ToolRiskAction::Allow,
            (_, ToolRiskLevel::Bounded) => ToolRiskAction::RequireConfirmation,
            (Self::Force, ToolRiskLevel::High) => ToolRiskAction::Allow,
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

    pub fn for_mode(mode: &str) -> anyhow::Result<Self> {
        Ok(Self::new(InteractiveApprovalMode::parse(mode)?))
    }

    /// Add a local workspace root so absolute in-workspace paths are admitted
    /// and existing symlink components can be checked.
    pub fn with_workspace(mut self, workspace: impl Into<std::path::PathBuf>) -> Self {
        self.workspace = Some(workspace.into());
        self
    }

    /// Return the explainable risk assessment before host mode semantics.
    ///
    /// Static callers have no workspace root, so absolute paths fail closed.
    /// Prefer [`Self::assess`] when a workspace is known.
    pub fn risk_assessment(tool_name: &str, args: &serde_json::Value) -> ToolRiskAssessment {
        assess_tool(tool_name, args, None)
    }

    /// Return the conservative legacy permission decision before mode semantics.
    ///
    /// This projection preserves existing host integrations. New hosts should
    /// consume [`Self::risk_assessment`] and the mode's decision matrix when they
    /// need to distinguish human confirmation from LLM review.
    ///
    /// Static callers have no workspace root, so absolute paths fail closed.
    /// Prefer [`Self::check`] / [`Self::assess`] when a workspace is known.
    pub fn risk_decision(tool_name: &str, args: &serde_json::Value) -> PermissionDecision {
        assessment_permission(&assess_tool(tool_name, args, None))
    }

    /// Return whether an exact Bash command matches the deterministic,
    /// non-bypassable catastrophic-operation floor.
    ///
    /// Sandboxed hosts use this separately from the conservative lexical risk
    /// projection: unknown shell syntax may be safe inside an enforced OS
    /// boundary, while destructive system commands remain denied in every
    /// execution mode.
    pub fn is_catastrophic_bash_command(command: &str) -> bool {
        shell::analyze(command).risk().is_hard_denial()
    }

    /// Assess an invocation, including workspace path and symlink boundary checks.
    pub fn assess(&self, tool_name: &str, args: &serde_json::Value) -> ToolRiskAssessment {
        if let Some(assessment) = self.workspace_boundary_assessment(tool_name, args) {
            return assessment;
        }
        assess_tool(tool_name, args, self.workspace.as_deref())
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
    if tool == "read" {
        if let Some(path) = args.get("file_path").and_then(serde_json::Value::as_str) {
            return local_path_crosses_symlink(root, path);
        }
        return args
            .get("files")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|files| {
                files.iter().any(|entry| {
                    entry
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|path| local_path_crosses_symlink(root, path))
                })
            });
    }
    let field = match tool.as_str() {
        "write" | "edit" | "patch" | "download" => "file_path",
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
    if path_is_outside_workspace(path, Some(root)) {
        return false;
    }
    let Some(relative) = workspace_relative_path(root, path) else {
        return true;
    };
    let mut current = root.to_path_buf();
    for component in std::path::Path::new(&relative).components() {
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

pub(super) fn atomic_tool_is_bounded(
    tool_name: &str,
    args: &serde_json::Value,
    workspace: Option<&std::path::Path>,
) -> bool {
    match tool_name.to_ascii_lowercase().as_str() {
        // Workspace-confined edits and ordinary structured Git changes are the
        // bounded operations that auto mode exists to streamline.
        "write" | "edit" | "patch" => bounded_file_target(args, workspace),
        // A missing destination is still bounded because download derives and
        // sanitizes a workspace-relative filename from the response metadata.
        "download" => args.get("file_path").is_none() || bounded_file_target(args, workspace),
        "git" => {
            classify_git(args) == PermissionDecision::Ask
                && git_call_is_known_bounded_mutation(args)
        }
        // Shell, delegation, runtime, dynamic scripts, skills, and unknown/MCP
        // tools retain HITL because their side effects cannot be bounded here.
        _ => false,
    }
}

fn bounded_file_target(args: &serde_json::Value, workspace: Option<&std::path::Path>) -> bool {
    args.get("file_path")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|path| !path.trim().is_empty() && !path_is_outside_workspace(path, workspace))
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
    workspace: Option<&std::path::Path>,
) -> PermissionDecision {
    match tool_name.to_ascii_lowercase().as_str() {
        "read" => classify_read(args, workspace),
        "search" | "ls" | "code_symbols" | "code_navigation" | "code_diagnostics" => {
            classify_scoped_path(args, "path", PermissionDecision::Allow, workspace)
        }
        "web_search" | "web_fetch" | "search_skills" | "generate_object" | "update_plan"
        | "ask_user" | "ask_user_form" => PermissionDecision::Allow,
        "write" | "edit" => {
            classify_scoped_path(args, "file_path", PermissionDecision::Ask, workspace)
        }
        "download" if args.get("file_path").is_none() => PermissionDecision::Ask,
        "download" => classify_scoped_path(args, "file_path", PermissionDecision::Ask, workspace),
        // Patch carries its target in a separate top-level field. A missing or
        // boundary-crossing target must never be silently approved.
        "patch" => classify_scoped_path(args, "file_path", PermissionDecision::Ask, workspace),
        "bash" => classify_bash(args, workspace),
        "git" => classify_git(args),
        // Delegation, scripts, skills, runtime calls, dynamic and MCP tools can
        // perform nested or external side effects, so they need authorization.
        _ => PermissionDecision::Ask,
    }
}

fn classify_read(
    args: &serde_json::Value,
    workspace: Option<&std::path::Path>,
) -> PermissionDecision {
    if let Some(path) = args.get("file_path").and_then(serde_json::Value::as_str) {
        return classify_path_value(path, PermissionDecision::Allow, workspace);
    }
    let Some(files) = args.get("files").and_then(serde_json::Value::as_array) else {
        return PermissionDecision::Ask;
    };
    if files.is_empty() {
        return PermissionDecision::Ask;
    }
    let mut decision = PermissionDecision::Allow;
    for entry in files {
        let Some(path) = entry.get("path").and_then(serde_json::Value::as_str) else {
            return PermissionDecision::Ask;
        };
        decision = stricter_permission(
            decision,
            classify_path_value(path, PermissionDecision::Allow, workspace),
        );
    }
    decision
}

fn stricter_permission(left: PermissionDecision, right: PermissionDecision) -> PermissionDecision {
    use PermissionDecision::*;
    match (left, right) {
        (Deny, _) | (_, Deny) => Deny,
        (Ask, _) | (_, Ask) => Ask,
        (Allow, Allow) => Allow,
    }
}

fn classify_scoped_path(
    args: &serde_json::Value,
    field: &str,
    safe_decision: PermissionDecision,
    workspace: Option<&std::path::Path>,
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
    classify_path_value(path, safe_decision, workspace)
}

fn classify_path_value(
    path: &str,
    safe_decision: PermissionDecision,
    workspace: Option<&std::path::Path>,
) -> PermissionDecision {
    if path.trim().is_empty() {
        return PermissionDecision::Ask;
    }
    if path_is_outside_workspace(path, workspace) {
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

fn classify_bash(
    args: &serde_json::Value,
    workspace: Option<&std::path::Path>,
) -> PermissionDecision {
    let Some(command) = args.get("command").and_then(serde_json::Value::as_str) else {
        return PermissionDecision::Ask;
    };
    let command = command.trim();
    if command.is_empty() {
        return PermissionDecision::Ask;
    }
    let analysis = shell::analyze(command);
    if analysis.risk().is_hard_denial() {
        PermissionDecision::Deny
    } else if is_read_only_bash_command(command, &analysis, workspace) {
        PermissionDecision::Allow
    } else {
        PermissionDecision::Ask
    }
}

/// The silent subset stays the lexical one: a pipeline of allowlisted
/// programs written with literal words only, no quoting, expansion, glob,
/// redirect, wrapper, nesting, or control whitespace, and no path outside the
/// workspace.
fn is_read_only_bash_command(
    command: &str,
    analysis: &shell::ShellAnalysis,
    workspace: Option<&std::path::Path>,
) -> bool {
    analysis.risk() == shell::ShellRisk::ReadOnly
        && analysis.has_only_pipes()
        && !command
            .chars()
            .any(|character| character.is_whitespace() && character != ' ')
        && analysis.commands().iter().all(|command| {
            command.depth() == 0
                && command.has_plain_words()
                && !command.has_redirects()
                && !command.has_assignments()
                && !command.is_wrapped()
                && !command
                    .argv()
                    .iter()
                    .any(|word| path_is_outside_workspace(clean_shell_token(word), workspace))
        })
}

fn clean_shell_token(token: &str) -> &str {
    token.trim_matches(|character: char| {
        matches!(
            character,
            '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ':'
        )
    })
}

/// Return true when `path` is outside the optional workspace root.
///
/// Without a workspace root, absolute paths and `..` escapes fail closed.
/// With a root, absolute paths that normalize inside the workspace are admitted.
fn path_is_outside_workspace(path: &str, workspace: Option<&std::path::Path>) -> bool {
    let normalized = path.replace('\\', "/");
    let path = normalized.trim();
    if path.is_empty() {
        return false;
    }
    if path.starts_with('~') || path.starts_with("$HOME") || path.starts_with("${HOME}") {
        return true;
    }

    let Some(root) = workspace else {
        return path_is_lexically_absolute(path) || relative_path_escapes(path);
    };
    path_escapes_workspace_root(root, path)
}

fn path_is_lexically_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || path.starts_with("//")
        || path.starts_with('/')
}

fn relative_path_escapes(path: &str) -> bool {
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

fn path_escapes_workspace_root(root: &std::path::Path, input: &str) -> bool {
    let candidate = std::path::Path::new(input);
    if candidate.is_absolute() || path_is_lexically_absolute(input) {
        match (
            normalize_abs_for_compare(root),
            normalize_abs_for_compare(candidate),
        ) {
            (Ok(root_cmp), Ok(target_cmp)) => !target_cmp.starts_with(&root_cmp),
            _ => true,
        }
    } else {
        relative_path_escapes(input)
    }
}

fn workspace_relative_path(root: &std::path::Path, path: &str) -> Option<String> {
    let candidate = std::path::Path::new(path);
    if !candidate.is_absolute() && !path_is_lexically_absolute(path) {
        return Some(path.replace('\\', "/"));
    }
    let root_cmp = normalize_abs_for_compare(root).ok()?;
    let target_cmp = normalize_abs_for_compare(candidate).ok()?;
    let relative = target_cmp.strip_prefix(&root_cmp).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}

/// Canonicalize when possible; for missing leaf paths, canonicalize the
/// deepest existing ancestor and reattach the suffix (macOS `/var` →
/// `/private/var` must stay consistent with the workspace root).
fn normalize_abs_for_compare(path: &std::path::Path) -> Result<std::path::PathBuf, ()> {
    let lexical = normalize_abs_lexical(path)?;
    if let Ok(canonical) = lexical.canonicalize() {
        return Ok(canonical);
    }

    let mut current = lexical.as_path();
    let mut suffix = Vec::new();
    while !current.exists() {
        let Some(file_name) = current.file_name() else {
            return Ok(lexical);
        };
        suffix.push(file_name.to_os_string());
        let Some(parent) = current.parent() else {
            return Ok(lexical);
        };
        current = parent;
    }

    let mut normalized = current
        .canonicalize()
        .unwrap_or_else(|_| current.to_path_buf());
    for part in suffix.iter().rev() {
        normalized.push(part);
    }
    Ok(normalized)
}

fn normalize_abs_lexical(path: &std::path::Path) -> Result<std::path::PathBuf, ()> {
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            std::path::Component::RootDir => {
                out.push(std::path::Path::new(std::path::MAIN_SEPARATOR_STR));
            }
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => out.push(part),
            std::path::Component::ParentDir => {
                if !out.pop() {
                    return Err(());
                }
            }
        }
    }
    if out.as_os_str().is_empty() {
        Err(())
    } else {
        Ok(out)
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    use crate::permissions::PermissionDecision;

    #[test]
    fn stricter_permission_and_bash_segment_helpers_cover_near_miss_branches() {
        assert_eq!(
            stricter_permission(PermissionDecision::Deny, PermissionDecision::Ask),
            PermissionDecision::Deny
        );
        assert_eq!(
            stricter_permission(PermissionDecision::Ask, PermissionDecision::Allow),
            PermissionDecision::Ask
        );

        let read_only = |segment: &str| {
            let words: Vec<&str> = segment.split_whitespace().collect();
            words
                .split_first()
                .is_some_and(|(program, args)| shell::read_only_program(program, args))
        };
        assert!(!read_only(""));
        assert!(!read_only("ls -R"));
        assert!(!read_only("ls --recursive"));
        assert!(!read_only("ls --dereference"));
        assert!(!read_only("grep --exclude-from=x pattern"));
        assert!(!read_only("grep --file=x pattern"));
        assert!(!read_only("grep --recursive pattern"));
        assert!(!read_only("du -L ."));
        assert!(!read_only("du --dereference ."));
        assert!(read_only("df -h"));
        assert!(read_only("uname -a"));
        assert!(!read_only("cat --output=x"));
        assert!(!read_only("cat --files-from=x"));

        assert!(!read_only("git"));
        assert!(read_only("git --no-pager status"));
        assert!(!read_only("git -C /tmp status"));
        assert!(!read_only("git diff --ext-diff"));
        assert!(read_only("git remote -v"));
        assert!(read_only("git remote"));
        assert!(read_only("git branch --show-current"));
        assert!(!read_only("git branch --delete x"));
    }

    #[test]
    fn normalize_abs_lexical_handles_dot_dot_and_curdir() {
        let rooted = normalize_abs_lexical(std::path::Path::new("/tmp/a/../b/./c")).unwrap();
        assert!(rooted.ends_with("b/c") || rooted.ends_with("b\\c"));

        assert!(normalize_abs_lexical(std::path::Path::new("/../")).is_err());
        assert!(!path_is_outside_workspace(
            "",
            Some(std::path::Path::new("/tmp"))
        ));
        assert!(path_is_outside_workspace(
            "/etc/passwd",
            Some(std::path::Path::new("/tmp"))
        ));
        assert!(!path_is_outside_workspace(
            "relative/path",
            Some(std::path::Path::new("/tmp"))
        ));
        assert!(relative_path_escapes("../escape"));
        assert!(!relative_path_escapes("safe/path"));
    }

    #[test]
    fn invocation_crosses_local_symlink_skips_malformed_batch_entries() {
        let root = tempfile::tempdir().unwrap();
        assert!(!invocation_crosses_local_symlink(
            root.path(),
            "batch",
            &serde_json::json!({
                "invocations": [
                    {"args": {"file_path": "x"}},
                    {"tool": "read"},
                    {"tool": "unknown_tool", "args": {"file_path": "x"}}
                ]
            })
        ));
        assert!(!invocation_crosses_local_symlink(
            root.path(),
            "unknown",
            &serde_json::json!({})
        ));
    }
}
