//! Verification contracts for A3S Code 2.0.
//!
//! Verification is represented as structured checks and reports. The first
//! stage is intentionally conservative: required checks start as
//! `needs_review` until a verifier or the harness marks them passed/failed.

use crate::program::ProgramVerificationHint;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

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
        let mut check =
            self.to_check()
                .with_status(if exit_code == 0 && execution_error.is_none() {
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

        if exit_code != 0 {
            check = check.with_residual_risk(format!(
                "verification command exited with code {exit_code}: {}",
                self.command
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
}

impl VerificationReport {
    pub fn new(subject: impl Into<String>, checks: Vec<VerificationCheck>) -> Self {
        let mut report = Self {
            schema: VERIFICATION_REPORT_SCHEMA.to_string(),
            subject: subject.into(),
            status: VerificationStatus::Skipped,
            checks,
            residual_risks: Vec::new(),
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
mod tests {
    use super::*;

    #[test]
    fn report_from_required_program_hint_needs_review() {
        let hints = vec![
            ProgramVerificationHint::new("inspect_matches", "Review matched files")
                .required()
                .with_suggested_tools(["read", "grep"])
                .with_evidence_uris(["a3s://tool-output/grep/abc"]),
        ];

        let report = VerificationReport::from_program_hints("program_code_search", &hints);

        assert_eq!(report.schema, VERIFICATION_REPORT_SCHEMA);
        assert_eq!(report.subject, "program:program_code_search");
        assert_eq!(report.status, VerificationStatus::NeedsReview);
        assert!(!report.is_complete());
        assert_eq!(report.checks[0].kind, "inspect_matches");
        assert_eq!(report.checks[0].suggested_tools, vec!["read", "grep"]);
        assert_eq!(
            report.checks[0].evidence_uris,
            vec!["a3s://tool-output/grep/abc"]
        );
    }

    #[test]
    fn report_passes_when_required_checks_pass() {
        let check = VerificationCheck::required("check:build", "run_build", "Run build")
            .with_status(VerificationStatus::Passed);

        let report = VerificationReport::new("turn", vec![check]);

        assert_eq!(report.status, VerificationStatus::Passed);
        assert!(report.is_complete());
    }

    #[test]
    fn report_fails_when_any_check_fails() {
        let check = VerificationCheck::required("check:test", "run_tests", "Run tests")
            .with_status(VerificationStatus::Failed);

        let report = VerificationReport::new("turn", vec![check]);

        assert_eq!(report.status, VerificationStatus::Failed);
        assert!(report.is_complete());
    }

    #[test]
    fn static_verifier_builds_report() {
        let verifier = StaticVerifier::new("turn");
        let check = VerificationCheck::optional("check:review", "review", "Review diff")
            .with_status(VerificationStatus::Passed);

        let report = verifier.verify(vec![check]).unwrap();

        assert_eq!(report.subject, "turn");
        assert_eq!(report.status, VerificationStatus::Passed);
    }

    #[test]
    fn verification_command_builds_passed_check_with_evidence() {
        let command = VerificationCommand::required(
            "check:build",
            "type_check",
            "Run cargo check",
            "cargo check",
        );

        let check = command.check_from_execution(
            0,
            Some(&serde_json::json!({
                "artifact": {
                    "artifact_uri": "a3s://tool-output/bash/abc"
                }
            })),
            None,
        );

        assert_eq!(check.status, VerificationStatus::Passed);
        assert!(check.required);
        assert_eq!(check.suggested_tools, vec!["bash"]);
        assert_eq!(check.evidence_uris, vec!["a3s://tool-output/bash/abc"]);
        assert!(check.residual_risk.is_none());
    }

    #[test]
    fn verification_command_builds_failed_check_from_exit_code() {
        let command =
            VerificationCommand::required("check:test", "test", "Run test suite", "cargo test");

        let check = command.check_from_execution(101, None, None);

        assert_eq!(check.status, VerificationStatus::Failed);
        assert_eq!(
            check.residual_risk.as_deref(),
            Some("verification command exited with code 101: cargo test")
        );
    }

    #[test]
    fn rust_workspace_preset_uses_cargo_commands() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\n",
        )
        .unwrap();

        let presets = verification_presets_for_workspace(dir.path());

        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].project_kind, "rust");
        assert_eq!(presets[0].commands[0].command, "cargo fmt -- --check");
        assert!(presets[0]
            .commands
            .iter()
            .any(|command| command.command == "cargo test"));
    }

    #[test]
    fn node_workspace_preset_uses_declared_scripts_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{
                "packageManager": "pnpm@9.0.0",
                "scripts": {
                    "test": "vitest",
                    "lint": "eslint ."
                }
            }"#,
        )
        .unwrap();

        let presets = verification_presets_for_workspace(dir.path());

        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].project_kind, "node");
        assert_eq!(presets[0].commands.len(), 2);
        assert_eq!(presets[0].commands[0].command, "pnpm test");
        assert_eq!(presets[0].commands[1].command, "pnpm lint");
    }

    #[test]
    fn python_workspace_preset_requires_clear_markers() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[tool.pytest.ini_options]\n[tool.ruff]\n",
        )
        .unwrap();

        let presets = verification_presets_for_workspace(dir.path());

        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].project_kind, "python");
        assert_eq!(presets[0].commands[0].command, "python -m pytest");
        assert_eq!(presets[0].commands[1].command, "python -m ruff check .");
    }

    #[test]
    fn summary_skips_empty_reports() {
        let summary = VerificationSummary::from_reports(&[]);

        assert_eq!(summary.status, VerificationStatus::Skipped);
        assert_eq!(summary.report_count, 0);
        assert!(summary.is_complete());
    }

    #[test]
    fn summary_tracks_pending_required_checks() {
        let report = VerificationReport::new(
            "program:search",
            vec![VerificationCheck::required(
                "check:inspect",
                "inspect_matches",
                "Inspect matches",
            )],
        );

        let summary = VerificationSummary::from_reports(&[report]);

        assert_eq!(summary.status, VerificationStatus::NeedsReview);
        assert_eq!(summary.report_count, 1);
        assert_eq!(summary.required_check_count, 1);
        assert_eq!(summary.pending_required_check_count, 1);
        assert_eq!(summary.pending_subjects, vec!["program:search"]);
        assert!(!summary.is_complete());
    }

    #[test]
    fn summary_prioritizes_failed_checks() {
        let failed = VerificationReport::new(
            "program:test",
            vec![
                VerificationCheck::required("check:test", "test", "Run tests")
                    .with_status(VerificationStatus::Failed),
            ],
        );
        let pending = VerificationReport::new(
            "program:search",
            vec![VerificationCheck::required(
                "check:inspect",
                "inspect_matches",
                "Inspect matches",
            )],
        );

        let summary = VerificationSummary::from_reports(&[pending, failed]);

        assert_eq!(summary.status, VerificationStatus::Failed);
        assert_eq!(summary.failed_check_count, 1);
        assert_eq!(summary.failed_subjects, vec!["program:test"]);
        assert!(summary.is_complete());
    }

    #[test]
    fn summary_passes_when_reports_pass() {
        let report = VerificationReport::new(
            "turn",
            vec![
                VerificationCheck::required("check:build", "build", "Run build")
                    .with_status(VerificationStatus::Passed),
            ],
        );

        let summary = VerificationSummary::from_reports(&[report]);

        assert_eq!(summary.status, VerificationStatus::Passed);
        assert_eq!(summary.pending_required_check_count, 0);
        assert_eq!(summary.failed_check_count, 0);
    }

    #[test]
    fn format_summary_includes_actionable_counts_and_subjects() {
        let failed = VerificationReport::new(
            "program:test",
            vec![
                VerificationCheck::required("check:test", "test", "Run tests")
                    .with_status(VerificationStatus::Failed),
            ],
        );
        let pending = VerificationReport::new(
            "program:search",
            vec![VerificationCheck::required(
                "check:review",
                "review",
                "Review matches",
            )],
        );

        let summary = VerificationSummary::from_reports(&[failed, pending]);
        let text = format_verification_summary(&summary);

        assert!(text.contains("Verification failed"));
        assert!(text.contains("1 failed check"));
        assert!(text.contains("program:test"));
        assert!(text.contains("2 reports"));
        assert!(text.contains("2 required checks"));
    }

    #[test]
    fn format_summary_skipped_mentions_no_reports() {
        let summary = VerificationSummary::from_reports(&[]);

        assert_eq!(
            format_verification_summary(&summary),
            "Verification skipped: no reports."
        );
    }

    #[test]
    fn format_summary_needs_review_mentions_pending_subject() {
        let report = VerificationReport::new(
            "program:search",
            vec![VerificationCheck::required(
                "check:review",
                "review",
                "Review matches",
            )],
        );
        let summary = VerificationSummary::from_reports(&[report]);
        let text = format_verification_summary(&summary);

        assert!(text.contains("Verification needs review"));
        assert!(text.contains("1 pending required check"));
        assert!(text.contains("program:search"));
    }

    #[test]
    fn format_summary_mentions_residual_risks() {
        let report = VerificationReport::new(
            "turn",
            vec![
                VerificationCheck::required("check:build", "build", "Run build")
                    .with_status(VerificationStatus::Passed)
                    .with_residual_risk("build did not cover integration tests"),
            ],
        );
        let summary = VerificationSummary::from_reports(&[report]);
        let text = format_verification_summary(&summary);

        assert!(text.contains("Verification needs review"));
        assert!(text.contains("Residual risks: 1."));
    }
}
