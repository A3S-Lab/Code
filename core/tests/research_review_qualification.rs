//! End-to-end RESEARCH-REVIEW1 qualification.
//!
//! One reviewer composition runs through the real evaluation substrate: a
//! Code run's evidence is read, a host-owned reviewer executes as an
//! auxiliary run, its result lands in the evaluation result store, and every
//! review finding plus the published batch bind to that exact evaluator
//! record and the admitted research Run. Drift across project, Run,
//! evaluator, evidence, and binding completeness fails closed.

use a3s_code_core::capability::{
    CapabilityCeiling, CapabilityContribution, CapabilityDescriptor, CapabilityExecutionCeiling,
    CapabilityKind, CapabilitySet, CapabilitySource, CodeCatalogGeneration,
    GovernanceCapabilityCeiling, RunCapabilityBindingV1, Sha256Digest, UseCapabilityGeneration,
    UsePackageGeneration, WorkspaceCapabilityCeiling,
};
use a3s_code_core::evaluation::{
    digest_bytes, AuxiliaryCapabilityProfileV1, AuxiliaryExecutor, AuxiliaryRunContextV1,
    AuxiliaryRunError, AuxiliaryRunService, AuxiliaryRunSpecV1, EvaluationRecordV1,
    EvaluationResultSink, EvaluationResultV1, EvidenceReadRequestV1, ExecutionFactRecorder,
    ExecutionFrameV1, ExecutionTargetV1, InMemoryAuxiliaryRunService,
    InMemoryEvaluationResultStore, InMemoryExecutionFactJournal, RunEvidenceReader,
};
use a3s_code_core::{
    AgentEvent, InMemoryRunStore, ResearchArtifactKindV1, ResearchProvenanceReceiptV1,
    ResearchReproducibilityV1, ResearchReviewBatchV1, ResearchReviewCategoryV1,
    ResearchReviewFindingV1, ResearchReviewSeverityV1, ResearchRunStatusV1, ResearchRunV1,
    RunEventRecord,
};
use async_trait::async_trait;
use std::sync::Arc;

fn sha_digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
}

fn use_bound_capability() -> RunCapabilityBindingV1 {
    let use_generation = UseCapabilityGeneration::new(7, sha_digest('a'), sha_digest('b'));
    let package = UsePackageGeneration::new(
        "acme/research-runtime",
        "reviewer",
        "reviewer",
        "1.2.3",
        4,
        sha_digest('c'),
        sha_digest('d'),
    )
    .unwrap();
    let source = CapabilitySource::use_package(use_generation.clone(), package).unwrap();
    let descriptor = CapabilityDescriptor::new(
        &source,
        CapabilityKind::Tool,
        "research-review",
        "research_review",
        sha_digest('e'),
        [],
    )
    .unwrap();
    let contribution = CapabilityContribution::new(source, [descriptor]).unwrap();
    let set = CapabilitySet::from_use_projection(
        CodeCatalogGeneration::new(3),
        use_generation,
        [contribution],
    )
    .unwrap();
    let ceiling = CapabilityCeiling::all(
        &set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        CapabilityExecutionCeiling::new(8, 4, None, None, None).unwrap(),
    )
    .unwrap();
    RunCapabilityBindingV1::from_set_and_ceiling(&set, &ceiling).unwrap()
}

/// Host-owned reviewer policy executed by the evaluation substrate. Core sees
/// only the structured output; this executor stands in for an injected
/// citation/calculation checker.
struct ReviewerExecutor {
    expected_run: String,
}

#[async_trait]
impl AuxiliaryExecutor for ReviewerExecutor {
    async fn execute(
        &self,
        context: AuxiliaryRunContextV1,
    ) -> Result<serde_json::Value, AuxiliaryRunError> {
        if context.spec.parent.target.run_id != self.expected_run {
            return Err(AuxiliaryRunError::TargetMismatch);
        }
        if context.evidence.events.is_empty() {
            return Err(AuxiliaryRunError::EvidenceMismatch);
        }
        Ok(serde_json::json!({
            "findings": [
                {"finding_id": "finding-1", "category": "reproducibility"},
                {"finding_id": "finding-2", "category": "citation"}
            ]
        }))
    }
}

struct Fixture {
    target: ExecutionTargetV1,
    evidence_digest: String,
    record: EvaluationRecordV1,
    store: InMemoryEvaluationResultStore,
    run: ResearchRunV1,
    receipt: ResearchProvenanceReceiptV1,
}

async fn reviewed_run() -> Fixture {
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id("run-review-1".to_string(), "session-review", "prompt")
        .await;
    let record_event = runs
        .record_event(
            &run.id,
            AgentEvent::TextDelta {
                text: "analysis output".to_string(),
            },
        )
        .await
        .unwrap();
    let target = ExecutionTargetV1::new("session-review", &run.id);
    let journal = Arc::new(InMemoryExecutionFactJournal::new());
    ExecutionFactRecorder::new(journal.clone(), ExecutionFrameV1::root(target.clone()))
        .record(&RunEventRecord {
            sequence: 0,
            timestamp_ms: record_event.updated_at_ms,
            event: AgentEvent::TextDelta {
                text: "analysis output".to_string(),
            },
        })
        .unwrap();

    let evidence = RunEvidenceReader::new(Arc::clone(&runs))
        .with_facts(journal)
        .read(EvidenceReadRequestV1::new(target.clone()))
        .await
        .unwrap();
    let evidence_digest = evidence.snapshot_digest.clone();

    let service = InMemoryAuxiliaryRunService::new(Arc::new(ReviewerExecutor {
        expected_run: run.id.clone(),
    }));
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(target.clone()),
        "research-review",
        "review the bounded evidence for citations and reproducibility",
        evidence_digest.clone(),
    )
    .with_id("aux-review-1")
    .with_capabilities(AuxiliaryCapabilityProfileV1::tool_free());
    let output = service
        .spawn(spec, evidence, None)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();

    let result = EvaluationResultV1::new(
        "research-reviewer",
        target.clone(),
        "aux-review-1",
        "needs_review",
        output.value,
        evidence_digest.clone(),
    )
    .unwrap();
    let record = EvaluationRecordV1::new(result, 64).unwrap();
    let store = InMemoryEvaluationResultStore::new();
    assert!(store.write(record.clone()).await.unwrap().written);

    let mut research_run = ResearchRunV1::new(
        "run-review-1",
        "project-review",
        3,
        format!("sha256:{}", "f".repeat(64)),
        evidence_digest.clone(),
        use_bound_capability(),
        "provider-a",
        "model-a",
        ResearchReproducibilityV1::Reproducible,
        Some(11),
    )
    .unwrap();
    research_run
        .transition_to(ResearchRunStatusV1::Admitted)
        .unwrap();

    let receipt = ResearchProvenanceReceiptV1::new(
        "project-review",
        3,
        "run-review-1",
        "artifact-report",
        ResearchArtifactKindV1::Report,
        format!("sha256:{}", "1".repeat(64)),
        vec![evidence_digest.clone()],
        format!("sha256:{}", "2".repeat(64)),
        format!("sha256:{}", "3".repeat(64)),
        format!("sha256:{}", "4".repeat(64)),
        "provider-a",
        Some(format!("sha256:{}", "5".repeat(64))),
        Some(11),
        None,
    )
    .unwrap();

    Fixture {
        target,
        evidence_digest,
        record,
        store,
        run: research_run,
        receipt,
    }
}

fn bound_finding(
    fixture: &Fixture,
    finding_id: &str,
    evaluator_id: &str,
) -> ResearchReviewFindingV1 {
    ResearchReviewFindingV1::new(
        finding_id,
        &fixture.run.project_id,
        &fixture.run.run_id,
        fixture.receipt.artifact_digest.clone(),
        ResearchReviewCategoryV1::Reproducibility,
        ResearchReviewSeverityV1::Warning,
        "The claim needs an explicit environment receipt.",
        None,
        vec![fixture.evidence_digest.clone()],
        evaluator_id,
        42,
    )
    .unwrap()
    .bind_evaluation_record_for_run(&fixture.record, &fixture.run)
    .unwrap()
    .bind_provenance_receipt_for_run(&fixture.receipt, &fixture.run)
    .unwrap()
}

#[tokio::test]
async fn reviewer_composition_publishes_a_run_aware_batch_with_terminal_decisions() {
    let fixture = reviewed_run().await;
    let findings = vec![
        bound_finding(&fixture, "finding-1", "research-reviewer"),
        bound_finding(&fixture, "finding-2", "research-reviewer"),
    ];
    let mut batch = ResearchReviewBatchV1::new_for_run(
        "batch-1",
        &fixture.run,
        &fixture.record,
        fixture.evidence_digest.clone(),
        findings,
    )
    .unwrap();
    batch
        .validate_for_run(&fixture.run, &fixture.record)
        .unwrap();

    // The result store retains the evaluator record the batch is bound to.
    assert_eq!(
        fixture.store.list_for_target(&fixture.target).await,
        vec![fixture.record.clone()]
    );

    // Wire round trip stays strict.
    let encoded = batch.to_vec().unwrap();
    let decoded = ResearchReviewBatchV1::from_slice(&encoded).unwrap();
    assert_eq!(decoded, batch);

    // Human decisions are explicit and terminal.
    batch
        .resolve_finding("finding-1", format!("sha256:{}", "6".repeat(64)))
        .unwrap();
    assert!(batch
        .resolve_finding("finding-1", format!("sha256:{}", "7".repeat(64)))
        .is_err());
    batch
        .waive_finding("finding-2", format!("sha256:{}", "8".repeat(64)))
        .unwrap();
    assert!(batch
        .validate_for_run(&fixture.run, &fixture.record)
        .is_ok());
    let closed = ResearchReviewBatchV1::from_slice(&batch.to_vec().unwrap()).unwrap();
    assert!(closed.findings.iter().all(|finding| finding
        .resolution_digest
        .as_deref()
        .is_some_and(|digest| digest.starts_with("sha256:"))));
}

#[tokio::test]
async fn evaluator_run_and_project_drift_fail_closed() {
    let fixture = reviewed_run().await;

    // A finding attributed to a different evaluator cannot bind the record,
    // so it can never enter a batch published for that record.
    let foreign = ResearchReviewFindingV1::new(
        "finding-foreign",
        &fixture.run.project_id,
        &fixture.run.run_id,
        fixture.receipt.artifact_digest.clone(),
        ResearchReviewCategoryV1::Reproducibility,
        ResearchReviewSeverityV1::Warning,
        "attributed to the wrong evaluator",
        None,
        vec![fixture.evidence_digest.clone()],
        "other-evaluator",
        42,
    )
    .unwrap();
    assert!(foreign
        .bind_evaluation_record_for_run(&fixture.record, &fixture.run)
        .is_err());

    // A different project revision of the same run identity is rejected.
    let mut other_revision = fixture.run.clone();
    other_revision.project_revision = 4;
    assert!(bound_finding(&fixture, "finding-1", "research-reviewer")
        .bind_provenance_receipt_for_run(&fixture.receipt, &other_revision)
        .is_err());

    // A different admitted run cannot adopt the evaluator record.
    let mut other_run = ResearchRunV1::new(
        "run-review-2",
        "project-review",
        3,
        format!("sha256:{}", "f".repeat(64)),
        fixture.evidence_digest.clone(),
        use_bound_capability(),
        "provider-a",
        "model-a",
        ResearchReproducibilityV1::Reproducible,
        Some(11),
    )
    .unwrap();
    other_run
        .transition_to(ResearchRunStatusV1::Admitted)
        .unwrap();
    let bound = bound_finding(&fixture, "finding-1", "research-reviewer");
    let smuggled = ResearchReviewBatchV1::new_for_run(
        "batch-2",
        &fixture.run,
        &fixture.record,
        fixture.evidence_digest.clone(),
        vec![bound],
    )
    .unwrap();
    assert!(matches!(
        smuggled.validate_for_run(&other_run, &fixture.record),
        Err(a3s_code_core::research::ResearchContractError::InvalidField("researchRun.runId"))
    ));

    // An evaluator record for different evidence cannot publish for this run.
    let mut mismatched = fixture.record.clone();
    mismatched.result.evidence_digest = digest_bytes("other", b"evidence");
    assert!(ResearchReviewBatchV1::new_for_run(
        "batch-3",
        &fixture.run,
        &mismatched,
        fixture.evidence_digest.clone(),
        Vec::new(),
    )
    .is_err());

    // An unbound finding cannot ride along in a published batch.
    let unbound = ResearchReviewFindingV1::new(
        "finding-unbound",
        &fixture.run.project_id,
        &fixture.run.run_id,
        fixture.receipt.artifact_digest.clone(),
        ResearchReviewCategoryV1::Citation,
        ResearchReviewSeverityV1::Info,
        "not bound to the evaluator record",
        None,
        vec![fixture.evidence_digest.clone()],
        "research-reviewer",
        42,
    )
    .unwrap();
    assert!(ResearchReviewBatchV1::new_for_run(
        "batch-4",
        &fixture.run,
        &fixture.record,
        fixture.evidence_digest.clone(),
        vec![unbound],
    )
    .is_err());
}
