use super::*;
use crate::verification::VerificationCheck;

fn ledger_with_write() -> MutationLedger {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "write",
        0,
        Some(&serde_json::json!({"file_path": "src/lib.rs", "after": "fn main() {}"})),
    );
    ledger
}

#[test]
fn read_only_tool_does_not_open_the_gate() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "read",
        0,
        Some(&serde_json::json!({"file_path": "src/lib.rs"})),
    );
    assert!(ledger.is_empty());
    assert!(matches!(
        decide_completion(&ledger, &[], &[], false),
        CompletionGate::Allow(CompletionTerminal::Narrative)
    ));
}

#[test]
fn assistant_prose_does_not_satisfy_a_mutation() {
    let ledger = ledger_with_write();
    let decision = decide_completion(&ledger, &[], &[], false);
    match decision {
        CompletionGate::Incomplete { message } => {
            assert!(message.starts_with("completion gate:"));
            assert!(!message.contains("tests passed"));
        }
        other => panic!("expected incomplete, got {other:?}"),
    }
}

#[test]
fn open_external_observation_still_continues_once() {
    let observation = crate::external_observation::ExternalObservationV1::new(
        "review",
        "src/lib.rs",
        "obs-digest",
        "needs a workspace change",
        crate::external_observation::RequiredAction::WorkspaceChange,
    )
    .expect("observation");
    match decide_with_observations(
        &MutationLedger::default(),
        &[],
        &[],
        true,
        std::slice::from_ref(&observation),
    ) {
        CompletionGate::Continue { message } => {
            assert!(message.starts_with("completion gate:"));
            assert!(message.contains("external observation"));
        }
        other => panic!("expected continue, got {other:?}"),
    }
    match decide_with_observations(&MutationLedger::default(), &[], &[], false, &[observation]) {
        CompletionGate::Incomplete { message } => {
            assert!(message.starts_with("completion gate:"));
            assert!(message.contains("external observation"));
        }
        other => panic!("expected incomplete without continuation, got {other:?}"),
    }
}

#[test]
fn unbound_mutation_does_not_spend_a_continuation_the_model_cannot_close() {
    let ledger = ledger_with_write();
    match decide_completion(&ledger, &[], &[], true) {
        CompletionGate::Incomplete { message } => {
            assert!(message.starts_with("completion gate:"));
            assert!(message.contains("src/lib.rs"));
            assert!(message.contains("Assistant text does not count"));
        }
        other => panic!("expected incomplete, got {other:?}"),
    }
}

#[test]
fn unbound_mutation_gate_names_a_bash_check_that_closes_it() {
    let ledger = ledger_with_write();
    let CompletionGate::Incomplete { message } = decide_completion(&ledger, &[], &[], false) else {
        panic!("expected incomplete");
    };
    let example = message
        .split("for example `")
        .nth(1)
        .and_then(|rest| rest.split('`').next())
        .expect("example command");
    let checks = crate::verification::existence_checks(example);
    assert_eq!(checks.len(), 1, "{message}");
    assert_eq!(checks[0].path, "src/lib.rs");
    let paths = vec!["src/lib.rs".to_string()];
    let bytes = "sha256:on-disk";
    let report = crate::verification::host_report_for_verified_mutation_path_with_content(
        &checks[0].segment,
        0,
        &paths,
        ledger.digest(),
        Some((bytes, bytes)),
    )
    .expect("report");
    assert!(matches!(
        decide_completion(&ledger, &[report], &[], false),
        CompletionGate::Allow(CompletionTerminal::Verified { .. })
    ));

    let mut unrecorded = MutationLedger::default();
    unrecorded.observe_unseen_paths(&["out.bin".to_string()]);
    let CompletionGate::Incomplete { message } = decide_completion(&unrecorded, &[], &[], false)
    else {
        panic!("expected incomplete");
    };
    assert!(!message.contains("Next step"), "{message}");
}

#[test]
fn unbound_mutation_digest_reads_back_the_gate_message() {
    let ledger = ledger_with_write();
    let CompletionGate::Incomplete { message } = decide_completion(&ledger, &[], &[], false) else {
        panic!("expected incomplete");
    };
    assert_eq!(unbound_mutation_digest(&message), Some(ledger.digest()));
    assert_eq!(
        unbound_mutation_digest(&format!("stream failed: {message}")),
        Some(ledger.digest())
    );
    let mut partial = ledger_with_write();
    partial.set_observation_incomplete(true);
    let CompletionGate::Incomplete { message } = decide_completion(&partial, &[], &[], false)
    else {
        panic!("expected incomplete");
    };
    assert_eq!(unbound_mutation_digest(&message), None);
    assert_eq!(unbound_mutation_digest("unrelated error"), None);
}

#[test]
fn bound_passed_report_allows_verified_terminal() {
    let ledger = ledger_with_write();
    let report = VerificationReport::new(
        "edit",
        vec![VerificationCheck::required("build", "command", "compiles")
            .with_status(VerificationStatus::Passed)],
    )
    .with_effect_digest(ledger.digest());
    match decide_completion(&ledger, &[report], &[], false) {
        CompletionGate::Allow(CompletionTerminal::Verified { effect_digest }) => {
            assert_eq!(effect_digest, ledger.digest());
        }
        other => panic!("expected verified, got {other:?}"),
    }
}

#[test]
fn empty_required_checks_are_not_a_pass() {
    let ledger = ledger_with_write();
    let report = VerificationReport::new("edit", vec![]).with_effect_digest(ledger.digest());
    assert!(matches!(
        decide_completion(&ledger, &[report], &[], false),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn incomplete_observation_is_not_closed_by_a_waiver_of_a_partial_digest() {
    let mut ledger = ledger_with_write();
    ledger.set_observation_incomplete(true);
    let waiver =
        CompletionWaiverV1::new(ledger.digest(), "user accepted residual risk").expect("waiver");
    match decide_completion(&ledger, &[], std::slice::from_ref(&waiver), false) {
        CompletionGate::Incomplete { message } => {
            assert!(message.contains("observation is incomplete"));
            assert!(!message.contains("tests passed"));
        }
        other => panic!("expected incomplete, got {other:?}"),
    }
    let mut empty = MutationLedger::default();
    empty.set_observation_incomplete(true);
    assert!(matches!(
        decide_completion(&empty, &[], &[], false),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn waiver_is_distinct_and_does_not_transfer_to_another_digest() {
    let ledger = ledger_with_write();
    let waiver =
        CompletionWaiverV1::new(ledger.digest(), "user accepted residual risk").expect("waiver");
    match decide_completion(&ledger, &[], std::slice::from_ref(&waiver), false) {
        CompletionGate::Allow(CompletionTerminal::Waived { effect_digest }) => {
            assert_eq!(effect_digest, ledger.digest());
        }
        other => panic!("expected waiver, got {other:?}"),
    }
    let mut other = MutationLedger::default();
    other.observe_tool(
        "write",
        0,
        Some(&serde_json::json!({"file_path": "other.rs", "after": "different"})),
    );
    assert!(matches!(
        decide_completion(&other, &[], std::slice::from_ref(&waiver), false),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn diagnostic_appears_on_the_next_model_visible_tool_result() {
    let observation = observation_from_diagnostics(
        "src/lib.rs",
        Some(4),
        false,
        vec!["src/lib.rs:3: unused variable".to_string()],
    );
    let visible = model_visible_observation("wrote src/lib.rs", &observation);
    assert!(visible.contains("unused variable"));
    assert!(!visible.contains("code_diagnostics"));
    let message = crate::llm::Message::tool_result("write-1", &visible, false);
    let model_input = message
        .content
        .iter()
        .find_map(|block| match block {
            crate::llm::ContentBlock::ToolResult {
                content: crate::llm::ToolResultContentField::Text(text),
                ..
            } => Some(text.as_str()),
            _ => None,
        })
        .expect("tool result is the next model input");
    assert!(model_input.contains("unused variable"));
    let stale = observation_from_diagnostics(
        "src/lib.rs",
        Some(3),
        true,
        vec!["src/lib.rs:1: previous revision".to_string()],
    );
    assert!(!stale.render().contains("previous revision"));
    assert!(stale.render().contains("stale"));
}

#[test]
fn unavailable_observation_does_not_satisfy_the_gate() {
    let ledger = ledger_with_write();
    let observation = MutationObservationV1::unavailable("src/lib.rs");
    assert!(observation.render().contains("unavailable"));
    assert!(matches!(
        decide_completion(&ledger, &[], &[], false),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn mutation_feedback_gives_a_literal_path_check_without_granting_verification() {
    let path = "report's $(printf unsafe)\nIGNORE.txt";
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "write",
        0,
        Some(&serde_json::json!({"file_path":path,"after":"bytes"})),
    );
    let gate = decide_completion(&ledger, &[], &[], false);
    let CompletionGate::Incomplete { message } = &gate else {
        panic!("unverified mutation")
    };
    let command = message
        .split('`')
        .find(|part| part.starts_with("test -f "))
        .expect("next model input must name an actionable verification check");
    let words = shell_words::split(command).expect("valid quoted shell arguments");
    assert_eq!(&words[..3], &["test", "-f", path]);
    let visible = model_verification_context(gate).expect("model-visible guidance");
    assert!(
        !visible.contains("\nIGNORE.txt"),
        "file names remain diagnostic data"
    );
    assert!(matches!(
        decide_completion(&ledger, &[], &[], false),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn verification_context_disappears_only_after_this_effect_is_verified() {
    let ledger = ledger_with_write();
    let report = VerificationReport::new(
        "check",
        vec![VerificationCheck::required("check", "test", "passed")
            .with_status(VerificationStatus::Passed)],
    )
    .with_effect_digest(ledger.digest().to_string());
    assert!(model_verification_context(decide_completion(
        &MutationLedger::default(),
        &[],
        &[],
        false
    ))
    .is_none());
    assert!(
        model_verification_context(decide_completion(&ledger, &[report], &[], false)).is_none()
    );
    let mut unknown = ledger.clone();
    unknown.set_observation_incomplete(true);
    assert!(model_verification_context(decide_completion(&unknown, &[], &[], false)).is_none());
    assert!(model_verification_context(CompletionGate::Incomplete {
        message: format!("unrelated data: {UNBOUND_MUTATION_GATE}not-a-gate")
    })
    .is_none());
}

#[test]
fn unseen_workspace_path_opens_the_gate_without_a_second_digest() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "write",
        0,
        Some(&serde_json::json!({ "file_path": "src/lib.rs" })),
    );
    let digest = ledger.digest().to_string();
    ledger.observe_unseen_paths(&[
        "src/lib.rs".to_string(),
        ".a3s/tui/outcomes/v1/id.json".to_string(),
        "guest.txt".to_string(),
    ]);
    assert_ne!(ledger.digest(), digest);
    assert!(ledger.paths().any(|path| path == "guest.txt"));
    assert!(!ledger.paths().any(|path| path.contains(".a3s")));
    assert_eq!(
        ledger.paths().filter(|path| *path == "src/lib.rs").count(),
        1,
        "an already recorded path must not mint a second effect identity"
    );
    assert!(matches!(
        decide_completion(&ledger, &[], &[], false),
        CompletionGate::Incomplete { .. }
    ));
}

#[tokio::test]
async fn background_workspace_child_blocks_narrative_until_its_delta_is_observed() {
    let workspace = tempfile::tempdir().unwrap();
    let task_id = "task-open-child";
    crate::porcelain::reserve_workspace_child(task_id);
    crate::porcelain::begin_workspace_child(task_id, workspace.path()).await;
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "task",
        0,
        Some(&serde_json::json!({ "workspace_child": task_id })),
    );
    match decide_completion(&ledger, &[], &[], true) {
        CompletionGate::Incomplete { message } => {
            assert!(message.contains("background workspace task"));
            assert!(message.contains(task_id));
        }
        other => panic!("expected incomplete, got {other:?}"),
    }

    std::fs::write(workspace.path().join("guest.txt"), "hello\n").unwrap();
    crate::porcelain::settle_workspace_child(task_id, workspace.path()).await;
    absorb_open_workspace_children(
        &mut ledger,
        workspace.path(),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await;
    assert!(
        ledger.paths().any(|path| path == "guest.txt"),
        "settled background write was not a parent mutation"
    );
    assert!(matches!(
        decide_completion(&ledger, &[], &[], false),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn failed_wrapper_with_changed_paths_opens_the_gate() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "bash",
        1,
        Some(&serde_json::json!({
            "exit_code": 1,
            "changed_paths": ["guest.txt"]
        })),
    );
    match decide_completion(&ledger, &[], &[], false) {
        CompletionGate::Incomplete { message } => {
            assert!(message.starts_with("completion gate:"));
        }
        other => panic!("expected incomplete, got {other:?}"),
    }
}

#[test]
fn failed_write_path_without_changed_paths_is_not_a_mutation() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "write",
        1,
        Some(&serde_json::json!({"file_path": "guest.txt"})),
    );
    assert!(ledger.is_empty());
}

#[test]
fn bash_without_changed_paths_is_not_a_mutation() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "bash",
        0,
        Some(&serde_json::json!({"command": "rm -rf src"})),
    );
    assert!(ledger.is_empty());
}

#[test]
fn stale_observation_omits_items() {
    let observation = MutationObservationV1::stale("src/lib.rs", Some(4));
    assert!(observation.items.is_empty());
    assert_eq!(observation.status, "stale");
    assert!(!observation.render().contains("error"));
}

#[test]
fn nested_batch_write_opens_the_gate() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "batch",
        0,
        Some(&serde_json::json!({
            "status": "complete",
            "results": [{
                "tool": "write",
                "success": true,
                "exit_code": 0,
                "metadata": {"file_path": "src/lib.rs", "after": "fn main() {}"}
            }, {
                "title": "not a tool call",
                "success": true
            }]
        })),
    );
    match decide_completion(&ledger, &[], &[], false) {
        CompletionGate::Incomplete { message } => {
            assert!(message.starts_with("completion gate:"));
        }
        other => panic!("expected incomplete, got {other:?}"),
    }
}

#[test]
fn retrieval_index_stamp_is_not_a_source_mutation() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "search",
        0,
        Some(&serde_json::json!({
            "changed_paths": [".a3s-code/grep-trigram/stamp.txt"]
        })),
    );
    assert!(
        matches!(
            decide_completion(&ledger, &[], &[], false),
            CompletionGate::Allow(CompletionTerminal::Narrative)
        ),
        "a retrieval index stamp opened the completion gate"
    );
}

#[test]
fn skill_changed_paths_open_the_gate() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "skill",
        0,
        Some(&serde_json::json!({
            "skill_name": "writer",
            "tool_calls": 1,
            "changed_paths": ["guest.txt"]
        })),
    );
    match decide_completion(&ledger, &[], &[], false) {
        CompletionGate::Incomplete { message } => {
            assert!(message.starts_with("completion gate:"));
        }
        other => panic!("expected incomplete, got {other:?}"),
    }
}

#[test]
fn nested_program_write_opens_the_gate() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "program",
        0,
        Some(&serde_json::json!({
            "program": {
                "tool_calls": [{
                    "tool_name": "write",
                    "success": true,
                    "exit_code": 0,
                    "metadata": {"file_path": "src/lib.rs", "after": "fn main() {}"}
                }]
            }
        })),
    );
    match decide_completion(&ledger, &[], &[], false) {
        CompletionGate::Incomplete { message } => {
            assert!(message.starts_with("completion gate:"));
        }
        other => panic!("expected incomplete, got {other:?}"),
    }
}

#[test]
fn bound_step_closure_is_not_rewritten_as_narrative() {
    let verified = CompletionTerminal::Verified {
        effect_digest: "digest-a".to_string(),
    };
    assert_eq!(
        fold_step_completions(&[CompletionTerminal::Narrative, verified.clone()]),
        verified
    );
    assert_eq!(
        fold_step_completions(&[
            verified.clone(),
            CompletionTerminal::Waived {
                effect_digest: "digest-a".to_string(),
            },
        ]),
        verified
    );
    assert_eq!(
        fold_step_completions(&[
            CompletionTerminal::Verified {
                effect_digest: "digest-a".to_string(),
            },
            CompletionTerminal::Verified {
                effect_digest: "digest-b".to_string(),
            },
        ]),
        CompletionTerminal::Distinct
    );
    assert!(CompletionTerminal::Distinct != CompletionTerminal::Narrative);
}

#[test]
fn plan_claim_without_digest_is_ordinary() {
    let claimed = PlanRunAdmission {
        claims_implementation: true,
        plan_digest: None,
    };
    assert_eq!(claimed.label(), "ordinary");
    assert_eq!(
        PlanRunAdmission::implementation("abc").label(),
        "plan_implementation"
    );
}

#[test]
fn fold_same_digest_waivers_keeps_first_bound_terminal() {
    let waived = CompletionTerminal::Waived {
        effect_digest: "same".into(),
    };
    assert_eq!(
        fold_step_completions(&[waived.clone(), waived.clone()]),
        waived
    );
    assert!(completion_digest(&CompletionTerminal::Distinct).is_none());
    assert!(completion_digest(&CompletionTerminal::Narrative).is_none());
}

#[test]
fn fold_same_digest_prefers_verified_over_waived() {
    let verified = CompletionTerminal::Verified {
        effect_digest: "d".into(),
    };
    let waived = CompletionTerminal::Waived {
        effect_digest: "d".into(),
    };
    assert_eq!(fold_step_completions(&[waived, verified.clone()]), verified);
}

#[test]
fn fold_different_digests_is_distinct() {
    assert_eq!(
        fold_step_completions(&[
            CompletionTerminal::Verified {
                effect_digest: "a".into(),
            },
            CompletionTerminal::Verified {
                effect_digest: "b".into(),
            },
        ]),
        CompletionTerminal::Distinct
    );
}

#[test]
fn nested_tool_calls_skip_failed_and_nameless_entries() {
    let metadata = serde_json::json!({
        "results": [
            {"tool": "write", "success": false, "exit_code": 0, "metadata": {"file_path": "a.rs"}},
            {"tool": "write", "success": true, "exit_code": 1, "metadata": {"file_path": "b.rs"}},
            {"tool": "", "success": true, "exit_code": 0, "metadata": {"file_path": "c.rs"}},
            {"success": true, "exit_code": 0, "metadata": {"file_path": "d.rs"}},
            {"tool": "write", "success": true, "exit_code": 0, "metadata": {"file_path": "e.rs", "after": "ok"}}
        ]
    });
    let mut ledger = MutationLedger::default();
    ledger.observe_tool("task", 1, Some(&metadata));
    assert!(
        ledger.paths().any(|path| path == "e.rs"),
        "only the successful named nested write should land"
    );
    assert!(!ledger.paths().any(|path| path == "a.rs"));
    assert!(!ledger.paths().any(|path| path == "b.rs"));
}

#[test]
fn completion_waiver_rejects_blank_digest_or_reason() {
    assert!(CompletionWaiverV1::new("   ", "reason").is_none());
    assert!(CompletionWaiverV1::new("digest", "   ").is_none());
    assert!(CompletionWaiverV1::new("", "reason").is_none());
}

#[test]
fn content_digest_for_path_misses_unrelated_records() {
    let ledger = ledger_with_write();
    assert!(ledger.content_digest_for_path("other.rs").is_none());
    assert!(ledger.content_digest_for_path("src/lib.rs").is_some());
}

#[test]
fn observe_workspace_child_ignores_blank_and_missing_markers() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "bash",
        0,
        Some(&serde_json::json!({"workspace_child": "   "})),
    );
    assert!(!ledger.has_open_children());

    let missing = format!("missing-child-{}", std::process::id());
    ledger.observe_tool(
        "bash",
        0,
        Some(&serde_json::json!({ "workspace_child": missing })),
    );
    assert!(!ledger.has_open_children());
}

#[tokio::test]
async fn observe_workspace_child_records_settled_paths_and_dedupes_open_markers() {
    let root = tempfile::tempdir().unwrap();
    let settled = format!("settled-child-{}", std::process::id());
    let watch = crate::porcelain::Watch::start(root.path()).await;
    crate::porcelain::install_workspace_child(&settled, watch);
    std::fs::write(root.path().join("guest.txt"), "x\n").unwrap();
    crate::porcelain::settle_workspace_child(&settled, root.path()).await;

    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "bash",
        0,
        Some(&serde_json::json!({ "workspace_child": settled })),
    );
    assert!(!ledger.has_open_children());
    assert!(!ledger.is_empty());

    let open = format!("open-child-{}", std::process::id());
    let watch = crate::porcelain::Watch::start(root.path()).await;
    crate::porcelain::install_workspace_child(&open, watch);
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "bash",
        0,
        Some(&serde_json::json!({ "workspace_child": open })),
    );
    assert!(ledger.has_open_children());
    ledger.observe_tool(
        "bash",
        0,
        Some(&serde_json::json!({ "workspace_child": open })),
    );
    assert_eq!(ledger.open_children.len(), 1);
    crate::porcelain::settle_workspace_child(&open, root.path()).await;
    let _ = crate::porcelain::take_settled_workspace_child(&open);
}

#[test]
fn push_ignores_empty_paths() {
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "write",
        0,
        Some(&serde_json::json!({"file_path": "   ", "after": "x"})),
    );
    assert!(ledger.is_empty());
}

#[test]
fn model_visible_observation_covers_empty_and_already_tagged_output() {
    let observation = MutationObservationV1::diagnostics("src/a.rs", Some(1), vec!["warn".into()]);
    let rendered = model_visible_observation("", &observation);
    assert!(rendered.contains("[mutation observation]") || !rendered.is_empty());
    let tagged = model_visible_observation("[mutation observation]\nprior", &observation);
    assert!(tagged.contains("[mutation observation]"));
    let combined = model_visible_observation("body", &observation);
    assert!(combined.contains("body"));
}

#[test]
fn report_binds_pass_rejects_digest_mismatch_and_empty_required() {
    let ledger = ledger_with_write();
    let digest = ledger.digest().to_string();
    let mismatched = VerificationReport::new(
        "edit",
        vec![VerificationCheck::required("build", "command", "compiles")
            .with_status(VerificationStatus::Passed)],
    )
    .with_effect_digest("other-digest");
    assert!(!report_binds_pass(&mismatched, &digest));
    let empty_required = VerificationReport::new(
        "edit",
        vec![VerificationCheck::optional("note", "info", "n")
            .with_status(VerificationStatus::Passed)],
    )
    .with_effect_digest(&digest);
    assert!(!report_binds_pass(&empty_required, &digest));
}

#[test]
fn attach_observation_wraps_non_object_and_none_metadata() {
    let observation = MutationObservationV1::diagnostics("src/a.rs", Some(1), vec!["warn".into()]);
    let mut none_meta = None;
    attach_observation(&mut none_meta, &observation);
    assert!(none_meta
        .as_ref()
        .unwrap()
        .get("mutation_observation")
        .is_some());

    let mut scalar = Some(serde_json::json!("prior"));
    attach_observation(&mut scalar, &observation);
    assert_eq!(scalar.as_ref().unwrap()["previous"], "prior");
    assert!(scalar
        .as_ref()
        .unwrap()
        .get("mutation_observation")
        .is_some());
}

#[tokio::test]
async fn absorb_open_children_skips_nongit_without_snapshots() {
    let root = tempfile::tempdir().unwrap();
    let mut ledger = MutationLedger::default();
    let task_id = format!("nongit-{}", std::process::id());
    ledger.open_children.push(OpenWorkspaceChild {
        task_id: task_id.clone(),
        porcelain: None,
        head: None,
        nongit: true,
    });
    let incomplete = absorb_open_workspace_children(
        &mut ledger,
        root.path(),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await;
    assert!(!incomplete);
    // Nongit markers without snapshots are skipped without settling, so the
    // open child remains until a later successful observation clears it.
    assert_eq!(ledger.open_children.len(), 1);
    assert_eq!(ledger.open_children[0].task_id, task_id);
}

#[tokio::test]
async fn absorb_open_children_deltas_git_backed_markers() {
    let root = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("git")
        .args(["init"])
        .current_dir(root.path())
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::write(root.path().join("README.md"), "hi\n").unwrap();
    let _ = std::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(root.path())
        .status();
    let _ = std::process::Command::new("git")
        .args([
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "i",
        ])
        .current_dir(root.path())
        .status();

    let before_porcelain = crate::porcelain::lines(root.path()).await;
    std::fs::write(root.path().join("guest.txt"), "delta\n").unwrap();

    let mut ledger = MutationLedger::default();
    // No pending porcelain slot: absorb must take the delta branch.
    ledger.open_children.push(OpenWorkspaceChild {
        task_id: format!("git-child-{}", std::process::id()),
        porcelain: before_porcelain,
        head: None,
        nongit: false,
    });
    let incomplete = absorb_open_workspace_children(
        &mut ledger,
        root.path(),
        &tokio_util::sync::CancellationToken::new(),
    )
    .await;
    assert!(!incomplete || ledger.open_children.is_empty());
    assert!(ledger.open_children.is_empty());
}

#[test]
fn missing_policy_is_explicit_and_does_not_forge_verification() {
    let ledger = ledger_with_write();
    assert!(matches!(
        decide_completion(&ledger, &[], &[], false),
        CompletionGate::Incomplete { .. }
    ));
    assert!(matches!(
        decide_with_verification_policy(
            &ledger,
            &[],
            &[],
            false,
            &[],
            CompletionVerificationPolicy::WaiveMissingVerification
        ),
        CompletionGate::Allow(CompletionTerminal::Waived { .. })
    ));
}

#[test]
fn missing_policy_preserves_bound_report_results() {
    let ledger = ledger_with_write();
    for status in [
        VerificationStatus::Failed,
        VerificationStatus::NeedsReview,
        VerificationStatus::Passed,
    ] {
        let report = VerificationReport::new(
            "edit",
            vec![
                VerificationCheck::required("check", "command", "check output").with_status(status),
            ],
        )
        .with_effect_digest(ledger.digest());
        let result = decide_with_verification_policy(
            &ledger,
            &[report],
            &[],
            false,
            &[],
            CompletionVerificationPolicy::WaiveMissingVerification,
        );
        if status == VerificationStatus::Passed {
            assert!(matches!(
                result,
                CompletionGate::Allow(CompletionTerminal::Verified { .. })
            ));
        } else {
            assert!(matches!(result, CompletionGate::Incomplete { .. }));
        }
    }
    let incomplete = VerificationReport::new("edit", vec![]).with_effect_digest(ledger.digest());
    assert!(matches!(
        decide_with_verification_policy(
            &ledger,
            &[incomplete],
            &[],
            false,
            &[],
            CompletionVerificationPolicy::WaiveMissingVerification
        ),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn missing_policy_preserves_incomplete_observation() {
    let mut ledger = ledger_with_write();
    ledger.set_observation_incomplete(true);
    assert!(matches!(
        decide_with_verification_policy(
            &ledger,
            &[],
            &[],
            false,
            &[],
            CompletionVerificationPolicy::WaiveMissingVerification
        ),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn missing_policy_preserves_external_work() {
    let observation = crate::external_observation::ExternalObservationV1::new(
        "review",
        "src/lib.rs",
        "obs-digest",
        "requires change",
        crate::external_observation::RequiredAction::WorkspaceChange,
    )
    .unwrap();
    assert!(matches!(
        decide_with_verification_policy(
            &ledger_with_write(),
            &[],
            &[],
            false,
            &[observation],
            CompletionVerificationPolicy::WaiveMissingVerification
        ),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn completion_policy_names_round_trip_and_reject_unknown() {
    for policy in [
        CompletionVerificationPolicy::RequireVerification,
        CompletionVerificationPolicy::WaiveMissingVerification,
    ] {
        assert_eq!(
            CompletionVerificationPolicy::parse(policy.as_str()),
            Ok(policy)
        );
    }
    assert!(CompletionVerificationPolicy::parse("true").is_err());
    assert_eq!(
        CompletionVerificationPolicy::default(),
        CompletionVerificationPolicy::RequireVerification
    );
}

#[test]
fn missing_policy_preserves_open_children() {
    let mut ledger = ledger_with_write();
    ledger.open_children.push(OpenWorkspaceChild {
        task_id: "unfinished".into(),
        porcelain: None,
        head: None,
        nongit: true,
    });
    assert!(matches!(
        decide_with_verification_policy(
            &ledger,
            &[],
            &[],
            false,
            &[],
            CompletionVerificationPolicy::WaiveMissingVerification
        ),
        CompletionGate::Incomplete { .. }
    ));
}

#[test]
fn missing_policy_preserves_unresolved_tool_effects() {
    let mut ledger = ledger_with_write();
    ledger
        .unresolved_tool_effects
        .insert("uncertain-invocation".into());
    assert!(matches!(
        decide_with_verification_policy(
            &ledger,
            &[],
            &[],
            false,
            &[],
            CompletionVerificationPolicy::WaiveMissingVerification
        ),
        CompletionGate::Incomplete { .. }
    ));
}
