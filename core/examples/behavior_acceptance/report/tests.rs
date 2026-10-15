#![cfg(test)]

use super::*;
use a3s_code_core::content_digest::digest_bytes;

fn hash(value: &str) -> DigestV1 {
    DigestV1::new(digest_bytes(
        "test.behavior-acceptance.v1",
        value.as_bytes(),
    ))
    .unwrap()
}
fn attempt() -> AttemptIdentityV1 {
    AttemptIdentityV1 {
        suite_attempt_id: "suite 甲".into(),
        logical_attempt_id: "first".into(),
        role: AttemptRoleV1::First,
        predecessor_id: None,
    }
}
fn request() -> AcceptanceRequestV1 {
    AcceptanceRequestV1 {
        schema: REQUEST_SCHEMA.into(),
        manifest_digest: hash("manifest"),
        attempt: attempt(),
        maker_target: ExecutionTargetV1::new("session", "run"),
        maker_completion: CompletionTerminal::Verified {
            effect_digest: hash("effects").as_str().into(),
        },
        effect_digest: hash("effects"),
        candidate_byte_digest: hash("candidate"),
        verifier_digest: hash("verifier"),
        environment_digest: hash("environment"),
        evidence_digest: hash("evidence"),
        original_receipt_capture_digest: hash("receipts"),
        settlement_evidence_digest: hash("settlement"),
    }
}
fn payload() -> AcceptancePayloadV1 {
    let request = request();
    AcceptancePayloadV1 {
        schema: PAYLOAD_SCHEMA.into(),
        request_identity: request.identity().unwrap(),
        task_id: "task 甲".into(),
        cluster_id: "cluster".into(),
        variant: "fixed".into(),
        manifest_digest: request.manifest_digest,
        maker_target: request.maker_target,
        maker_completion: request.maker_completion,
        effect_digest: request.effect_digest,
        candidate_byte_digest: request.candidate_byte_digest,
        verifier_digest: request.verifier_digest,
        environment_digest: request.environment_digest,
        evidence_digest: request.evidence_digest,
        original_receipt_capture_digest: request.original_receipt_capture_digest,
        settlement_evidence_digest: request.settlement_evidence_digest,
        artifact_behavior: ArtifactBehaviorV1::Incomplete,
        delivery_accepted: false,
        reasons: vec![
            AcceptanceReasonV1::EvidenceIncomplete,
            AcceptanceReasonV1::CostUnknown,
        ],
        expected_case_count: 16,
        executed_case_count: 0,
        case_receipt_digests: Vec::new(),
        verification_report_digest: hash("report"),
        resource_settlement: ResourceSettlementV1::Unknown,
        waiver_present: false,
        attempt: request.attempt,
        observed_at_ms: 1,
    }
}

#[test]
fn every_host_closed_set_round_trips_and_names_unknown_refusals() {
    macro_rules! check {
        ($kind:ident) => {
            for value in $kind::ALL {
                assert_eq!($kind::parse(value.as_str()).unwrap(), *value);
                let wire = serde_json::to_string(value).unwrap();
                assert_eq!(serde_json::from_str::<$kind>(&wire).unwrap(), *value);
            }
            assert!($kind::parse("not_a_member")
                .unwrap_err()
                .to_string()
                .contains(stringify!($kind)));
        };
    }
    check!(ArtifactBehaviorV1);
    check!(AcceptanceReasonV1);
    check!(ResourceSettlementV1);
    check!(NativeQualificationStatusV1);
    check!(ResourceLimitStatusV1);
    check!(AttemptRoleV1);
    check!(FailureOriginV1);
    check!(BillingStatusV1);
    check!(UsageUncertaintyPolicyV1);
}

#[test]
fn request_identity_binds_all_declared_original_and_frozen_inputs() {
    let original = request();
    let identity = original.identity().unwrap();
    for field in [
        "manifest_digest",
        "candidate_byte_digest",
        "verifier_digest",
        "environment_digest",
        "evidence_digest",
        "original_receipt_capture_digest",
        "settlement_evidence_digest",
    ] {
        let mut wire = serde_json::to_value(&original).unwrap();
        wire[field] = serde_json::to_value(hash("changed")).unwrap();
        let changed: AcceptanceRequestV1 = serde_json::from_value(wire).unwrap();
        assert_ne!(changed.identity().unwrap(), identity, "{field}");
    }
    let mut changed = original.clone();
    changed.maker_target.run_id = "another-run".into();
    assert_ne!(changed.identity().unwrap(), identity);
    let mut changed = original.clone();
    changed.attempt.suite_attempt_id = "another-suite".into();
    assert_ne!(changed.identity().unwrap(), identity);
    let mut changed = original.clone();
    changed.attempt.logical_attempt_id = "another-first".into();
    assert_ne!(changed.identity().unwrap(), identity);
    let mut changed = original.clone();
    changed.maker_completion = CompletionTerminal::Narrative;
    assert_ne!(changed.identity().unwrap(), identity);
    let mut changed = original.clone();
    changed.effect_digest = hash("changed");
    assert!(changed.identity().is_err());
    changed.maker_completion = CompletionTerminal::Verified {
        effect_digest: changed.effect_digest.as_str().into(),
    };
    assert_ne!(changed.identity().unwrap(), identity);
    assert_eq!(
        original.auxiliary_id().unwrap(),
        original.auxiliary_id().unwrap()
    );
}

#[test]
fn report_rejects_unknown_duplicate_and_unbound_data_without_exposing_expectations() {
    let original = payload();
    original.validate().unwrap();
    let wire = serde_json::to_value(&original).unwrap();
    assert!(wire.get("expected_output").is_none());
    let mut changed = wire.clone();
    changed["unknown"] = true.into();
    assert!(serde_json::from_value::<AcceptancePayloadV1>(changed).is_err());
    let mut changed = wire.clone();
    changed["maker_completion"]["unknown"] = true.into();
    assert!(serde_json::from_value::<AcceptancePayloadV1>(changed).is_err());
    let mut changed = wire.clone();
    changed["maker_completion"] = serde_json::json!({
        "kind": "narrative", "effect_digest": null
    });
    assert!(serde_json::from_value::<AcceptancePayloadV1>(changed).is_err());
    let mut changed = wire.clone();
    changed["reasons"] = serde_json::json!(["unknown"]);
    assert!(serde_json::from_value::<AcceptancePayloadV1>(changed).is_err());
    let mut changed = original.clone();
    changed.reasons.push(AcceptanceReasonV1::EvidenceIncomplete);
    assert!(changed.validate().is_err());
    let mut changed = original.clone();
    changed.evidence_digest = hash("different original evidence");
    assert!(changed.validate().is_err());
    let mut changed = wire;
    changed["verifier_digest"] = "unpinned".into();
    assert!(serde_json::from_value::<AcceptancePayloadV1>(changed).is_err());
}

#[test]
fn case_receipt_identity_binds_observed_bytes_and_requires_a_settled_observation() {
    let request = request();
    let mut receipt = AcceptanceCaseReceiptV1 {
        case_id: " exact 甲\\case ".into(),
        ordinal: 0,
        input_digest: hash("input"),
        maker_target: request.maker_target.clone(),
        request_identity: request.identity().unwrap(),
        candidate_byte_digest: request.candidate_byte_digest,
        verifier_digest: request.verifier_digest,
        environment_digest: request.environment_digest,
        response_digest: hash("observed stdout"),
        response_byte_count: 15,
        stderr_digest: hash("observed stderr"),
        stderr_byte_count: 0,
        exit_code: Some(0),
        timed_out: false,
        output_complete: true,
        started_at_ms: 10,
        finished_at_ms: 11,
        resource_settlement: ResourceSettlementV1::Settled,
        outcome: ArtifactBehaviorV1::Accepted,
        reason: AcceptanceReasonV1::AllCasesPassed,
    };
    let identity = receipt.digest().unwrap();
    receipt.response_digest = hash("different observed bytes");
    assert_ne!(receipt.digest().unwrap(), identity);
    receipt.resource_settlement = ResourceSettlementV1::Unknown;
    assert!(receipt.validate().is_err());
    receipt.outcome = ArtifactBehaviorV1::Incomplete;
    receipt.reason = AcceptanceReasonV1::ResourcesUnsettled;
    receipt.validate().unwrap();
    receipt.ordinal = CASE_COUNT as u64;
    assert!(receipt.validate().is_err());
}

#[test]
fn positive_case_coverage_and_waived_maker_cannot_be_inferred_from_report_text() {
    let mut report = payload();
    report.artifact_behavior = ArtifactBehaviorV1::Accepted;
    report.reasons = vec![
        AcceptanceReasonV1::AllCasesPassed,
        AcceptanceReasonV1::CostUnknown,
    ];
    assert!(report.validate().is_err());
    report.executed_case_count = 16;
    report.case_receipt_digests = (0..16)
        .map(|index| hash(&format!("receipt {index}")))
        .collect();
    report.resource_settlement = ResourceSettlementV1::Settled;
    report.validate().unwrap(); // Diagnostic behavior pass is distinct from delivery.
    report.delivery_accepted = true;
    report.waiver_present = true;
    assert!(report.validate().is_err());
    report.waiver_present = false;
    report.maker_completion = CompletionTerminal::Waived {
        effect_digest: report.effect_digest.as_str().into(),
    };
    report.request_identity = report.request().identity().unwrap();
    assert!(report.validate().is_err());
}

#[test]
fn generic_evaluation_carries_the_host_payload_without_a_core_business_enum() {
    let report = payload();
    report.validate().unwrap();
    let result = a3s_code_core::evaluation::EvaluationResultV1::new(
        "a3s.code.behavior-acceptance.v1",
        report.maker_target.clone(),
        report.request().auxiliary_id().unwrap(),
        report.artifact_behavior.as_str(),
        serde_json::to_value(&report).unwrap(),
        report.evidence_digest.as_str(),
    )
    .unwrap();
    result.validate().unwrap();
    let reopened: AcceptancePayloadV1 = serde_json::from_value(result.payload).unwrap();
    reopened.validate().unwrap();
    assert_eq!(reopened.digest().unwrap(), report.digest().unwrap());
}

#[test]
fn attempt_and_cost_unknowns_are_preserved_without_zero_defaults() {
    let mut accounting = AttemptAccountingV1 {
        attempt: attempt(),
        failure_origin: FailureOriginV1::HostInjectedCancel,
        provider_model_route: "declared/model".into(),
        provider_revision: None,
        model_revision: None,
        provider_invocation_ids: vec!["observed call".into()],
        reported_usage: None,
        estimated_tokens: None,
        estimated_money_microusd: None,
        reserved_tokens: None,
        reserved_money_microusd: None,
        known_retry_count: None,
        known_fallback_count: None,
        billing_status: BillingStatusV1::Unknown,
        cost_complete: false,
        uncertainty_reasons: vec![AcceptanceReasonV1::CostUnknown],
    };
    accounting.validate().unwrap();
    let wire = serde_json::to_value(&accounting).unwrap();
    assert!(wire["reported_usage"].is_null());
    assert!(wire["reserved_money_microusd"].is_null());
    let mut unknown_usage = wire;
    unknown_usage["reported_usage"] = serde_json::json!({
        "prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3,
        "cache_read_tokens": null, "cache_write_tokens": null, "unknown": true
    });
    assert!(serde_json::from_value::<AttemptAccountingV1>(unknown_usage).is_err());
    accounting.cost_complete = true;
    assert!(accounting.validate().is_err());
    accounting.billing_status = BillingStatusV1::Reported;
    accounting.reported_usage = Some(Default::default());
    accounting.known_retry_count = Some(0);
    accounting.known_fallback_count = Some(0);
    accounting.uncertainty_reasons.clear();
    assert!(accounting.validate().is_err());
    let mut remediation = attempt();
    remediation.role = AttemptRoleV1::Remediation;
    assert!(remediation.validate().is_err());
    remediation.predecessor_id = Some("first-retained".into());
    remediation.validate().unwrap();
}
