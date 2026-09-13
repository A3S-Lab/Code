//! Verification contracts for A3S Code 2.0.
//!
//! Verification is represented as structured checks and reports. The first
//! stage is intentionally conservative: required checks start as
//! `needs_review` until a verifier or the harness marks them passed/failed.

use crate::program::ProgramVerificationHint;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const VERIFICATION_REPORT_SCHEMA: &str = "a3s.verification_report.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    NeedsReview,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationCheck {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub status: VerificationStatus,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suggested_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_uris: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual_risk: Option<String>,
}

impl VerificationCheck {
    pub fn required(
        id: impl Into<String>,
        kind: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            description: description.into(),
            status: VerificationStatus::NeedsReview,
            required: true,
            suggested_tools: Vec::new(),
            evidence_uris: Vec::new(),
            residual_risk: None,
        }
    }

    pub fn optional(
        id: impl Into<String>,
        kind: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            required: false,
            ..Self::required(id, kind, description)
        }
    }

    pub fn with_status(mut self, status: VerificationStatus) -> Self {
        self.status = status;
        self
    }

    pub fn with_suggested_tools(
        mut self,
        tools: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.suggested_tools = tools.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_evidence_uris(mut self, uris: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.evidence_uris = uris.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_residual_risk(mut self, risk: impl Into<String>) -> Self {
        self.residual_risk = Some(risk.into());
        self
    }

    pub fn from_program_hint(subject: &str, index: usize, hint: &ProgramVerificationHint) -> Self {
        let id = format!("program:{subject}:{}:{index}", hint.kind);
        let check = if hint.required {
            Self::required(id, hint.kind.clone(), hint.message.clone())
        } else {
            Self::optional(id, hint.kind.clone(), hint.message.clone())
        };

        check
            .with_suggested_tools(hint.suggested_tools.clone())
            .with_evidence_uris(hint.evidence_uris.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationCommand {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub command: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Expected process exit code (ACCEPTANCE `expect:exit=N`; presets use 0).
    #[serde(default)]
    pub expect_exit: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationPreset {
    pub id: String,
    pub project_kind: String,
    pub description: String,
    pub commands: Vec<VerificationCommand>,
}

impl VerificationPreset {
    pub fn new(
        id: impl Into<String>,
        project_kind: impl Into<String>,
        description: impl Into<String>,
        commands: Vec<VerificationCommand>,
    ) -> Self {
        Self {
            id: id.into(),
            project_kind: project_kind.into(),
            description: description.into(),
            commands,
        }
    }
}

impl VerificationCommand {
    pub fn required(
        id: impl Into<String>,
        kind: impl Into<String>,
        description: impl Into<String>,
        command: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: kind.into(),
            description: description.into(),
            command: command.into(),
            required: true,
            timeout_ms: None,
            expect_exit: 0,
        }
    }

    pub fn optional(
        id: impl Into<String>,
        kind: impl Into<String>,
        description: impl Into<String>,
        command: impl Into<String>,
    ) -> Self {
        Self {
            required: false,
            ..Self::required(id, kind, description, command)
        }
    }

    pub fn with_expect_exit(mut self, expect_exit: i32) -> Self {
        self.expect_exit = expect_exit;
        self
    }

    pub fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn to_check(&self) -> VerificationCheck {
        let check = if self.required {
            VerificationCheck::required(
                self.id.clone(),
                self.kind.clone(),
                self.description.clone(),
            )
        } else {
            VerificationCheck::optional(
                self.id.clone(),
                self.kind.clone(),
                self.description.clone(),
            )
        };

        check.with_suggested_tools(["bash"])
    }

    pub fn check_from_execution(
        &self,
        exit_code: i32,
        metadata: Option<&serde_json::Value>,
        execution_error: Option<&str>,
    ) -> VerificationCheck {
        let passed = exit_code == self.expect_exit && execution_error.is_none();
        let mut check = self.to_check().with_status(if passed {
            VerificationStatus::Passed
        } else {
            VerificationStatus::Failed
        });

        let evidence_uris = artifact_uris(metadata);
        if !evidence_uris.is_empty() {
            check = check.with_evidence_uris(evidence_uris);
        }

        if let Some(error) = execution_error {
            return check
                .with_residual_risk(format!("verification command could not run: {error}"));
        }

        if exit_code != self.expect_exit {
            check = check.with_residual_risk(format!(
                "verification command exited with code {exit_code}, expected {}: {}",
                self.expect_exit, self.command
            ));
        }

        check
    }
}

pub fn verification_presets_for_workspace(workspace: impl AsRef<Path>) -> Vec<VerificationPreset> {
    let workspace = workspace.as_ref();
    let mut presets = Vec::new();

    if workspace.join("Cargo.toml").is_file() {
        presets.push(VerificationPreset::new(
            "rust-default",
            "rust",
            "Rust cargo verification",
            vec![
                VerificationCommand::required(
                    "rust:fmt",
                    "format",
                    "Check Rust formatting",
                    "cargo fmt -- --check",
                ),
                VerificationCommand::required(
                    "rust:check",
                    "type_check",
                    "Run Rust type checking",
                    "cargo check",
                ),
                VerificationCommand::required("rust:test", "test", "Run Rust tests", "cargo test"),
                VerificationCommand::optional(
                    "rust:clippy",
                    "lint",
                    "Run Rust clippy lints",
                    "cargo clippy -- -D warnings",
                ),
            ],
        ));
    }

    if workspace.join("package.json").is_file() {
        if let Some(preset) = node_verification_preset(workspace) {
            presets.push(preset);
        }
    }

    if workspace.join("pyproject.toml").is_file() || workspace.join("pytest.ini").is_file() {
        let mut commands = Vec::new();
        if workspace.join("tests").is_dir()
            || file_contains(&workspace.join("pyproject.toml"), "[tool.pytest")
            || workspace.join("pytest.ini").is_file()
        {
            commands.push(VerificationCommand::required(
                "python:test",
                "test",
                "Run Python tests",
                "python -m pytest",
            ));
        }
        if workspace.join("ruff.toml").is_file()
            || workspace.join(".ruff.toml").is_file()
            || file_contains(&workspace.join("pyproject.toml"), "[tool.ruff")
        {
            commands.push(VerificationCommand::optional(
                "python:ruff",
                "lint",
                "Run Ruff lint checks",
                "python -m ruff check .",
            ));
        }
        if workspace.join("mypy.ini").is_file()
            || workspace.join(".mypy.ini").is_file()
            || file_contains(&workspace.join("pyproject.toml"), "[tool.mypy")
        {
            commands.push(VerificationCommand::optional(
                "python:mypy",
                "type_check",
                "Run mypy type checking",
                "python -m mypy .",
            ));
        }
        if !commands.is_empty() {
            presets.push(VerificationPreset::new(
                "python-default",
                "python",
                "Python project verification",
                commands,
            ));
        }
    }

    if workspace.join("go.mod").is_file() {
        presets.push(VerificationPreset::new(
            "go-default",
            "go",
            "Go module verification",
            vec![
                VerificationCommand::required("go:test", "test", "Run Go tests", "go test ./..."),
                VerificationCommand::optional("go:vet", "lint", "Run go vet", "go vet ./..."),
            ],
        ));
    }

    presets
}

fn node_verification_preset(workspace: &Path) -> Option<VerificationPreset> {
    let package_json = std::fs::read_to_string(workspace.join("package.json")).ok()?;
    let package: serde_json::Value = serde_json::from_str(&package_json).ok()?;
    let scripts = package.get("scripts").and_then(|value| value.as_object())?;
    let package_manager = detect_node_package_manager(workspace, &package);
    let mut commands = Vec::new();

    for (script, kind, description, required) in [
        ("test", "test", "Run JavaScript tests", true),
        (
            "typecheck",
            "type_check",
            "Run JavaScript type checks",
            false,
        ),
        ("lint", "lint", "Run JavaScript lint checks", false),
    ] {
        if scripts.contains_key(script) {
            let command = node_script_command(&package_manager, script);
            let id = format!("node:{script}");
            let verification = if required {
                VerificationCommand::required(id, kind, description, command)
            } else {
                VerificationCommand::optional(id, kind, description, command)
            };
            commands.push(verification);
        }
    }

    if commands.is_empty() {
        return None;
    }

    Some(VerificationPreset::new(
        "node-default",
        "node",
        "Node.js package verification",
        commands,
    ))
}

fn detect_node_package_manager(workspace: &Path, package: &serde_json::Value) -> String {
    if let Some(manager) = package
        .get("packageManager")
        .and_then(|value| value.as_str())
    {
        if let Some((name, _)) = manager.split_once('@') {
            return name.to_string();
        }
    }

    if workspace.join("pnpm-lock.yaml").is_file() {
        "pnpm".to_string()
    } else if workspace.join("yarn.lock").is_file() {
        "yarn".to_string()
    } else if workspace.join("bun.lockb").is_file() || workspace.join("bun.lock").is_file() {
        "bun".to_string()
    } else {
        "npm".to_string()
    }
}

fn node_script_command(package_manager: &str, script: &str) -> String {
    match package_manager {
        "pnpm" | "yarn" => format!("{package_manager} {script}"),
        "bun" => format!("bun run {script}"),
        "npm" if script == "test" => "npm test".to_string(),
        "npm" => format!("npm run {script}"),
        other => format!("{other} run {script}"),
    }
}

fn file_contains(path: &Path, needle: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|content| content.contains(needle))
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationReport {
    pub schema: String,
    pub subject: String,
    pub status: VerificationStatus,
    pub checks: Vec<VerificationCheck>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub residual_risks: Vec<String>,
    /// Effect digest this report is allowed to close. Unbound reports never pass the completion gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_digest: Option<String>,
}

impl VerificationReport {
    pub fn new(subject: impl Into<String>, checks: Vec<VerificationCheck>) -> Self {
        let mut report = Self {
            schema: VERIFICATION_REPORT_SCHEMA.to_string(),
            subject: subject.into(),
            status: VerificationStatus::Skipped,
            checks,
            residual_risks: Vec::new(),
            effect_digest: None,
        };
        report.status = report.derive_status();
        report
    }

    pub fn from_program_hints(subject: &str, hints: &[ProgramVerificationHint]) -> Self {
        let checks = hints
            .iter()
            .enumerate()
            .map(|(index, hint)| VerificationCheck::from_program_hint(subject, index, hint))
            .collect();
        Self::new(format!("program:{subject}"), checks)
    }

    pub fn with_effect_digest(mut self, digest: impl Into<String>) -> Self {
        self.effect_digest = Some(digest.into());
        self
    }

    pub fn with_residual_risk(mut self, risk: impl Into<String>) -> Self {
        self.residual_risks.push(risk.into());
        self.status = self.derive_status();
        self
    }

    pub fn is_complete(&self) -> bool {
        !matches!(self.status, VerificationStatus::NeedsReview)
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| {
            serde_json::json!({
                "schema": VERIFICATION_REPORT_SCHEMA,
                "subject": self.subject,
                "status": "failed",
                "checks": [],
                "residual_risks": ["failed to serialize verification report"],
            })
        })
    }

    fn derive_status(&self) -> VerificationStatus {
        if self
            .checks
            .iter()
            .any(|check| check.status == VerificationStatus::Failed)
        {
            return VerificationStatus::Failed;
        }

        if self.checks.iter().any(|check| {
            check.required
                && matches!(
                    check.status,
                    VerificationStatus::NeedsReview | VerificationStatus::Skipped
                )
        }) {
            return VerificationStatus::NeedsReview;
        }

        if !self.residual_risks.is_empty() {
            return VerificationStatus::NeedsReview;
        }

        if self.checks.is_empty() {
            VerificationStatus::Skipped
        } else {
            VerificationStatus::Passed
        }
    }
}

fn artifact_uris(metadata: Option<&serde_json::Value>) -> Vec<String> {
    let mut uris = Vec::new();
    if let Some(metadata) = metadata {
        collect_artifact_uris(metadata, &mut uris);
    }
    uris.sort();
    uris.dedup();
    uris
}

fn collect_artifact_uris(value: &serde_json::Value, uris: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(uri) = object.get("artifact_uri").and_then(|value| value.as_str()) {
                uris.push(uri.to_string());
            }
            for value in object.values() {
                collect_artifact_uris(value, uris);
            }
        }
        serde_json::Value::Array(items) => {
            for value in items {
                collect_artifact_uris(value, uris);
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationSummary {
    pub status: VerificationStatus,
    pub report_count: usize,
    pub required_check_count: usize,
    pub pending_required_check_count: usize,
    pub failed_check_count: usize,
    pub residual_risk_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_subjects: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_subjects: Vec<String>,
}

impl VerificationSummary {
    pub fn from_reports(reports: &[VerificationReport]) -> Self {
        let mut required_check_count = 0;
        let mut pending_required_check_count = 0;
        let mut failed_check_count = 0;
        let mut residual_risk_count = 0;
        let mut pending_subjects = Vec::new();
        let mut failed_subjects = Vec::new();

        for report in reports {
            if matches!(report.status, VerificationStatus::NeedsReview) {
                pending_subjects.push(report.subject.clone());
            }

            if matches!(report.status, VerificationStatus::Failed) {
                failed_subjects.push(report.subject.clone());
            }

            residual_risk_count += report.residual_risks.len();

            for check in &report.checks {
                if check.required {
                    required_check_count += 1;
                    if matches!(
                        check.status,
                        VerificationStatus::NeedsReview | VerificationStatus::Skipped
                    ) {
                        pending_required_check_count += 1;
                        pending_subjects.push(report.subject.clone());
                    }
                }

                if check.status == VerificationStatus::Failed {
                    failed_check_count += 1;
                    failed_subjects.push(report.subject.clone());
                }

                if check.residual_risk.is_some() {
                    residual_risk_count += 1;
                    pending_subjects.push(report.subject.clone());
                }
            }
        }

        pending_subjects.sort();
        pending_subjects.dedup();
        failed_subjects.sort();
        failed_subjects.dedup();

        let status = if failed_check_count > 0
            || reports
                .iter()
                .any(|report| report.status == VerificationStatus::Failed)
        {
            VerificationStatus::Failed
        } else if pending_required_check_count > 0
            || residual_risk_count > 0
            || reports
                .iter()
                .any(|report| report.status == VerificationStatus::NeedsReview)
        {
            VerificationStatus::NeedsReview
        } else if reports.is_empty() {
            VerificationStatus::Skipped
        } else {
            VerificationStatus::Passed
        };

        Self {
            status,
            report_count: reports.len(),
            required_check_count,
            pending_required_check_count,
            failed_check_count,
            residual_risk_count,
            pending_subjects,
            failed_subjects,
        }
    }

    pub fn is_complete(&self) -> bool {
        !matches!(self.status, VerificationStatus::NeedsReview)
    }

    /// Whether structured verification is strong enough to authorize `GoalAchieved`.
    ///
    /// Fail-closed: empty reports, skipped/optional-only passes, pending required
    /// checks, failures, and residual risks never authorize completion by themselves.
    /// An LLM may still evaluate prose, but the host/core gate requires this.
    pub fn supports_goal_achievement(&self) -> bool {
        matches!(self.status, VerificationStatus::Passed)
            && self.report_count > 0
            && self.required_check_count > 0
            && self.pending_required_check_count == 0
            && self.failed_check_count == 0
            && self.residual_risk_count == 0
    }

    pub fn to_value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| {
            serde_json::json!({
                "status": "failed",
                "report_count": self.report_count,
                "required_check_count": self.required_check_count,
                "pending_required_check_count": self.pending_required_check_count,
                "failed_check_count": self.failed_check_count,
                "residual_risk_count": self.residual_risk_count,
                "failed_subjects": ["failed to serialize verification summary"],
            })
        })
    }
}

/// Normalize shell text for preset command coverage checks.
pub fn normalize_shell_command(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// True when `command` executes `preset` (exact, with trailing args, or after `&&` / `;`).
pub fn shell_command_covers_preset(command: &str, preset: &str) -> bool {
    let command = normalize_shell_command(command);
    let preset = normalize_shell_command(preset);
    if command.is_empty() || preset.is_empty() {
        return false;
    }
    if command == preset || command.starts_with(&format!("{preset} ")) {
        return true;
    }
    let and_prefix = format!("&& {preset}");
    let semi_prefix = format!("; {preset}");
    command.contains(&format!("{and_prefix} "))
        || command.ends_with(&and_prefix)
        || command.contains(&format!("{semi_prefix} "))
        || command.ends_with(&semi_prefix)
}

/// Build a verification report when a shell command covers a workspace preset
/// command **or** a durable `/goal` ACCEPTANCE.md machine criterion
/// (`kind:command` or synthesized `test -f` for `kind:file_exists`).
pub fn shell_verification_report_for_command(
    workspace: &Path,
    command: &str,
    exit_code: i32,
    metadata: Option<&serde_json::Value>,
    execution_error: Option<&str>,
) -> Option<VerificationReport> {
    let matched = verification_presets_for_workspace(workspace)
        .into_iter()
        .flat_map(|preset| preset.commands)
        .chain(acceptance_shell_commands_for_workspace(workspace))
        .find(|preset_command| shell_command_covers_preset(command, &preset_command.command))?;
    let check = matched.check_from_execution(exit_code, metadata, execution_error);
    Some(VerificationReport::new(
        format!("shell:{}", matched.id),
        vec![check],
    ))
}

/// Loop STATE statuses that still own a live `/goal` ACCEPTANCE contract.
///
/// Completed (`verified` / `achieved` / `cancelled`) loops must not pollute
/// shell evidence or GoalAchieved emission for a later goal in the same workspace.
const ACTIVE_GOAL_LOOP_STATUSES: &[&str] = &["running", "retrying", "paused"];

/// True when `STATE.md` marks this loop as still owning durable ACCEPTANCE.
fn loop_owns_active_acceptance_contract(loop_dir: &Path) -> bool {
    let Ok(state) = std::fs::read_to_string(loop_dir.join("STATE.md")) else {
        return false;
    };
    for line in state.lines() {
        let trimmed = line.trim();
        let Some(status) = trimmed.strip_prefix("Status:") else {
            continue;
        };
        let status = status.trim();
        return ACTIVE_GOAL_LOOP_STATUSES
            .iter()
            .any(|allowed| status.eq_ignore_ascii_case(allowed));
    }
    false
}

/// Parse machine ACCEPTANCE criteria from **active** `.a3s/loops/*/ACCEPTANCE.md`
/// so Core shell evidence and Host ACCEPTANCE re-checks share the same predicates.
///
/// Only loops whose `STATE.md` is `running`, `retrying`, or `paused` contribute.
/// Stale completed loops are ignored (avoids leftover `assert:true` authorizing
/// a later goal).
///
/// - `kind:command` → the assert command (with optional `expect:exit`)
/// - `kind:file_exists` → synthesized `test -f <path>` (exit 0 == exists)
pub fn acceptance_shell_commands_for_workspace(
    workspace: impl AsRef<Path>,
) -> Vec<VerificationCommand> {
    let loops = workspace.as_ref().join(".a3s").join("loops");
    let Ok(entries) = std::fs::read_dir(&loops) else {
        return Vec::new();
    };
    let mut commands = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !loop_owns_active_acceptance_contract(&path) {
            continue;
        }
        let acceptance = path.join("ACCEPTANCE.md");
        let Ok(body) = std::fs::read_to_string(&acceptance) else {
            continue;
        };
        let loop_id = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("goal");
        commands.extend(parse_acceptance_shell_commands(
            &body,
            loop_id,
            workspace.as_ref(),
        ));
    }
    commands
}

fn parse_acceptance_shell_commands(
    body: &str,
    loop_id: &str,
    workspace: &Path,
) -> Vec<VerificationCommand> {
    let mut commands = Vec::new();
    for (index, line) in body.lines().enumerate() {
        let trimmed = line.trim_start();
        let rest = if let Some(rest) = trimmed.strip_prefix("- [") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix("* [") {
            rest
        } else {
            continue;
        };
        let Some((_mark, body)) = rest.split_once(']') else {
            continue;
        };
        let body = body.trim().trim_start_matches(':').trim();
        let lower = body.to_ascii_lowercase();
        let line_id = format!("acceptance:{loop_id}:{}", index + 1);
        if lower.starts_with("kind:command") {
            let Some(command) = extract_acceptance_assert_command(body) else {
                continue;
            };
            let expect_exit = extract_acceptance_expect_exit(body).unwrap_or(0);
            commands.push(
                VerificationCommand::required(
                    line_id,
                    "acceptance_command",
                    format!("ACCEPTANCE kind:command ({loop_id})"),
                    command,
                )
                .with_expect_exit(expect_exit),
            );
            continue;
        }
        if lower.starts_with("kind:file_exists") || lower.starts_with("kind:file-exists") {
            let Some(path) = extract_acceptance_assert_command(body) else {
                continue;
            };
            // Skip workspace-escaping paths so Core evidence matches Host latch
            // (durable goals prove in-workspace outcomes only).
            if !acceptance_file_path_allowed_in_workspace(workspace, &path) {
                continue;
            }
            let command = format!("test -f {}", shell_quote_acceptance_path(&path));
            commands.push(VerificationCommand::required(
                line_id,
                "acceptance_file_exists",
                format!("ACCEPTANCE kind:file_exists ({loop_id})"),
                command,
            ));
        }
    }
    commands
}

/// Whether a `kind:file_exists` assert may contribute Core shell evidence.
///
/// Relative `../` escapes are rejected. Absolute paths are accepted only when
/// they canonicalize to a regular file under the workspace (otherwise Host
/// latch is the authority and Core must not treat them as machine evidence).
fn acceptance_file_path_allowed_in_workspace(workspace: &Path, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let p = Path::new(path);
    if !p.is_absolute() {
        let mut depth = 0i32;
        for component in p.components() {
            match component {
                std::path::Component::ParentDir => {
                    depth -= 1;
                    if depth < 0 {
                        return false;
                    }
                }
                std::path::Component::Normal(_) => depth += 1,
                std::path::Component::RootDir | std::path::Component::Prefix(_) => return false,
                std::path::Component::CurDir => {}
            }
        }
        return true;
    }
    let Ok(workspace_canon) = workspace.canonicalize() else {
        return false;
    };
    let candidate = PathBuf::from(path);
    if !candidate.is_file() {
        return false;
    }
    let Ok(file_canon) = candidate.canonicalize() else {
        return false;
    };
    file_canon.starts_with(&workspace_canon)
}

/// Quote a path for a synthesized `test -f` ACCEPTANCE predicate.
fn shell_quote_acceptance_path(path: &str) -> String {
    if path.is_empty() {
        return "''".to_string();
    }
    if path
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
    {
        return path.to_string();
    }
    format!("'{}'", path.replace('\'', "'\\''"))
}

fn extract_acceptance_assert_command(body: &str) -> Option<String> {
    let idx = body.to_ascii_lowercase().find("assert:")?;
    let after = body[idx + "assert:".len()..].trim_start();
    if let Some(rest) = after.strip_prefix('`') {
        let end = rest.find('`')?;
        let command = rest[..end].trim();
        if command.is_empty() {
            return None;
        }
        return Some(command.to_string());
    }
    let command = after
        .split_whitespace()
        .next()
        .filter(|token| !token.to_ascii_lowercase().starts_with("expect:"))?;
    Some(command.to_string())
}

fn extract_acceptance_expect_exit(body: &str) -> Option<i32> {
    let lower = body.to_ascii_lowercase();
    let idx = lower.find("expect:exit=")?;
    let after = &body[idx + "expect:exit=".len()..];
    let digits: String = after
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    digits.parse().ok()
}

/// Merge a shell-preset verification report into tool metadata when applicable.
pub fn merge_shell_verification_metadata(
    metadata: Option<serde_json::Value>,
    workspace: Option<&Path>,
    command: &str,
    exit_code: i32,
    execution_error: Option<&str>,
) -> Option<serde_json::Value> {
    let mut metadata = metadata.unwrap_or_else(|| serde_json::json!({}));
    let Some(workspace) = workspace else {
        return Some(metadata);
    };
    if metadata.get("verification_report").is_some() {
        return Some(metadata);
    }
    let Some(report) = shell_verification_report_for_command(
        workspace,
        command,
        exit_code,
        Some(&metadata),
        execution_error,
    ) else {
        return Some(metadata);
    };
    if let Some(object) = metadata.as_object_mut() {
        object.insert("verification_report".to_string(), report.to_value());
    }
    Some(metadata)
}

/// Combine an LLM achievement judgment with structured verification evidence.
pub fn goal_achieved_after_evidence_gate(
    llm_achieved: bool,
    reports: &[VerificationReport],
) -> bool {
    llm_achieved && VerificationSummary::from_reports(reports).supports_goal_achievement()
}

/// True when a shell verification subject was derived from ACCEPTANCE.md.
pub fn is_acceptance_verification_subject(subject: &str) -> bool {
    subject.starts_with("shell:acceptance:")
}

/// True when reports include at least one passing ACCEPTANCE-derived shell report.
pub fn reports_include_passing_acceptance(reports: &[VerificationReport]) -> bool {
    reports.iter().any(|report| {
        is_acceptance_verification_subject(&report.subject)
            && matches!(report.status, VerificationStatus::Passed)
            && report
                .checks
                .iter()
                .any(|check| check.required && matches!(check.status, VerificationStatus::Passed))
    })
}

fn reports_include_passing_active_acceptance(
    reports: &[VerificationReport],
    acceptance_commands: &[VerificationCommand],
) -> bool {
    // Group machine criteria by loop id. Every active loop that still owns
    // ACCEPTANCE must have its own passing report — a sibling/orphaned loop's
    // easy criterion must not authorize GoalAchieved for a different loop.
    let mut subjects_by_loop: std::collections::HashMap<String, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    for command in acceptance_commands {
        let Some(loop_id) = acceptance_loop_id_from_command_id(&command.id) else {
            continue;
        };
        subjects_by_loop
            .entry(loop_id.to_string())
            .or_default()
            .insert(format!("shell:{}", command.id));
    }
    if subjects_by_loop.is_empty() {
        return false;
    }
    subjects_by_loop.values().all(|subjects| {
        reports.iter().any(|report| {
            subjects.contains(&report.subject)
                && matches!(report.status, VerificationStatus::Passed)
                && report.checks.iter().any(|check| {
                    check.required && matches!(check.status, VerificationStatus::Passed)
                })
        })
    })
}

/// Command ids look like `acceptance:<loop_id>:<line>` (loop ids may contain `-`).
fn acceptance_loop_id_from_command_id(command_id: &str) -> Option<&str> {
    let rest = command_id.strip_prefix("acceptance:")?;
    let (loop_id, _line) = rest.rsplit_once(':')?;
    if loop_id.is_empty() {
        return None;
    }
    Some(loop_id)
}

/// Decide whether planning should emit `GoalAchieved` before `End`.
///
/// When the workspace declares durable `/goal` machine ACCEPTANCE criteria on an
/// **active** loop, emission requires a passing report for **each** such loop's
/// criteria — so a workspace preset alone, a completed-loop leftover, or a
/// sibling/orphaned active loop's report cannot authorize GoalAchieved.
/// Host latch still re-checks the current loop ACCEPTANCE.
pub fn should_emit_goal_achieved_for_workspace(
    llm_achieved: bool,
    reports: &[VerificationReport],
    workspace: Option<&Path>,
) -> bool {
    if !goal_achieved_after_evidence_gate(llm_achieved, reports) {
        return false;
    }
    let Some(workspace) = workspace else {
        return true;
    };
    let acceptance = acceptance_shell_commands_for_workspace(workspace);
    if acceptance.is_empty() {
        return true;
    }
    reports_include_passing_active_acceptance(reports, &acceptance)
}

/// Decide whether planning should emit `GoalAchieved` before `End`.
///
/// Kept as a thin wrapper so host/core share one fail-closed contract and tests
/// can pin emission policy without standing up a full agent loop.
pub fn should_emit_goal_achieved(llm_achieved: bool, reports: &[VerificationReport]) -> bool {
    should_emit_goal_achieved_for_workspace(llm_achieved, reports, None)
}

pub fn format_verification_summary(summary: &VerificationSummary) -> String {
    let reports = plural(summary.report_count, "report", "reports");
    let required_checks = plural(
        summary.required_check_count,
        "required check",
        "required checks",
    );

    let mut text = match summary.status {
        VerificationStatus::Skipped if summary.report_count == 0 => {
            "Verification skipped: no reports.".to_string()
        }
        VerificationStatus::Skipped => format!("Verification skipped: {reports}."),
        VerificationStatus::Passed => {
            format!("Verification passed: {reports}, {required_checks}.")
        }
        VerificationStatus::Failed => {
            let failed = if summary.failed_check_count > 0 {
                plural(summary.failed_check_count, "failed check", "failed checks")
            } else {
                "failed report".to_string()
            };
            let subjects = subject_list(&summary.failed_subjects);
            if subjects.is_empty() {
                format!("Verification failed: {failed}. {reports}, {required_checks}.")
            } else {
                format!(
                    "Verification failed: {failed} across subjects: {subjects}. {reports}, {required_checks}."
                )
            }
        }
        VerificationStatus::NeedsReview => {
            let pending = if summary.pending_required_check_count > 0 {
                plural(
                    summary.pending_required_check_count,
                    "pending required check",
                    "pending required checks",
                )
            } else {
                "review required".to_string()
            };
            let subjects = subject_list(&summary.pending_subjects);
            if subjects.is_empty() {
                format!("Verification needs review: {pending}. {reports}, {required_checks}.")
            } else {
                format!(
                    "Verification needs review: {pending} across subjects: {subjects}. {reports}, {required_checks}."
                )
            }
        }
    };

    if summary.residual_risk_count > 0 {
        text.push(' ');
        text.push_str(&format!("Residual risks: {}.", summary.residual_risk_count));
    }

    text
}

pub fn verification_status_label(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Passed => "passed",
        VerificationStatus::Failed => "failed",
        VerificationStatus::NeedsReview => "needs_review",
        VerificationStatus::Skipped => "skipped",
    }
}

fn plural(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

fn subject_list(subjects: &[String]) -> String {
    const MAX_SUBJECTS: usize = 5;
    let mut visible: Vec<&str> = subjects
        .iter()
        .take(MAX_SUBJECTS)
        .map(String::as_str)
        .collect();
    if subjects.len() > MAX_SUBJECTS {
        visible.push("...");
    }
    visible.join(", ")
}

pub trait Verifier: Send + Sync {
    fn verify(&self, checks: Vec<VerificationCheck>) -> Result<VerificationReport>;
}

#[derive(Debug, Clone)]
pub struct StaticVerifier {
    subject: String,
}

impl StaticVerifier {
    pub fn new(subject: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
        }
    }
}

impl Verifier for StaticVerifier {
    fn verify(&self, checks: Vec<VerificationCheck>) -> Result<VerificationReport> {
        Ok(VerificationReport::new(self.subject.clone(), checks))
    }
}

#[cfg(test)]
#[path = "verification/tests.rs"]
mod tests;
