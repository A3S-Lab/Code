use super::*;

#[test]
fn report_from_required_program_hint_needs_review() {
    let hints = vec![
        ProgramVerificationHint::new("inspect_matches", "Review matched files")
            .required()
            .with_suggested_tools(["read", "search"])
            .with_evidence_uris(["a3s://tool-output/grep/abc"]),
    ];

    let report = VerificationReport::from_program_hints("program_code_search", &hints);

    assert_eq!(report.schema, VERIFICATION_REPORT_SCHEMA);
    assert_eq!(report.subject, "program:program_code_search");
    assert_eq!(report.status, VerificationStatus::NeedsReview);
    assert!(!report.is_complete());
    assert_eq!(report.checks[0].kind, "inspect_matches");
    assert_eq!(report.checks[0].suggested_tools, vec!["read", "search"]);
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
