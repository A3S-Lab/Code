#![cfg(test)]

use super::*;
use crate::harness_loop::{CompletionGate, MutationLedger};
use crate::tools::{ToolContext, ToolExecutor};
use crate::verification::{
    VerificationCheck, VerificationContractV1, VerificationReport, VerificationStatus,
};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

const SOURCE: &str = "exact 甲.rs";
const BYTES: &str = "the actual file bytes\n";
const BYTE_CHECK: &str = "test -f 'exact 甲.rs'";
const SUITE: &str = "python -m unittest";

fn agent(root: &Path, contracts: Vec<VerificationContractV1>) -> AgentLoop {
    AgentLoop::new(
        Arc::new(crate::agent::tests::MockLlmClient::new(Vec::new())),
        Arc::new(ToolExecutor::new(root.to_string_lossy().into_owned())),
        ToolContext::new(root.to_path_buf()),
        crate::agent::AgentConfig {
            verification_contracts: contracts,
            ..crate::agent::AgentConfig::default()
        },
    )
}

fn contract() -> VerificationContractV1 {
    VerificationContractV1::new("host-frozen-suite", SUITE, [SOURCE]).unwrap()
}

fn observed_bytes(root: &Path) -> MutationLedger {
    std::fs::write(root.join(SOURCE), BYTES).unwrap();
    let mut ledger = MutationLedger::default();
    ledger.observe_tool(
        "write",
        0,
        Some(&json!({"file_path": SOURCE, "after": BYTES})),
    );
    ledger
}

fn report(subject: &str, status: VerificationStatus) -> VerificationReport {
    VerificationReport::new(
        subject,
        vec![
            VerificationCheck::required("observed-check", "test", "observed check")
                .with_status(status),
        ],
    )
}

fn call(command: &str) -> ToolCall {
    ToolCall {
        id: "this-executed-call".into(),
        name: "bash".into(),
        args: json!({"command": command}),
    }
}

fn replay_report(metadata: &Option<Value>) -> Vec<VerificationReport> {
    let wire = serde_json::to_vec(metadata).unwrap();
    let original_metadata = serde_json::from_slice(&wire).unwrap();
    let mut reports = Vec::new();
    AgentLoop::collect_verification_report(&mut reports, &original_metadata);
    reports
}

async fn record(
    agent: &AgentLoop,
    command: &str,
    exit_code: i32,
    metadata: &mut Option<Value>,
    ledger: &mut MutationLedger,
    reports: &mut Vec<VerificationReport>,
) {
    let mut output = "original executed output".to_string();
    agent
        .record_prepared_tool_verification(
            &call(command),
            exit_code,
            &mut output,
            metadata,
            ledger,
            reports,
        )
        .await;
    assert_eq!(output, "original executed output");
}

#[tokio::test]
async fn prepared_byte_check_round_trips_bound_report_in_original_metadata() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = agent(workspace.path(), Vec::new());
    let mut ledger = observed_bytes(workspace.path());
    let digest = ledger.digest().to_string();
    let mut reports = Vec::new();
    let mut metadata = Some(json!({
        "command": BYTE_CHECK,
        "retained": {"exact": "original metadata"}
    }));
    record(
        &agent,
        BYTE_CHECK,
        0,
        &mut metadata,
        &mut ledger,
        &mut reports,
    )
    .await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].effect_digest.as_deref(), Some(digest.as_str()));
    assert_eq!(reports[0].status, VerificationStatus::Passed);
    assert_eq!(
        metadata.as_ref().unwrap()["retained"]["exact"],
        "original metadata"
    );
    assert_eq!(
        metadata.as_ref().unwrap()["verification_report"],
        reports[0].to_value()
    );
    assert_eq!(replay_report(&metadata), reports);

    // A later command that sees different bytes cannot acquire this binding.
    std::fs::write(workspace.path().join(SOURCE), "different bytes").unwrap();
    let mut later = Some(json!({"command": BYTE_CHECK}));
    let mut later_reports = Vec::new();
    record(
        &agent,
        BYTE_CHECK,
        0,
        &mut later,
        &mut ledger,
        &mut later_reports,
    )
    .await;
    assert!(later_reports.is_empty());
    assert!(later.as_ref().unwrap().get("verification_report").is_none());
}

#[tokio::test]
async fn prepared_host_suite_round_trips_one_synthesized_report_without_a_slot() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = agent(workspace.path(), vec![contract()]);
    let mut ledger = observed_bytes(workspace.path());
    let digest = ledger.digest().to_string();
    let mut reports = Vec::new();
    let mut metadata = Some(json!({"command": SUITE, "retained": 7}));
    // These are host-result projection fixtures, not evidence that this test
    // executed an interpreter or suite. No model or command executor is called.
    record(&agent, SUITE, 0, &mut metadata, &mut ledger, &mut reports).await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].subject, "shell:contract:host-frozen-suite");
    assert_eq!(reports[0].effect_digest.as_deref(), Some(digest.as_str()));
    assert_eq!(metadata.as_ref().unwrap()["retained"], 7);
    assert_eq!(replay_report(&metadata), reports);
    let mut absent_metadata = None;
    let mut without_slot = Vec::new();
    record(
        &agent,
        SUITE,
        0,
        &mut absent_metadata,
        &mut ledger,
        &mut without_slot,
    )
    .await;
    assert_eq!(without_slot, reports);
    assert_eq!(replay_report(&absent_metadata), without_slot);
    for (command, code) in [("python other.py", 0), (SUITE, 1)] {
        let mut metadata = Some(json!({"command": command}));
        let mut reports = Vec::new();
        record(
            &agent,
            command,
            code,
            &mut metadata,
            &mut ledger,
            &mut reports,
        )
        .await;
        assert!(reports.is_empty());
        assert!(metadata
            .as_ref()
            .unwrap()
            .get("verification_report")
            .is_none());
    }
}

#[tokio::test]
async fn current_required_pass_is_bound_written_back_and_prior_report_is_unchanged() {
    for author in ["host", "verifier"] {
        let workspace = tempfile::tempdir().unwrap();
        let agent = agent(workspace.path(), Vec::new());
        let mut ledger = observed_bytes(workspace.path());
        let current = report("this-call", VerificationStatus::Passed);
        let prior = report("prior-call", VerificationStatus::Passed);
        let mut reports = vec![prior.clone()];
        let mut metadata = Some(json!({
            "command": BYTE_CHECK, "verification_author": author,
            "verification_report": current.to_value(), "retained": true
        }));
        record(
            &agent,
            BYTE_CHECK,
            0,
            &mut metadata,
            &mut ledger,
            &mut reports,
        )
        .await;
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0], prior);
        assert!(reports[0].effect_digest.is_none());
        let mut expected = current;
        expected.effect_digest = Some(ledger.digest().to_string());
        assert_eq!(reports[1], expected);
        assert_eq!(metadata.as_ref().unwrap()["verification_author"], author);
        assert_eq!(
            metadata.as_ref().unwrap()["verification_report"],
            expected.to_value()
        );
        assert_eq!(replay_report(&metadata), vec![expected]);
    }
}

#[tokio::test]
async fn prior_call_reports_never_acquire_or_substitute_for_this_call_binding() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = agent(workspace.path(), Vec::new());
    let mut ledger = observed_bytes(workspace.path());
    let current_digest = ledger.digest().to_string();
    let prior_digest = sha256::digest("older effects");
    for binding in [
        None,
        Some(prior_digest.as_str()),
        Some(current_digest.as_str()),
    ] {
        let mut prior = report("prior-call", VerificationStatus::Passed);
        prior.effect_digest = binding.map(str::to_string);
        let mut reports = vec![prior.clone()];
        let mut metadata = Some(json!({"command": BYTE_CHECK}));
        record(
            &agent,
            BYTE_CHECK,
            0,
            &mut metadata,
            &mut ledger,
            &mut reports,
        )
        .await;
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0], prior);
        assert_ne!(reports[1].subject, prior.subject);
        assert_eq!(reports[1].effect_digest.as_deref(), Some(ledger.digest()));
        assert_eq!(replay_report(&metadata), vec![reports[1].clone()]);
    }
}

#[tokio::test]
async fn occupied_failed_or_review_report_cannot_gain_a_second_synthetic_pass() {
    for status in [
        VerificationStatus::Failed,
        VerificationStatus::NeedsReview,
        VerificationStatus::Skipped,
    ] {
        for command in [BYTE_CHECK, SUITE] {
            let workspace = tempfile::tempdir().unwrap();
            let agent = agent(workspace.path(), vec![contract()]);
            let mut ledger = observed_bytes(workspace.path());
            let original = report("this-call-non-pass", status);
            let mut reports = Vec::new();
            let mut metadata = Some(json!({
                "command": command, "verification_author": "host",
                "verification_report": original.to_value()
            }));
            let before = metadata.clone();
            record(&agent, command, 0, &mut metadata, &mut ledger, &mut reports).await;
            assert_eq!(reports, vec![original]);
            assert_eq!(metadata, before);
            assert_eq!(replay_report(&metadata), reports);
            assert!(matches!(
                agent.fact_completion_gate(&ledger, &reports),
                CompletionGate::Incomplete { .. }
            ));
        }
    }
}

#[tokio::test]
async fn editor_and_malformed_slots_remain_unaccepted_without_host_replacements() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = agent(workspace.path(), vec![contract()]);
    let mut ledger = observed_bytes(workspace.path());
    let editor = report("editor-claim", VerificationStatus::Passed).to_value();
    for metadata in [
        json!({"command": BYTE_CHECK, "verification_author": "editor", "verification_report": editor}),
        json!({"command": SUITE, "verification_author": "editor"}),
        json!({"command": BYTE_CHECK, "verification_report": {"schema": "invalid", "checks": false}}),
        json!({"command": SUITE, "verification_report": null}),
        json!("malformed metadata envelope"),
    ] {
        let mut metadata = Some(metadata);
        let before = metadata.clone();
        let prior = report("prior-call", VerificationStatus::Passed);
        let mut reports = vec![prior.clone()];
        record(
            &agent,
            BYTE_CHECK,
            0,
            &mut metadata,
            &mut ledger,
            &mut reports,
        )
        .await;
        assert_eq!(metadata, before);
        assert_eq!(reports, vec![prior]);
        assert!(replay_report(&metadata).is_empty());
    }
}

#[tokio::test]
async fn occupied_suite_report_remains_unbound_under_the_single_slot_contract() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = agent(workspace.path(), vec![contract()]);
    let mut ledger = observed_bytes(workspace.path());
    let original = report("existing-suite-report", VerificationStatus::Passed);
    let mut reports = Vec::new();
    let mut metadata = Some(json!({"command": SUITE, "verification_report": original.to_value()}));
    let before = metadata.clone();
    record(&agent, SUITE, 0, &mut metadata, &mut ledger, &mut reports).await;
    assert_eq!(metadata, before);
    assert_eq!(reports, vec![original]);
    assert!(reports[0].effect_digest.is_none());
    assert!(matches!(
        agent.fact_completion_gate(&ledger, &reports),
        CompletionGate::Incomplete { .. }
    ));
}

#[tokio::test]
async fn a_failed_execution_cannot_bind_an_existing_passed_report() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = agent(workspace.path(), Vec::new());
    let mut ledger = observed_bytes(workspace.path());
    let original = report("unbound-tool-claim", VerificationStatus::Passed);
    let mut reports = Vec::new();
    let mut metadata =
        Some(json!({"command": BYTE_CHECK, "verification_report": original.to_value()}));
    let before = metadata.clone();
    record(
        &agent,
        BYTE_CHECK,
        1,
        &mut metadata,
        &mut ledger,
        &mut reports,
    )
    .await;
    assert_eq!(metadata, before);
    assert_eq!(reports, vec![original]);
    assert!(reports[0].effect_digest.is_none());
    assert!(matches!(
        agent.fact_completion_gate(&ledger, &reports),
        CompletionGate::Incomplete { .. }
    ));
}

#[tokio::test]
async fn ordinary_tool_end_carries_the_same_current_report_without_rebinding_prior() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = agent(workspace.path(), Vec::new());
    let mut state = ExecutionLoopState::new(&[]);
    state.mutations = observed_bytes(workspace.path());
    let prior = report("prior-call", VerificationStatus::Passed);
    state.verification_reports.push(prior.clone());
    let (tx, mut rx) = mpsc::channel(8);
    agent
        .complete_tool_call(
            &mut state,
            ToolCompletionInput {
                tool_call: &call(BYTE_CHECK),
                event_tx: &Some(tx),
                session_id: None,
                tool_start: Instant::now(),
                normalized: NormalizedToolResult {
                    output: "original executed output".into(),
                    exit_code: 0,
                    is_error: false,
                    metadata: Some(json!({"command": BYTE_CHECK})),
                    images: Vec::new(),
                    error_kind: None,
                    trust: crate::llm::ToolResultTrustV1::Trusted,
                    redaction_reviewed: true,
                },
            },
        )
        .await;
    let AgentEvent::ToolEnd { metadata, .. } = rx.try_recv().unwrap() else {
        panic!("expected this call's ToolEnd");
    };
    assert_eq!(state.verification_reports.len(), 2);
    assert_eq!(state.verification_reports[0], prior);
    assert_eq!(
        replay_report(&metadata),
        vec![state.verification_reports[1].clone()]
    );
}
