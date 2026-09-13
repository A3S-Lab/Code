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
        Some("verification command exited with code 101, expected 0: cargo test")
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

#[test]
fn goal_achievement_gate_requires_passed_required_checks() {
    assert!(!goal_achieved_after_evidence_gate(true, &[]));
    assert!(!goal_achieved_after_evidence_gate(false, &[]));

    let optional_only = VerificationReport::new(
        "turn",
        vec![VerificationCheck::optional("opt", "lint", "optional lint")
            .with_status(VerificationStatus::Passed)],
    );
    assert!(
        !goal_achieved_after_evidence_gate(true, &[optional_only]),
        "optional-only passes must not authorize GoalAchieved"
    );

    let required_passed = VerificationReport::new(
        "shell:rust:test",
        vec![
            VerificationCheck::required("rust:test", "test", "Run Rust tests")
                .with_status(VerificationStatus::Passed),
        ],
    );
    assert!(goal_achieved_after_evidence_gate(
        true,
        &[required_passed.clone()]
    ));
    assert!(!goal_achieved_after_evidence_gate(
        false,
        &[required_passed]
    ));

    let required_failed = VerificationReport::new(
        "shell:rust:test",
        vec![
            VerificationCheck::required("rust:test", "test", "Run Rust tests")
                .with_status(VerificationStatus::Failed),
        ],
    );
    assert!(!goal_achieved_after_evidence_gate(true, &[required_failed]));
    assert!(should_emit_goal_achieved(
        true,
        &[VerificationReport::new(
            "shell:rust:test",
            vec![
                VerificationCheck::required("rust:test", "test", "Run Rust tests")
                    .with_status(VerificationStatus::Passed)
            ],
        )]
    ));
    assert!(!should_emit_goal_achieved(true, &[]));
}

fn write_active_loop_state(loop_dir: &std::path::Path, status: &str) {
    std::fs::write(
        loop_dir.join("STATE.md"),
        format!("Status: {status}\nPhase: executing\n"),
    )
    .unwrap();
}

#[test]
fn goal_emit_requires_acceptance_report_when_machine_acceptance_exists() {
    let root = tempfile::tempdir().unwrap();
    let loop_dir = root
        .path()
        .join(".a3s")
        .join("loops")
        .join("goal-emit-gate");
    std::fs::create_dir_all(&loop_dir).unwrap();
    write_active_loop_state(&loop_dir, "running");
    // Single criterion line so parse line-id is `:1` (matches report subjects below).
    std::fs::write(
        loop_dir.join("ACCEPTANCE.md"),
        "- [ ] kind:command assert:`true` expect:exit=0\n",
    )
    .unwrap();

    let preset_only = VerificationReport::new(
        "shell:rust:test",
        vec![
            VerificationCheck::required("rust:test", "test", "Run Rust tests")
                .with_status(VerificationStatus::Passed),
        ],
    );
    assert!(
        !should_emit_goal_achieved_for_workspace(true, &[preset_only.clone()], Some(root.path())),
        "workspace preset alone must not authorize GoalAchieved when ACCEPTANCE machine criteria exist"
    );

    let acceptance = VerificationReport::new(
        "shell:acceptance:goal-emit-gate:1",
        vec![VerificationCheck::required(
            "acceptance:goal-emit-gate:1",
            "acceptance_command",
            "ACCEPTANCE kind:command",
        )
        .with_status(VerificationStatus::Passed)],
    );
    assert!(should_emit_goal_achieved_for_workspace(
        true,
        &[preset_only, acceptance],
        Some(root.path()),
    ));

    // No machine ACCEPTANCE → preset path remains valid for ordinary goal_tracking.
    let bare = tempfile::tempdir().unwrap();
    assert!(should_emit_goal_achieved_for_workspace(
        true,
        &[VerificationReport::new(
            "shell:rust:test",
            vec![
                VerificationCheck::required("rust:test", "test", "Run Rust tests")
                    .with_status(VerificationStatus::Passed)
            ],
        )],
        Some(bare.path()),
    ));
}

#[test]
fn stale_completed_loop_acceptance_does_not_authorize_or_pollute_active_goal() {
    let root = tempfile::tempdir().unwrap();
    let loops = root.path().join(".a3s").join("loops");

    let stale = loops.join("goal-stale-old");
    std::fs::create_dir_all(&stale).unwrap();
    write_active_loop_state(&stale, "verified");
    std::fs::write(
        stale.join("ACCEPTANCE.md"),
        "- [x] kind:command assert:`true` expect:exit=0\n",
    )
    .unwrap();

    let active = loops.join("goal-active-now");
    std::fs::create_dir_all(&active).unwrap();
    write_active_loop_state(&active, "running");
    std::fs::write(
        active.join("ACCEPTANCE.md"),
        "- [ ] kind:command assert:`false` expect:exit=0\n",
    )
    .unwrap();

    let commands = acceptance_shell_commands_for_workspace(root.path());
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert_eq!(commands[0].id, "acceptance:goal-active-now:1");
    assert_eq!(commands[0].command, "false");

    let preset = VerificationReport::new(
        "shell:rust:test",
        vec![
            VerificationCheck::required("rust:test", "test", "Run Rust tests")
                .with_status(VerificationStatus::Passed),
        ],
    );
    let stale_report = VerificationReport::new(
        "shell:acceptance:goal-stale-old:1",
        vec![VerificationCheck::required(
            "acceptance:goal-stale-old:1",
            "acceptance_command",
            "stale ACCEPTANCE",
        )
        .with_status(VerificationStatus::Passed)],
    );
    assert!(
        !should_emit_goal_achieved_for_workspace(
            true,
            &[preset.clone(), stale_report],
            Some(root.path()),
        ),
        "completed-loop ACCEPTANCE report must not authorize emit for a different active goal"
    );

    let active_report = VerificationReport::new(
        "shell:acceptance:goal-active-now:1",
        vec![VerificationCheck::required(
            "acceptance:goal-active-now:1",
            "acceptance_command",
            "active ACCEPTANCE",
        )
        .with_status(VerificationStatus::Passed)],
    );
    assert!(should_emit_goal_achieved_for_workspace(
        true,
        &[preset, active_report],
        Some(root.path()),
    ));

    // Only completed loops → no active durable contract → preset path remains valid.
    let only_stale = tempfile::tempdir().unwrap();
    let only_stale_loop = only_stale
        .path()
        .join(".a3s")
        .join("loops")
        .join("goal-done");
    std::fs::create_dir_all(&only_stale_loop).unwrap();
    write_active_loop_state(&only_stale_loop, "achieved");
    std::fs::write(
        only_stale_loop.join("ACCEPTANCE.md"),
        "- [x] kind:command assert:`true` expect:exit=0\n",
    )
    .unwrap();
    assert!(acceptance_shell_commands_for_workspace(only_stale.path()).is_empty());
    assert!(should_emit_goal_achieved_for_workspace(
        true,
        &[VerificationReport::new(
            "shell:rust:test",
            vec![
                VerificationCheck::required("rust:test", "test", "Run Rust tests")
                    .with_status(VerificationStatus::Passed)
            ],
        )],
        Some(only_stale.path()),
    ));
}

#[test]
fn each_active_loop_needs_its_own_acceptance_report_before_goal_emit() {
    let root = tempfile::tempdir().unwrap();
    let loops = root.path().join(".a3s").join("loops");

    let orphan = loops.join("goal-orphan-easy");
    std::fs::create_dir_all(&orphan).unwrap();
    write_active_loop_state(&orphan, "running");
    std::fs::write(
        orphan.join("ACCEPTANCE.md"),
        "- [x] kind:command assert:`true` expect:exit=0\n",
    )
    .unwrap();

    let current = loops.join("goal-current-hard");
    std::fs::create_dir_all(&current).unwrap();
    write_active_loop_state(&current, "running");
    std::fs::write(
        current.join("ACCEPTANCE.md"),
        "- [ ] kind:command assert:`false` expect:exit=0\n",
    )
    .unwrap();

    let commands = acceptance_shell_commands_for_workspace(root.path());
    assert_eq!(commands.len(), 2, "{commands:?}");

    let preset = VerificationReport::new(
        "shell:rust:test",
        vec![
            VerificationCheck::required("rust:test", "test", "Run Rust tests")
                .with_status(VerificationStatus::Passed),
        ],
    );
    let orphan_report = VerificationReport::new(
        "shell:acceptance:goal-orphan-easy:1",
        vec![VerificationCheck::required(
            "acceptance:goal-orphan-easy:1",
            "acceptance_command",
            "orphan ACCEPTANCE",
        )
        .with_status(VerificationStatus::Passed)],
    );
    assert!(
        !should_emit_goal_achieved_for_workspace(
            true,
            &[preset.clone(), orphan_report.clone()],
            Some(root.path()),
        ),
        "sibling/orphaned active-loop report must not authorize emit for another active loop"
    );

    let current_report = VerificationReport::new(
        "shell:acceptance:goal-current-hard:1",
        vec![VerificationCheck::required(
            "acceptance:goal-current-hard:1",
            "acceptance_command",
            "current ACCEPTANCE",
        )
        .with_status(VerificationStatus::Passed)],
    );
    assert!(
        !should_emit_goal_achieved_for_workspace(
            true,
            &[preset.clone(), current_report.clone()],
            Some(root.path()),
        ),
        "current-loop report alone is insufficient while another active loop still owns criteria"
    );
    assert!(should_emit_goal_achieved_for_workspace(
        true,
        &[preset, orphan_report, current_report],
        Some(root.path()),
    ));
}

#[test]
fn shell_command_covers_preset_matches_exact_and_trailing_args() {
    assert!(shell_command_covers_preset("cargo test", "cargo test"));
    assert!(shell_command_covers_preset(
        "cargo test -p a3s-code-core",
        "cargo test"
    ));
    assert!(shell_command_covers_preset(
        "echo hi && cargo test",
        "cargo test"
    ));
    assert!(!shell_command_covers_preset("cargo check", "cargo test"));
    assert!(!shell_command_covers_preset(
        "echo cargo test",
        "cargo test"
    ));
}

#[test]
fn shell_verification_report_attaches_for_workspace_preset_commands() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("Cargo.toml"), "[package]\nname=\"demo\"\n").unwrap();
    let report = shell_verification_report_for_command(
        root.path(),
        "cargo test -p demo",
        0,
        Some(&serde_json::json!({"exit_code": 0})),
        None,
    )
    .expect("rust preset should match cargo test");
    assert_eq!(report.subject, "shell:rust:test");
    assert_eq!(report.status, VerificationStatus::Passed);
    assert!(report.checks.iter().any(|check| check.required));
}

#[test]
fn shell_verification_report_attaches_for_goal_acceptance_commands() {
    let root = tempfile::tempdir().unwrap();
    let loop_dir = root
        .path()
        .join(".a3s")
        .join("loops")
        .join("goal-demo-abcd");
    std::fs::create_dir_all(&loop_dir).unwrap();
    write_active_loop_state(&loop_dir, "running");
    std::fs::write(
        loop_dir.join("ACCEPTANCE.md"),
        "# Acceptance\n\n\
         - [ ] kind:command assert:`true` expect:exit=0\n\
         - [x] kind:file_exists assert:README.md\n\
         - [ ] kind:manual assert:reviewed\n",
    )
    .unwrap();

    let commands = acceptance_shell_commands_for_workspace(root.path());
    assert_eq!(commands.len(), 2, "{commands:?}");
    assert_eq!(commands[0].command, "true");
    assert_eq!(commands[0].expect_exit, 0);
    assert_eq!(commands[1].command, "test -f README.md");
    assert_eq!(commands[1].expect_exit, 0);

    let report = shell_verification_report_for_command(
        root.path(),
        "true",
        0,
        Some(&serde_json::json!({"exit_code": 0})),
        None,
    )
    .expect("ACCEPTANCE kind:command should produce a Core verification report");
    assert!(report.subject.contains("acceptance:goal-demo-abcd"));
    assert_eq!(report.status, VerificationStatus::Passed);
    assert!(
        VerificationSummary::from_reports(&[report.clone()]).supports_goal_achievement(),
        "passing ACCEPTANCE command evidence must authorize GoalAchieved gate"
    );

    let failed = shell_verification_report_for_command(root.path(), "true", 1, None, None)
        .expect("failed exit still yields a report");
    assert_eq!(failed.status, VerificationStatus::Failed);
    assert!(!VerificationSummary::from_reports(&[failed]).supports_goal_achievement());

    let file_ok = shell_verification_report_for_command(
        root.path(),
        "test -f README.md",
        0,
        Some(&serde_json::json!({"exit_code": 0})),
        None,
    )
    .expect("ACCEPTANCE kind:file_exists must attach via synthesized test -f");
    assert!(file_ok.subject.contains("acceptance:goal-demo-abcd"));
    assert_eq!(file_ok.status, VerificationStatus::Passed);
    assert!(VerificationSummary::from_reports(&[file_ok]).supports_goal_achievement());

    let file_missing =
        shell_verification_report_for_command(root.path(), "test -f README.md", 1, None, None)
            .expect("missing file still yields a report");
    assert_eq!(file_missing.status, VerificationStatus::Failed);
    assert!(!VerificationSummary::from_reports(&[file_missing]).supports_goal_achievement());
}

#[test]
fn acceptance_file_exists_quotes_paths_with_spaces() {
    let root = tempfile::tempdir().unwrap();
    let body = "# Acceptance\n\n\
         - [ ] kind:file_exists assert:`docs/my file.md`\n";
    let commands = super::parse_acceptance_shell_commands(body, "goal-quote", root.path());
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert_eq!(commands[0].command, "test -f 'docs/my file.md'");
}

#[test]
fn acceptance_file_exists_skips_paths_that_escape_workspace() {
    let root = tempfile::tempdir().unwrap();
    let loop_dir = root.path().join(".a3s").join("loops").join("goal-escape");
    std::fs::create_dir_all(&loop_dir).unwrap();
    write_active_loop_state(&loop_dir, "running");
    std::fs::write(
        loop_dir.join("ACCEPTANCE.md"),
        "- [ ] kind:file_exists assert:../outside.txt\n\
         - [ ] kind:file_exists assert:ok.txt\n\
         - [ ] kind:command assert:`true` expect:exit=0\n",
    )
    .unwrap();

    let commands = acceptance_shell_commands_for_workspace(root.path());
    assert_eq!(commands.len(), 2, "{commands:?}");
    assert!(
        commands.iter().any(|c| c.command == "test -f ok.txt"),
        "{commands:?}"
    );
    assert!(commands.iter().any(|c| c.command == "true"), "{commands:?}");
    assert!(
        commands
            .iter()
            .all(|c| !c.command.contains("outside") && !c.command.contains("..")),
        "escaping file_exists must not become Core shell evidence: {commands:?}"
    );
}
