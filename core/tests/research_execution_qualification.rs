use a3s_code_core::capability::{
    CapabilityCeiling, CapabilityContribution, CapabilityDescriptor, CapabilityExecutionCeiling,
    CapabilityKind, CapabilitySet, CapabilitySource, CodeCatalogGeneration,
    GovernanceCapabilityCeiling, RunCapabilityBindingV1, Sha256Digest, UseCapabilityGeneration,
    UsePackageGeneration, WorkspaceCapabilityCeiling,
};
use a3s_code_core::evaluation::{
    digest_bytes, EvaluationDispatchClaimOutcome, EvaluationDispatchLedger, EvaluationRecordV1,
    EvaluationResultSink, EvaluationResultV1, EvidenceError, EvidenceReadRequestV1,
    ExecutionFactRecorder, ExecutionFrameV1, ExecutionIdentityV1, ExecutionResultOutcomeV1,
    ExecutionResultReceiptV1, ExecutionTargetV1, FileEvaluationDispatchLedger,
    FileEvaluationResultStore, InMemoryExecutionFactJournal, RunEvidenceReader,
};
use a3s_code_core::tools::{
    ImmutableContentAdapter, ImmutableContentAdapterBindingV1, ImmutableContentAdapterSession,
    ImmutableContentError, ImmutableContentKindV1, ImmutableContentReferenceV1,
    ImmutableContentResult, ImmutableContentWriteRequestV1, TOOL_RESULT_CONTENT_MEDIA_TYPE,
};
use a3s_code_core::{
    AgentEvent, CoreEventIdentity, CoreIdentity, EvidenceCursor, InMemoryRunStore, OperationId,
    ResearchArtifactKindV1, ResearchEventV1, ResearchEvidenceFactKindV1, ResearchEvidenceFactV1,
    ResearchProvenanceReceiptV1, ResearchReproducibilityV1, ResearchReviewBatchV1,
    ResearchReviewCategoryV1, ResearchReviewFindingV1, ResearchReviewSeverityV1,
    ResearchReviewStatusV1, ResearchRunStatusV1, ResearchRunV1, SourceRevision,
};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

use a3s_code_core::execution_identity::EVALUATION_DISPATCH_IDENTITY_DOMAIN_V1;

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn sha_digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(digest(byte)).unwrap()
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

#[derive(Clone, Debug)]
struct FileCreateOnlyAdapter {
    root: PathBuf,
}

impl FileCreateOnlyAdapter {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    fn path_for(&self, digest: &str) -> PathBuf {
        self.root
            .join(digest.strip_prefix("sha256:").unwrap_or(digest))
    }
}

#[async_trait]
impl ImmutableContentAdapter for FileCreateOnlyAdapter {
    fn name(&self) -> &str {
        "qualification-file-content"
    }

    async fn put(
        &self,
        request: &ImmutableContentWriteRequestV1<'_>,
    ) -> ImmutableContentResult<ImmutableContentReferenceV1> {
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|_| ImmutableContentError::Provider("content directory unavailable".into()))?;
        let path = self.path_for(&request.descriptor().content_digest);
        match tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                file.write_all(request.content())
                    .await
                    .map_err(|_| ImmutableContentError::Provider("content write failed".into()))?;
                file.sync_data()
                    .await
                    .map_err(|_| ImmutableContentError::Provider("content sync failed".into()))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = tokio::fs::read(&path).await.map_err(|_| {
                    ImmutableContentError::Provider("content replay read failed".into())
                })?;
                if existing != request.content() {
                    return Err(ImmutableContentError::Provider(
                        "content address collision".into(),
                    ));
                }
            }
            Err(_) => {
                return Err(ImmutableContentError::Provider(
                    "content create failed".into(),
                ));
            }
        }
        ImmutableContentReferenceV1::new(
            request.binding(),
            request.descriptor(),
            format!(
                "a3s+qualification://artifact/{}",
                request
                    .descriptor()
                    .content_digest
                    .trim_start_matches("sha256:")
            ),
        )
    }
}

#[tokio::test]
async fn research_execution_restarts_with_contiguous_evidence_and_exact_bindings() {
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .reserve_run_with_id(
            "research-run-1".to_owned(),
            "research-session",
            "private research prompt",
        )
        .await;
    let run = run.snapshot().clone();
    let capability_binding = use_bound_capability();
    let admitted = runs
        .bind_capability_generation(&run.id, capability_binding.clone())
        .await
        .unwrap();
    assert_eq!(
        admitted.capability_binding,
        Some(capability_binding.clone())
    );

    let target = ExecutionTargetV1::new("research-session", &run.id);
    let journal = Arc::new(InMemoryExecutionFactJournal::new());
    let event = AgentEvent::ToolEnd {
        id: "source-capture".to_owned(),
        name: "research_review".to_owned(),
        args: Some(serde_json::json!({"source": "paper-1"})),
        output: "captured source metadata".to_owned(),
        exit_code: 0,
        metadata: None,
        error_kind: None,
    };
    runs.record_event(&run.id, event).await.unwrap();
    let records = runs.events(&run.id).await;
    let recorder =
        ExecutionFactRecorder::new(journal.clone(), ExecutionFrameV1::root(target.clone()));
    recorder.record(&records[0]).unwrap();

    let evidence = RunEvidenceReader::new(Arc::clone(&runs))
        .with_facts(journal)
        .read(EvidenceReadRequestV1::new(target.clone()))
        .await
        .unwrap();
    assert!(evidence.complete);
    assert!(evidence.validate().is_ok());
    assert_eq!(evidence.events.len(), 1);
    assert_eq!(evidence.facts[0].sequence, 0);

    let mut missing_fact = evidence.clone();
    missing_fact.facts.clear();
    assert!(matches!(
        missing_fact.validate(),
        Err(EvidenceError::InvalidField("facts.events"))
    ));
    let mut gapped_fact = evidence.clone();
    let mut shifted = records[0].clone();
    shifted.sequence = 2;
    gapped_fact.facts[0] = a3s_code_core::evaluation::ExecutionFactV1::from_run_event(
        ExecutionFrameV1::root(target.clone()),
        &shifted,
    )
    .unwrap();
    assert!(matches!(
        gapped_fact.validate(),
        Err(EvidenceError::InvalidField("facts.sequence"))
    ));

    let mut research = ResearchRunV1::new(
        &run.id,
        "research-project",
        9,
        digest_bytes("research-source", b"source-snapshot"),
        evidence.snapshot_digest.clone(),
        capability_binding.clone(),
        "fixture-provider",
        "fixture-model",
        ResearchReproducibilityV1::Deterministic,
        Some(41),
    )
    .unwrap();
    research
        .validate_execution_target(&target)
        .expect("exact Run identity must be admitted");
    research
        .transition_to(ResearchRunStatusV1::Admitted)
        .unwrap();
    research
        .transition_to(ResearchRunStatusV1::Running)
        .unwrap();
    research
        .transition_to(ResearchRunStatusV1::Checkpointed)
        .unwrap();

    let restarted: ResearchRunV1 =
        serde_json::from_slice(&serde_json::to_vec(&research).unwrap()).unwrap();
    assert_eq!(restarted, research);
    assert!(restarted.validate().is_ok());
    research = restarted;
    research
        .transition_to(ResearchRunStatusV1::Running)
        .unwrap();
    research
        .transition_to(ResearchRunStatusV1::Completed)
        .unwrap();
    assert!(matches!(
        research.transition_to(ResearchRunStatusV1::Running),
        Err(a3s_code_core::ResearchContractError::InvalidTransition { .. })
    ));

    let core_event = CoreEventIdentity::from_agent_event(
        CoreIdentity::new(
            OperationId::new("research-session/research-run-1/turn-1").unwrap(),
            SourceRevision::new(9),
            None,
            EvidenceCursor::new(0),
        ),
        records[0].timestamp_ms,
        &records[0].event,
    )
    .unwrap();
    let projected =
        ResearchEventV1::from_core_event_for_run("research-project", 9, &run.id, &core_event)
            .unwrap();
    assert_eq!(projected.run_id.as_deref(), Some(run.id.as_str()));
    assert_ne!(
        projected.run_id.as_deref(),
        Some(core_event.identity.operation_id.as_str())
    );
    assert_eq!(projected.sequence, 1);
    assert!(projected.validate().is_ok());

    let mut metadata = BTreeMap::new();
    metadata.insert("source".to_owned(), "paper-1".to_owned());
    let fact = ResearchEvidenceFactV1::new(
        &run.id,
        1,
        ResearchEvidenceFactKindV1::Source,
        "paper-1",
        Some(digest_bytes("research-source", b"source-snapshot")),
        digest_bytes("source-metadata", b"paper-1"),
        metadata,
        records[0].timestamp_ms,
    )
    .unwrap();
    assert!(fact.validate().is_ok());

    let artifact_dir = tempfile::tempdir().unwrap();
    let binding = ImmutableContentAdapterBindingV1::new(digest('f'), 4096).unwrap();
    let adapter = Arc::new(FileCreateOnlyAdapter::new(artifact_dir.path()));
    let content_session = ImmutableContentAdapterSession::new(binding.clone(), adapter).unwrap();
    let artifact = b"immutable figure bytes";
    let reference = content_session
        .put(
            ImmutableContentKindV1::ToolResultOriginal,
            TOOL_RESULT_CONTENT_MEDIA_TYPE,
            artifact,
        )
        .await
        .unwrap();
    let replay = content_session
        .put(
            ImmutableContentKindV1::ToolResultOriginal,
            TOOL_RESULT_CONTENT_MEDIA_TYPE,
            artifact,
        )
        .await
        .unwrap();
    assert_eq!(reference, replay);
    assert_eq!(std::fs::read_dir(artifact_dir.path()).unwrap().count(), 1);
    let receipt = ResearchProvenanceReceiptV1::new(
        "research-project",
        9,
        &run.id,
        "figure-1",
        ResearchArtifactKindV1::Figure,
        reference.content_digest.clone(),
        vec![evidence.snapshot_digest.clone()],
        digest_bytes("research-workflow", b"workflow-v1"),
        digest_bytes("research-code", b"code-v1"),
        digest_bytes("research-environment", b"env-v1"),
        "fixture-provider",
        Some(digest_bytes("research-model", b"fixture-model")),
        Some(41),
        Some(digest_bytes("research-validation", b"validated")),
    )
    .unwrap();
    let reopened_receipt: ResearchProvenanceReceiptV1 =
        serde_json::from_slice(&serde_json::to_vec(&receipt).unwrap()).unwrap();
    assert!(reopened_receipt.validate().is_ok());
    assert_eq!(reopened_receipt, receipt);

    let identity = ExecutionIdentityV1::derive(
        EVALUATION_DISPATCH_IDENTITY_DOMAIN_V1,
        &serde_json::json!({
            "run_id": run.id,
            "evidence_digest": evidence.snapshot_digest,
            "evaluator": "research-reviewer"
        }),
    )
    .unwrap();
    let request_digest = digest_bytes("research-review-request", b"review-v1");
    let dispatch_dir = tempfile::tempdir().unwrap();
    let first_ledger =
        FileEvaluationDispatchLedger::with_max_records(dispatch_dir.path(), 8).unwrap();
    assert_eq!(
        first_ledger
            .claim_with_identity(
                "research-review-1",
                &request_digest,
                &identity,
                "worker-a",
                1_000,
                60_000,
            )
            .await
            .unwrap(),
        EvaluationDispatchClaimOutcome::Claimed { attempt: 1 }
    );
    drop(first_ledger);
    let restarted_ledger =
        FileEvaluationDispatchLedger::with_max_records(dispatch_dir.path(), 8).unwrap();
    assert!(matches!(
        restarted_ledger
            .claim_with_identity(
                "research-review-1",
                &request_digest,
                &identity,
                "worker-b",
                2_000,
                60_000,
            )
            .await
            .unwrap(),
        EvaluationDispatchClaimOutcome::Busy { .. }
    ));
    assert_eq!(
        restarted_ledger
            .claim_with_identity(
                "research-review-1",
                &request_digest,
                &identity,
                "worker-b",
                61_001,
                60_000,
            )
            .await
            .unwrap(),
        EvaluationDispatchClaimOutcome::Claimed { attempt: 2 }
    );
    let terminal_receipt = ExecutionResultReceiptV1::new(
        identity.clone(),
        evidence.snapshot_digest.clone(),
        ExecutionResultOutcomeV1::Succeeded,
        Some(digest_bytes("research-review-result", b"review-result")),
        13,
    )
    .unwrap();
    restarted_ledger
        .complete_with_receipt(
            "research-review-1",
            &request_digest,
            &identity,
            "worker-b",
            &terminal_receipt,
            61_002,
        )
        .await
        .unwrap();
    assert_eq!(
        restarted_ledger
            .completed_receipt("research-review-1")
            .await
            .unwrap(),
        Some(terminal_receipt)
    );

    let result = EvaluationResultV1::new(
        "research-reviewer",
        target.clone(),
        "aux-research-review-1",
        "needs_review",
        serde_json::json!({"finding_count": 1}),
        evidence.snapshot_digest.clone(),
    )
    .unwrap();
    let record = EvaluationRecordV1::new(result, 61_003).unwrap();
    let result_dir = tempfile::tempdir().unwrap();
    let result_store = FileEvaluationResultStore::with_max_records(result_dir.path(), 8).unwrap();
    assert!(result_store.write(record.clone()).await.unwrap().written);
    drop(result_store);
    let reopened_store = FileEvaluationResultStore::with_max_records(result_dir.path(), 8).unwrap();
    let reopened_record = reopened_store
        .get_checked(&record.record_digest)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reopened_record, record);

    let finding = ResearchReviewFindingV1::new(
        "finding-1",
        "research-project",
        &run.id,
        reference.content_digest.clone(),
        ResearchReviewCategoryV1::Reproducibility,
        ResearchReviewSeverityV1::Warning,
        "The figure needs an explicit environment receipt.",
        None,
        vec![evidence.snapshot_digest.clone()],
        "research-reviewer",
        61_004,
    )
    .unwrap()
    .bind_provenance_receipt_for_run(&reopened_receipt, &research)
    .unwrap()
    .bind_evaluation_record_for_run(&reopened_record, &research)
    .unwrap();

    let cross_project_finding = ResearchReviewFindingV1::new(
        "finding-cross-project",
        "other-research-project",
        &run.id,
        reference.content_digest.clone(),
        ResearchReviewCategoryV1::Reproducibility,
        ResearchReviewSeverityV1::Warning,
        "The evaluator result must remain in its project namespace.",
        None,
        vec![evidence.snapshot_digest.clone()],
        "research-reviewer",
        61_005,
    )
    .unwrap();
    assert_eq!(
        cross_project_finding.bind_evaluation_record_for_run(&reopened_record, &research),
        Err(a3s_code_core::ResearchContractError::InvalidField(
            "researchRun.projectId"
        ))
    );

    let drifted_evidence_digest = digest_bytes("research-review-evidence", b"drifted");
    let drifted_record = EvaluationRecordV1::new(
        EvaluationResultV1::new(
            "research-reviewer",
            target.clone(),
            "aux-research-review-1",
            "needs_review",
            serde_json::json!({"finding_count": 1}),
            drifted_evidence_digest.clone(),
        )
        .unwrap(),
        61_005,
    )
    .unwrap();
    let drifted_evidence_finding = ResearchReviewFindingV1::new(
        "finding-drifted-evidence",
        "research-project",
        &run.id,
        reference.content_digest.clone(),
        ResearchReviewCategoryV1::Reproducibility,
        ResearchReviewSeverityV1::Warning,
        "The evaluator must review the admitted Run evidence snapshot.",
        None,
        vec![drifted_evidence_digest],
        "research-reviewer",
        61_006,
    )
    .unwrap();
    assert_eq!(
        drifted_evidence_finding.bind_evaluation_record_for_run(&drifted_record, &research),
        Err(a3s_code_core::ResearchContractError::InvalidField(
            "evaluationRecord.evidenceDigest"
        ))
    );

    let drifted_receipt = ResearchProvenanceReceiptV1::new(
        "research-project",
        8,
        &run.id,
        "figure-1",
        ResearchArtifactKindV1::Figure,
        reference.content_digest.clone(),
        vec![evidence.snapshot_digest.clone()],
        digest_bytes("research-workflow", b"workflow-v1"),
        digest_bytes("research-code", b"code-v1"),
        digest_bytes("research-environment", b"env-v1"),
        "fixture-provider",
        Some(digest_bytes("research-model", b"fixture-model")),
        Some(41),
        Some(digest_bytes("research-validation", b"validated")),
    )
    .unwrap();
    let drifted_finding = ResearchReviewFindingV1::new(
        "finding-drifted-revision",
        "research-project",
        &run.id,
        reference.content_digest.clone(),
        ResearchReviewCategoryV1::Reproducibility,
        ResearchReviewSeverityV1::Warning,
        "The provenance revision must match the admitted Run.",
        None,
        vec![evidence.snapshot_digest.clone()],
        "research-reviewer",
        61_007,
    )
    .unwrap();
    assert_eq!(
        drifted_finding.bind_provenance_receipt_for_run(&drifted_receipt, &research),
        Err(a3s_code_core::ResearchContractError::InvalidField(
            "provenanceReceipt.projectRevision"
        ))
    );

    let provider_drift_receipt = ResearchProvenanceReceiptV1::new(
        "research-project",
        9,
        &run.id,
        "figure-1",
        ResearchArtifactKindV1::Figure,
        reference.content_digest.clone(),
        vec![evidence.snapshot_digest.clone()],
        digest_bytes("research-workflow", b"workflow-v1"),
        digest_bytes("research-code", b"code-v1"),
        digest_bytes("research-environment", b"env-v1"),
        "other-provider",
        Some(digest_bytes("research-model", b"fixture-model")),
        Some(41),
        Some(digest_bytes("research-validation", b"validated")),
    )
    .unwrap();
    let provider_drift_finding = ResearchReviewFindingV1::new(
        "finding-drifted-provider",
        "research-project",
        &run.id,
        reference.content_digest.clone(),
        ResearchReviewCategoryV1::Reproducibility,
        ResearchReviewSeverityV1::Warning,
        "The provenance provider must match the admitted Run.",
        None,
        vec![evidence.snapshot_digest.clone()],
        "research-reviewer",
        61_008,
    )
    .unwrap();
    assert_eq!(
        provider_drift_finding.bind_provenance_receipt_for_run(&provider_drift_receipt, &research),
        Err(a3s_code_core::ResearchContractError::InvalidField(
            "provenanceReceipt.providerId"
        ))
    );

    let seed_drift_receipt = ResearchProvenanceReceiptV1::new(
        "research-project",
        9,
        &run.id,
        "figure-1",
        ResearchArtifactKindV1::Figure,
        reference.content_digest.clone(),
        vec![evidence.snapshot_digest.clone()],
        digest_bytes("research-workflow", b"workflow-v1"),
        digest_bytes("research-code", b"code-v1"),
        digest_bytes("research-environment", b"env-v1"),
        "fixture-provider",
        Some(digest_bytes("research-model", b"fixture-model")),
        Some(42),
        Some(digest_bytes("research-validation", b"validated")),
    )
    .unwrap();
    let seed_drift_finding = ResearchReviewFindingV1::new(
        "finding-drifted-seed",
        "research-project",
        &run.id,
        reference.content_digest.clone(),
        ResearchReviewCategoryV1::Reproducibility,
        ResearchReviewSeverityV1::Warning,
        "The provenance seed must match the admitted Run.",
        None,
        vec![evidence.snapshot_digest.clone()],
        "research-reviewer",
        61_009,
    )
    .unwrap();
    assert_eq!(
        seed_drift_finding.bind_provenance_receipt_for_run(&seed_drift_receipt, &research),
        Err(a3s_code_core::ResearchContractError::InvalidField(
            "provenanceReceipt.randomSeed"
        ))
    );

    let planned_research = ResearchRunV1::new(
        &run.id,
        "research-project",
        9,
        digest_bytes("research-source", b"source-snapshot"),
        evidence.snapshot_digest.clone(),
        capability_binding.clone(),
        "fixture-provider",
        "fixture-model",
        ResearchReproducibilityV1::Deterministic,
        Some(41),
    )
    .unwrap();
    let unadmitted_finding = ResearchReviewFindingV1::new(
        "finding-unadmitted-run",
        "research-project",
        &run.id,
        reference.content_digest.clone(),
        ResearchReviewCategoryV1::Reproducibility,
        ResearchReviewSeverityV1::Warning,
        "Reviewer output requires an admitted Run.",
        None,
        vec![evidence.snapshot_digest.clone()],
        "research-reviewer",
        61_010,
    )
    .unwrap();
    assert_eq!(
        unadmitted_finding.bind_evaluation_record_for_run(&reopened_record, &planned_research),
        Err(a3s_code_core::ResearchContractError::InvalidField(
            "researchRun.status"
        ))
    );

    let batch = ResearchReviewBatchV1::new_for_run(
        "research-review-batch-1",
        &research,
        &reopened_record,
        evidence.snapshot_digest.clone(),
        vec![finding],
    )
    .unwrap();
    assert!(batch.validate().is_ok());
    assert!(batch.validate_for_run(&research, &reopened_record).is_ok());
    assert_eq!(batch.findings[0].status, ResearchReviewStatusV1::Open);
    assert_eq!(
        batch.findings[0].provenance_receipt_digest.as_deref(),
        Some(reopened_receipt.receipt_digest.as_str())
    );
    assert_eq!(
        ResearchReviewBatchV1::new_for_run(
            "research-review-batch-unadmitted",
            &planned_research,
            &reopened_record,
            evidence.snapshot_digest.clone(),
            vec![batch.findings[0].clone()],
        ),
        Err(a3s_code_core::ResearchContractError::InvalidField(
            "researchRun.status"
        ))
    );

    let mut preclosed_finding = batch.findings[0].clone();
    preclosed_finding.resolve(digest('9')).unwrap();
    assert_eq!(
        ResearchReviewBatchV1::new_for_run(
            "research-review-batch-preclosed",
            &research,
            &reopened_record,
            evidence.snapshot_digest.clone(),
            vec![preclosed_finding],
        ),
        Err(a3s_code_core::ResearchContractError::InvalidField(
            "finding.status"
        ))
    );

    #[derive(serde::Serialize)]
    struct ForgedFindingIdentity<'a> {
        schema: &'a str,
        finding_id: &'a str,
        project_id: &'a str,
        run_id: &'a str,
        artifact_digest: &'a str,
        category: ResearchReviewCategoryV1,
        severity: ResearchReviewSeverityV1,
        status: ResearchReviewStatusV1,
        message: &'a str,
        location: &'a Option<a3s_code_core::ResearchReviewLocationV1>,
        evidence_digests: &'a [String],
        evaluator_id: &'a str,
        evaluation_record_digest: &'a str,
        provenance_receipt_digest: &'a str,
        observed_at_ms: u64,
        resolution_digest: Option<&'a str>,
    }
    let mut forged_finding = batch.findings[0].clone();
    forged_finding.evaluator_id = "forged-reviewer".to_owned();
    forged_finding.finding_digest = a3s_code_core::digest_json(
        "a3s.code.review-finding.identity.v1",
        &ForgedFindingIdentity {
            schema: &forged_finding.schema,
            finding_id: &forged_finding.finding_id,
            project_id: &forged_finding.project_id,
            run_id: &forged_finding.run_id,
            artifact_digest: &forged_finding.artifact_digest,
            category: forged_finding.category,
            severity: forged_finding.severity,
            status: forged_finding.status,
            message: &forged_finding.message,
            location: &forged_finding.location,
            evidence_digests: &forged_finding.evidence_digests,
            evaluator_id: &forged_finding.evaluator_id,
            evaluation_record_digest: forged_finding.evaluation_record_digest.as_deref().unwrap(),
            provenance_receipt_digest: forged_finding.provenance_receipt_digest.as_deref().unwrap(),
            observed_at_ms: forged_finding.observed_at_ms,
            resolution_digest: forged_finding.resolution_digest.as_deref(),
        },
    )
    .unwrap();
    let forged_batch = ResearchReviewBatchV1::new(
        "research-review-batch-forged-evaluator",
        "research-project",
        &run.id,
        reopened_record.record_digest.clone(),
        evidence.snapshot_digest.clone(),
        vec![forged_finding],
    )
    .unwrap();
    assert_eq!(
        forged_batch.validate_for_run(&research, &reopened_record),
        Err(a3s_code_core::ResearchContractError::InvalidField(
            "finding.evaluatorId"
        ))
    );

    let alternate_evidence_digest = digest_bytes("research-review-evidence", b"different");
    let mixed_evidence_finding = ResearchReviewFindingV1::new(
        "finding-mixed-evidence",
        "research-project",
        &run.id,
        reference.content_digest.clone(),
        ResearchReviewCategoryV1::Reproducibility,
        ResearchReviewSeverityV1::Warning,
        "The batch evidence must match the evaluator record.",
        None,
        vec![
            evidence.snapshot_digest.clone(),
            alternate_evidence_digest.clone(),
        ],
        "research-reviewer",
        61_009,
    )
    .unwrap()
    .bind_evaluation_record_for_run(&reopened_record, &research)
    .unwrap();
    let mixed_evidence_batch = ResearchReviewBatchV1::new(
        "research-review-batch-mixed-evidence",
        "research-project",
        &run.id,
        reopened_record.record_digest.clone(),
        alternate_evidence_digest,
        vec![mixed_evidence_finding],
    )
    .unwrap();
    assert_eq!(
        mixed_evidence_batch.validate_for_run(&research, &reopened_record),
        Err(a3s_code_core::ResearchContractError::InvalidField(
            "evaluationRecord.evidenceDigest"
        ))
    );
}

#[tokio::test]
async fn research_cancellation_is_terminal_and_cannot_resume() {
    let target = ExecutionTargetV1::new("cancel-session", "cancel-run");
    let binding = use_bound_capability();
    let mut research = ResearchRunV1::new(
        "cancel-run",
        "cancel-project",
        1,
        digest('a'),
        digest('b'),
        binding,
        "fixture-provider",
        "fixture-model",
        ResearchReproducibilityV1::Reproducible,
        None,
    )
    .unwrap();
    research
        .transition_to(ResearchRunStatusV1::Admitted)
        .unwrap();
    research
        .transition_to(ResearchRunStatusV1::Running)
        .unwrap();
    research
        .transition_to(ResearchRunStatusV1::Cancelled)
        .unwrap();
    let reopened: ResearchRunV1 =
        serde_json::from_slice(&serde_json::to_vec(&research).unwrap()).unwrap();
    assert!(reopened.validate_execution_target(&target).is_ok());
    assert!(matches!(
        research.transition_to(ResearchRunStatusV1::Running),
        Err(a3s_code_core::ResearchContractError::InvalidTransition { .. })
    ));
}

#[test]
fn capability_binding_round_trip_retains_the_exact_use_generation() {
    let binding = use_bound_capability();
    let encoded = serde_json::to_vec(&binding).unwrap();
    let reopened: RunCapabilityBindingV1 = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(reopened, binding);
    assert_eq!(reopened.code_catalog_generation(), 3);
    let use_generation = reopened.use_generation().unwrap();
    assert_eq!(use_generation.generation(), 7);
    assert_eq!(use_generation.revision(), digest('a'));
    assert_eq!(use_generation.registry_revision(), digest('b'));
}
