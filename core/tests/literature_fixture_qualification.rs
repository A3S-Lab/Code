#![cfg(feature = "research")]
//! Local literature-plus-data fixture over the delivered research contracts.
//!
//! The host owns the reviewer comparison, the approval event, and export.
//! Code only admits the existing research Run, evaluation record, and
//! reproducibility manifest. A finding does not approve the run, and this
//! fixture does not resolve a package or open a second research store.

use a3s_code_core::capability::{
    CapabilityCeiling, CapabilityContribution, CapabilityDescriptor, CapabilityExecutionCeiling,
    CapabilityKind, CapabilitySet, CapabilitySource, CodeCatalogGeneration,
    GovernanceCapabilityCeiling, RunCapabilityBindingV1, Sha256Digest, UseCapabilityGeneration,
    UsePackageGeneration, WorkspaceCapabilityCeiling,
};
use a3s_code_core::evaluation::{
    AuxiliaryCapabilityProfileV1, AuxiliaryExecutor, AuxiliaryModeV1, AuxiliaryRunContextV1,
    AuxiliaryRunError, AuxiliaryRunService, AuxiliaryRunSpecV1, EvaluationRecordV1,
    EvaluationResultSink, EvaluationResultV1, EvidenceContentModeV1, EvidenceReadRequestV1,
    ExecutionFactRecorder, ExecutionFrameV1, ExecutionTargetV1, InMemoryAuxiliaryRunService,
    InMemoryEvaluationResultStore, InMemoryExecutionFactJournal, RunEvidenceReader,
};
use a3s_code_core::{
    digest_bytes, AgentEvent, InMemoryRunStore, ResearchArtifactKindV1, ResearchCitationV1,
    ResearchClaimV1, ResearchEventV1, ResearchEvidenceFactKindV1, ResearchEvidenceFactV1,
    ResearchEvidenceGraphV1, ResearchProvenanceReceiptV1, ResearchReproducibilityManifestV1,
    ResearchReproducibilityV1, ResearchReviewBatchV1, ResearchReviewCategoryV1,
    ResearchReviewFindingV1, ResearchReviewLocationV1, ResearchReviewSeverityV1,
    ResearchReviewStatusV1, ResearchRunStatusV1, ResearchRunV1, ResearchWireEnvelopeV1,
    ResearchWireKindV1, ResearchWorkflowPlanV1, ResearchWorkflowStepV1, RunEventRecord,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

const LITERATURE: &[u8] = b"Local cohort note. Reported mean is twelve.";
const DATA: &[u8] = b"10\n14\n";
const SEED: u64 = 17;
const OBSERVED_AT_MS: u64 = 1_700_000_000_000;
const PROJECT_ID: &str = "project-literature";
const PROVIDER_ID: &str = "local-fixture";
const MODEL_ID: &str = "deterministic-analyzer";
const SOURCE_DOMAIN: &str = "a3s.code.literature-fixture.source.v1";
const DATA_DOMAIN: &str = "a3s.code.literature-fixture.data.v1";
const SNAPSHOT_DOMAIN: &str = "a3s.code.literature-fixture.snapshot.v1";
const ARTIFACT_DOMAIN: &str = "a3s.code.literature-fixture.artifact.v1";
const CODE_DOMAIN: &str = "a3s.code.literature-fixture.code.v1";
const WORKFLOW_DOMAIN: &str = "a3s.code.literature-fixture.workflow.v1";
const ENVIRONMENT_DOMAIN: &str = "a3s.code.literature-fixture.environment.v1";
const PARAMETER_DOMAIN: &str = "a3s.code.literature-fixture.parameters.v1";
const APPROVAL_DOMAIN: &str = "a3s.code.literature-fixture.approval.v1";
const APPROVAL_EVENT: &str = "research.approval.recorded";

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DraftObservation {
    cited: String,
    reported_total: u64,
    analysis_digest: String,
}

struct LiteratureReviewer {
    expected_run: String,
}

#[async_trait]
impl AuxiliaryExecutor for LiteratureReviewer {
    async fn execute(
        &self,
        context: AuxiliaryRunContextV1,
    ) -> Result<serde_json::Value, AuxiliaryRunError> {
        if context.spec.parent.target.run_id != self.expected_run {
            return Err(AuxiliaryRunError::TargetMismatch);
        }
        if !context.evidence.complete
            || context.evidence.retention_gap
            || context.evidence.events.len() != 1
        {
            return Err(AuxiliaryRunError::EvidenceMismatch);
        }
        let Some(text) = context.evidence.events[0]
            .event
            .payload
            .get("text")
            .and_then(serde_json::Value::as_str)
        else {
            return Err(AuxiliaryRunError::EvidenceMismatch);
        };
        let draft: DraftObservation =
            serde_json::from_str(text).map_err(|_| AuxiliaryRunError::EvidenceMismatch)?;
        let literature = digest_bytes(SOURCE_DOMAIN, LITERATURE);
        let analysis = artifact_digest(LITERATURE, DATA, SEED);
        let total = measurement_total(DATA);
        let mut findings = Vec::new();
        if draft.cited != literature {
            findings.push(serde_json::json!({
                "findingId": "citation-fault",
                "category": "citation",
                "message": "cited source is not the admitted literature digest"
            }));
        }
        if draft.reported_total != total {
            findings.push(serde_json::json!({
                "findingId": "numeric-fault",
                "category": "numeric",
                "message": "reported total does not match the measurement table"
            }));
        }
        if draft.analysis_digest != analysis {
            findings.push(serde_json::json!({
                "findingId": "figure-code-fault",
                "category": "figure-code",
                "message": "analysis digest does not match the pinned code and seed"
            }));
        }
        Ok(serde_json::json!({ "findings": findings }))
    }
}

struct GateReview {
    evidence_digest: String,
    record: EvaluationRecordV1,
    output: serde_json::Value,
}

fn measurement_total(data: &[u8]) -> u64 {
    std::str::from_utf8(data)
        .expect("measurement table is utf-8")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.parse::<u64>().expect("measurement row"))
        .sum()
}

fn analyze(literature: &[u8], data: &[u8], seed: u64) -> Vec<u8> {
    let literature = digest_bytes(SOURCE_DOMAIN, literature);
    format!(
        "seed={seed}\nliterature={literature}\ntotal={}\n",
        measurement_total(data)
    )
    .into_bytes()
}

fn artifact_digest(literature: &[u8], data: &[u8], seed: u64) -> String {
    digest_bytes(ARTIFACT_DOMAIN, &analyze(literature, data, seed))
}

fn source_snapshot_digest() -> String {
    let mut bytes = Vec::with_capacity(LITERATURE.len() + DATA.len());
    bytes.extend_from_slice(LITERATURE);
    bytes.extend_from_slice(DATA);
    digest_bytes(SNAPSHOT_DOMAIN, &bytes)
}

fn sha_digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
}

/// Host-injected capability binding. The fixture does not resolve a registry.
fn use_bound_capability() -> RunCapabilityBindingV1 {
    let use_generation = UseCapabilityGeneration::new(7, sha_digest('a'), sha_digest('b'));
    let package = UsePackageGeneration::new(
        "acme/research-runtime",
        "literature",
        "literature",
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
        "literature-analysis",
        "literature_analysis",
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

fn pinned_digest(domain: &str, label: &str) -> String {
    digest_bytes(domain, label.as_bytes())
}

fn approval_digest(run: &ResearchRunV1, manifest: &ResearchReproducibilityManifestV1) -> String {
    digest_bytes(
        APPROVAL_DOMAIN,
        format!(
            "{}\n{}\n{}\n{}",
            run.run_id,
            run.source_snapshot_digest,
            run.evidence_snapshot_digest,
            manifest.manifest_digest
        )
        .as_bytes(),
    )
}

fn approval_event(
    run: &ResearchRunV1,
    manifest: &ResearchReproducibilityManifestV1,
) -> ResearchEventV1 {
    ResearchEventV1::new(
        run.project_id.clone(),
        run.project_revision,
        Some(run.run_id.clone()),
        1,
        APPROVAL_EVENT,
        approval_digest(run, manifest),
        OBSERVED_AT_MS,
    )
    .expect("approval event")
}

fn admit_run(run_id: &str, evidence_digest: String) -> ResearchRunV1 {
    ResearchRunV1::new(
        run_id,
        PROJECT_ID,
        1,
        source_snapshot_digest(),
        evidence_digest,
        use_bound_capability(),
        PROVIDER_ID,
        MODEL_ID,
        ResearchReproducibilityV1::Deterministic,
        Some(SEED),
    )
    .expect("research run")
}

fn evidence_fact(
    run_id: &str,
    sequence: u64,
    kind: ResearchEvidenceFactKindV1,
    subject: &str,
    content_digest: String,
    snapshot: &str,
) -> ResearchEvidenceFactV1 {
    ResearchEvidenceFactV1::new(
        run_id,
        sequence,
        kind,
        subject,
        None,
        content_digest,
        BTreeMap::new(),
        OBSERVED_AT_MS,
    )
    .expect("evidence fact")
    .with_source_snapshot_digest(snapshot)
    .expect("source snapshot")
}

fn manifest_for(
    run: &ResearchRunV1,
    manifest_id: &str,
    artifact: &str,
) -> (
    ResearchWorkflowPlanV1,
    ResearchProvenanceReceiptV1,
    ResearchReproducibilityManifestV1,
) {
    let workflow = pinned_digest(WORKFLOW_DOMAIN, "literature-plus-data");
    let code = pinned_digest(CODE_DOMAIN, "literature-fixture-analyze-v1");
    let environment = pinned_digest(ENVIRONMENT_DOMAIN, "local-lock");
    let literature = digest_bytes(SOURCE_DOMAIN, LITERATURE);
    let data = digest_bytes(DATA_DOMAIN, DATA);
    let ingest = ResearchWorkflowStepV1::new(
        "01-ingest",
        run.project_id.clone(),
        run.run_id.clone(),
        workflow.clone(),
        pinned_digest(CODE_DOMAIN, "01-ingest"),
        vec![literature.clone(), data.clone()],
        vec![data.clone()],
        Vec::new(),
    )
    .expect("ingest step");
    let analyze_step = ResearchWorkflowStepV1::new(
        "02-analyze",
        run.project_id.clone(),
        run.run_id.clone(),
        workflow.clone(),
        pinned_digest(CODE_DOMAIN, "02-analyze"),
        vec![literature.clone(), data.clone()],
        vec![artifact.to_string()],
        vec!["01-ingest".to_string()],
    )
    .expect("analyze step");
    let plan = ResearchWorkflowPlanV1::new_for_run(
        format!("{manifest_id}-plan"),
        run,
        workflow.clone(),
        vec![ingest, analyze_step],
    )
    .expect("workflow plan");
    let receipt = ResearchProvenanceReceiptV1::new(
        run.project_id.clone(),
        run.project_revision,
        run.run_id.clone(),
        "table-mean",
        ResearchArtifactKindV1::Table,
        artifact,
        vec![literature, data],
        workflow.clone(),
        code.clone(),
        environment.clone(),
        run.provider_id.clone(),
        None,
        run.random_seed,
        None,
    )
    .expect("provenance receipt");
    let manifest = ResearchReproducibilityManifestV1::new_for_run(
        manifest_id,
        run,
        None,
        environment,
        code,
        workflow,
        pinned_digest(PARAMETER_DOMAIN, &SEED.to_string()),
        Vec::new(),
        vec![artifact.to_string()],
    )
    .expect("reproducibility manifest");
    manifest
        .validate_against_provenance(std::slice::from_ref(&receipt))
        .expect("manifest provenance");
    plan.validate_for_run(run).expect("plan run");
    (plan, receipt, manifest)
}

fn project_findings(
    run: &ResearchRunV1,
    record: &EvaluationRecordV1,
    receipt: &ResearchProvenanceReceiptV1,
    output: &serde_json::Value,
) -> Vec<ResearchReviewFindingV1> {
    let rows = output
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .expect("reviewer findings");
    rows.iter()
        .map(|row| {
            let finding_id = row
                .get("findingId")
                .and_then(|value| value.as_str())
                .unwrap();
            let category = match row.get("category").and_then(|value| value.as_str()) {
                Some("citation") => ResearchReviewCategoryV1::Citation,
                Some("numeric") => ResearchReviewCategoryV1::Numeric,
                Some("figure-code") => ResearchReviewCategoryV1::FigureCode,
                other => panic!("unexpected reviewer category {other:?}"),
            };
            let message = row.get("message").and_then(|value| value.as_str()).unwrap();
            let location = (category == ResearchReviewCategoryV1::Citation).then(|| {
                ResearchReviewLocationV1::new("table-mean")
                    .unwrap()
                    .with_line(1, Some(1))
            });
            ResearchReviewFindingV1::new(
                finding_id,
                run.project_id.clone(),
                run.run_id.clone(),
                receipt.artifact_digest.clone(),
                category,
                ResearchReviewSeverityV1::Error,
                message,
                location,
                vec![
                    run.evidence_snapshot_digest.clone(),
                    digest_bytes(SOURCE_DOMAIN, LITERATURE),
                ],
                record.result.evaluator_id.clone(),
                OBSERVED_AT_MS,
            )
            .expect("finding")
            .bind_evaluation_record_for_run(record, run)
            .expect("evaluation binding")
            .bind_provenance_receipt_for_run(receipt, run)
            .expect("provenance binding")
        })
        .collect()
}

fn export_bundle(
    run: &ResearchRunV1,
    manifest: &ResearchReproducibilityManifestV1,
    receipt: &ResearchProvenanceReceiptV1,
    batch: Option<&ResearchReviewBatchV1>,
    approval: Option<&ResearchEventV1>,
) -> Result<Vec<u8>, &'static str> {
    let Some(approval) = approval else {
        return Err("approval missing");
    };
    if approval.event_type != APPROVAL_EVENT
        || approval.run_id.as_deref() != Some(run.run_id.as_str())
    {
        return Err("approval event");
    }
    if approval.payload_digest != approval_digest(run, manifest) {
        return Err("approval digest");
    }
    if let Some(batch) = batch {
        if batch.project_id != run.project_id || batch.run_id != run.run_id {
            return Err("batch run");
        }
    }
    manifest.validate_for_run(run).map_err(|_| "manifest")?;
    manifest
        .validate_against_provenance(std::slice::from_ref(receipt))
        .map_err(|_| "provenance")?;
    let mut envelopes = vec![
        ResearchWireEnvelopeV1::new(
            ResearchWireKindV1::ResearchRun,
            serde_json::to_value(run).map_err(|_| "run")?,
        )
        .map_err(|_| "run envelope")?,
        ResearchWireEnvelopeV1::from_reproducibility_manifest(manifest.clone())
            .map_err(|_| "manifest envelope")?,
        ResearchWireEnvelopeV1::from_provenance_receipt(receipt.clone())
            .map_err(|_| "receipt envelope")?,
    ];
    if let Some(batch) = batch {
        envelopes.push(
            ResearchWireEnvelopeV1::new(
                ResearchWireKindV1::ResearchReviewBatch,
                serde_json::to_value(batch).map_err(|_| "batch")?,
            )
            .map_err(|_| "batch envelope")?,
        );
    }
    envelopes.push(
        ResearchWireEnvelopeV1::from_research_event(approval.clone())
            .map_err(|_| "approval envelope")?,
    );
    serde_json::to_vec(&envelopes).map_err(|_| "export")
}

fn assert_export_round_trip(bytes: &[u8], manifest: &ResearchReproducibilityManifestV1) {
    let text = std::str::from_utf8(bytes).expect("export utf-8");
    assert!(!text.contains("Local cohort note"));
    assert!(!text.contains("twelve"));
    assert!(!text.contains("10\n14"));
    let envelopes: Vec<ResearchWireEnvelopeV1> =
        serde_json::from_slice(bytes).expect("export envelopes");
    assert!(envelopes.len() >= 4);
    let decoded = envelopes
        .iter()
        .find(|envelope| envelope.kind() == ResearchWireKindV1::ResearchReproducibilityManifest)
        .expect("manifest envelope")
        .payload_as::<ResearchReproducibilityManifestV1>(
            ResearchWireKindV1::ResearchReproducibilityManifest,
        )
        .expect("manifest payload");
    assert_eq!(decoded, *manifest);
}

async fn gate_review(run_id: &str, draft: &DraftObservation) -> GateReview {
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id(
            run_id.to_string(),
            "session-literature",
            "literature-fixture",
        )
        .await;
    let draft_json = serde_json::to_string(draft).expect("draft json");
    let recorded = runs
        .record_event(
            &run.id,
            AgentEvent::TextDelta {
                text: draft_json.clone(),
            },
        )
        .await
        .expect("record draft");
    let target = ExecutionTargetV1::new("session-literature", &run.id);
    let journal = Arc::new(InMemoryExecutionFactJournal::new());
    ExecutionFactRecorder::new(journal.clone(), ExecutionFrameV1::root(target.clone()))
        .record(&RunEventRecord {
            sequence: 0,
            timestamp_ms: recorded.updated_at_ms,
            event: AgentEvent::TextDelta { text: draft_json },
        })
        .expect("execution fact");
    let mut request = EvidenceReadRequestV1::new(target.clone());
    request.content_mode = EvidenceContentModeV1::BoundedPayload;
    let evidence = RunEvidenceReader::new(Arc::clone(&runs))
        .with_facts(journal)
        .read(request)
        .await
        .expect("evidence");
    assert!(
        evidence.complete,
        "Gate-mode review requires complete evidence"
    );
    let evidence_digest = evidence.snapshot_digest.clone();
    let service = InMemoryAuxiliaryRunService::new(Arc::new(LiteratureReviewer {
        expected_run: run.id.clone(),
    }));
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(target.clone()),
        "research-review",
        "compare the draft with the admitted literature and measurement table",
        evidence_digest.clone(),
    )
    .with_id(format!("aux-{run_id}"))
    .with_mode(AuxiliaryModeV1::Gate)
    .with_capabilities(AuxiliaryCapabilityProfileV1::tool_free());
    let output = service
        .spawn(spec, evidence, None)
        .await
        .expect("gate spawn")
        .wait()
        .await
        .expect("gate wait")
        .value;
    let result = EvaluationResultV1::new(
        "literature-fixture-reviewer",
        target,
        format!("aux-{run_id}"),
        if output["findings"]
            .as_array()
            .is_some_and(|rows| rows.is_empty())
        {
            "clean"
        } else {
            "fault"
        },
        output.clone(),
        evidence_digest.clone(),
    )
    .expect("evaluation result");
    let record = EvaluationRecordV1::new(result, OBSERVED_AT_MS).expect("evaluation record");
    let store = InMemoryEvaluationResultStore::new();
    assert!(store.write(record.clone()).await.expect("store").written);
    let retained = store
        .get(&record.record_digest)
        .await
        .expect("retained evaluation record");
    assert_eq!(retained, record);
    GateReview {
        evidence_digest,
        record,
        output,
    }
}

#[tokio::test]
async fn literature_plus_data_fixture_restarts_forks_reviews_and_exports() {
    let first = analyze(LITERATURE, DATA, SEED);
    let second = analyze(LITERATURE, DATA, SEED);
    assert_eq!(
        first, second,
        "the same literature, data, and seed analyze once"
    );
    let artifact = artifact_digest(LITERATURE, DATA, SEED);
    assert_ne!(artifact_digest(LITERATURE, b"10\n15\n", SEED), artifact);
    assert_ne!(artifact_digest(LITERATURE, DATA, SEED + 1), artifact);
    assert_ne!(
        artifact_digest(b"a different local note", DATA, SEED),
        artifact
    );
    assert_eq!(measurement_total(DATA), 24);

    let fault = gate_review(
        "run-literature",
        &DraftObservation {
            cited: digest_bytes(SOURCE_DOMAIN, b"foreign-paper"),
            reported_total: 99,
            analysis_digest: digest_bytes(ARTIFACT_DOMAIN, b"tampered-table"),
        },
    )
    .await;
    let mut parent = admit_run("run-literature", fault.evidence_digest.clone());
    parent
        .validate_execution_target(&ExecutionTargetV1::new(
            "session-literature",
            "run-literature",
        ))
        .unwrap();
    let snapshot = parent.source_snapshot_digest.clone();
    let facts = vec![
        evidence_fact(
            &parent.run_id,
            1,
            ResearchEvidenceFactKindV1::Source,
            "literature-note",
            digest_bytes(SOURCE_DOMAIN, LITERATURE),
            &snapshot,
        ),
        evidence_fact(
            &parent.run_id,
            2,
            ResearchEvidenceFactKindV1::Measurement,
            "cohort-table",
            digest_bytes(DATA_DOMAIN, DATA),
            &snapshot,
        ),
        evidence_fact(
            &parent.run_id,
            3,
            ResearchEvidenceFactKindV1::Artifact,
            "table-mean",
            artifact.clone(),
            &snapshot,
        ),
    ];
    assert!(facts
        .iter()
        .all(|fact| fact.source_snapshot_digest() == Some(snapshot.as_str())));
    let citation = ResearchCitationV1::new(
        "cite-note",
        PROJECT_ID,
        &parent.run_id,
        "claim-total",
        digest_bytes(SOURCE_DOMAIN, LITERATURE),
        digest_bytes("a3s.code.literature-fixture.span.v1", b"note#p1"),
        None,
        OBSERVED_AT_MS,
    )
    .unwrap();
    let claim = ResearchClaimV1::new(
        "claim-total",
        PROJECT_ID,
        &parent.run_id,
        digest_bytes(
            "a3s.code.literature-fixture.claim.v1",
            b"cohort total is 24",
        ),
        OBSERVED_AT_MS,
    )
    .unwrap()
    .mark_supported(vec![citation.citation_digest.clone()])
    .unwrap();
    let graph = ResearchEvidenceGraphV1::new_for_run(
        "graph-literature",
        &parent,
        vec![claim],
        vec![citation],
    )
    .unwrap();
    graph.validate_publication_completeness().unwrap();

    parent.transition_to(ResearchRunStatusV1::Admitted).unwrap();
    parent.transition_to(ResearchRunStatusV1::Running).unwrap();
    parent
        .transition_to(ResearchRunStatusV1::Checkpointed)
        .unwrap();
    let checkpoint = parent.to_vec().unwrap();
    let fact_bytes: Vec<_> = facts.iter().map(|fact| fact.to_vec().unwrap()).collect();
    let graph_bytes = graph.to_vec().unwrap();
    let mut restarted = ResearchRunV1::from_slice(&checkpoint).unwrap();
    assert_eq!(restarted, parent);
    restarted
        .transition_to(ResearchRunStatusV1::Running)
        .unwrap();
    assert_eq!(restarted.run_id, parent.run_id);
    assert_eq!(restarted.source_snapshot_digest, snapshot);
    assert_eq!(restarted.evidence_snapshot_digest, fault.evidence_digest);
    assert_eq!(restarted.random_seed, Some(SEED));
    assert_ne!(restarted.run_digest, parent.run_digest);
    let reopened_facts: Vec<_> = fact_bytes
        .iter()
        .map(|bytes| ResearchEvidenceFactV1::from_slice(bytes).unwrap())
        .collect();
    assert_eq!(reopened_facts, facts);
    assert_eq!(
        ResearchEvidenceGraphV1::from_slice(&graph_bytes).unwrap(),
        graph
    );
    let mut terminal = restarted.clone();
    terminal
        .transition_to(ResearchRunStatusV1::Completed)
        .unwrap();
    assert!(terminal
        .transition_to(ResearchRunStatusV1::Running)
        .is_err());
    assert_eq!(restarted.status, ResearchRunStatusV1::Running);

    let (plan, receipt, manifest) = manifest_for(&restarted, "manifest-literature", &artifact);
    assert_eq!(manifest.output_artifact_digests, vec![artifact.clone()]);
    assert_eq!(
        ResearchWorkflowPlanV1::from_slice(&plan.to_vec().unwrap()).unwrap(),
        plan
    );
    let findings = project_findings(&restarted, &fault.record, &receipt, &fault.output);
    let batch = ResearchReviewBatchV1::new_for_run(
        "batch-literature",
        &restarted,
        &fault.record,
        restarted.evidence_snapshot_digest.clone(),
        findings,
    )
    .unwrap();
    assert_eq!(
        batch
            .findings
            .iter()
            .map(|finding| finding.category)
            .collect::<Vec<_>>(),
        vec![
            ResearchReviewCategoryV1::Citation,
            ResearchReviewCategoryV1::FigureCode,
            ResearchReviewCategoryV1::Numeric,
        ]
    );
    assert!(batch
        .findings
        .iter()
        .all(|finding| finding.status == ResearchReviewStatusV1::Open));
    assert_eq!(restarted.status, ResearchRunStatusV1::Running);
    assert!(graph.validate_publication_completeness().is_ok());
    assert!(export_bundle(&restarted, &manifest, &receipt, Some(&batch), None).is_err());
    let resolution = digest_bytes(
        "a3s.code.literature-fixture.resolution.v1",
        b"resolved-without-approval",
    );
    let mut resolved = batch.findings[0].clone();
    resolved.resolve(resolution.clone()).unwrap();
    assert_ne!(resolution, approval_digest(&restarted, &manifest));
    assert_eq!(resolved.status, ResearchReviewStatusV1::Resolved);
    assert!(export_bundle(&restarted, &manifest, &receipt, Some(&batch), None).is_err());
    assert!(batch
        .findings
        .iter()
        .all(|finding| finding.status == ResearchReviewStatusV1::Open));

    let parent_bytes = restarted.to_vec().unwrap();
    let parent_facts = facts.clone();
    let mut fork = ResearchRunV1::new(
        "run-literature-fork",
        restarted.project_id.clone(),
        restarted.project_revision,
        restarted.source_snapshot_digest.clone(),
        restarted.evidence_snapshot_digest.clone(),
        restarted.capability_binding.clone(),
        restarted.provider_id.clone(),
        restarted.model_id.clone(),
        ResearchReproducibilityV1::Deterministic,
        restarted.random_seed,
    )
    .unwrap();
    fork.transition_to(ResearchRunStatusV1::Admitted).unwrap();
    assert_eq!(
        fork.source_snapshot_digest,
        restarted.source_snapshot_digest
    );
    assert_eq!(
        fork.evidence_snapshot_digest,
        restarted.evidence_snapshot_digest
    );
    assert_eq!(fork.random_seed, Some(SEED));
    assert_ne!(fork.run_id, restarted.run_id);
    assert_ne!(fork.run_digest, restarted.run_digest);
    let fork_fact = evidence_fact(
        &fork.run_id,
        1,
        ResearchEvidenceFactKindV1::Source,
        "literature-note",
        digest_bytes(SOURCE_DOMAIN, LITERATURE),
        &fork.source_snapshot_digest,
    );
    assert_eq!(fork_fact.content_digest, facts[0].content_digest);
    assert_ne!(fork_fact.fact_digest, facts[0].fact_digest);
    assert_eq!(fork_fact.run_id, fork.run_id);
    assert_eq!(facts, parent_facts);
    assert_eq!(restarted.to_vec().unwrap(), parent_bytes);
    let (_, fork_receipt, fork_manifest) =
        manifest_for(&fork, "manifest-literature-fork", &artifact);
    assert_eq!(
        fork_manifest.output_artifact_digests,
        vec![artifact.clone()]
    );
    let parent_approval = approval_event(&restarted, &manifest);
    assert!(ResearchReviewBatchV1::new_for_run(
        "batch-literature-fork",
        &fork,
        &fault.record,
        fork.evidence_snapshot_digest.clone(),
        Vec::new(),
    )
    .is_err());
    assert!(export_bundle(
        &fork,
        &fork_manifest,
        &fork_receipt,
        None,
        Some(&parent_approval),
    )
    .is_err());

    let clean = gate_review(
        "run-literature-clean",
        &DraftObservation {
            cited: digest_bytes(SOURCE_DOMAIN, LITERATURE),
            reported_total: 24,
            analysis_digest: artifact.clone(),
        },
    )
    .await;
    let mut clean_run = admit_run("run-literature-clean", clean.evidence_digest.clone());
    clean_run
        .transition_to(ResearchRunStatusV1::Admitted)
        .unwrap();
    assert_eq!(clean_run.source_snapshot_digest, snapshot);
    assert_eq!(clean_run.random_seed, Some(SEED));
    let (_, clean_receipt, clean_manifest) =
        manifest_for(&clean_run, "manifest-literature-clean", &artifact);
    let clean_batch = ResearchReviewBatchV1::new_for_run(
        "batch-literature-clean",
        &clean_run,
        &clean.record,
        clean_run.evidence_snapshot_digest.clone(),
        Vec::new(),
    )
    .unwrap();
    assert!(clean_batch.findings.is_empty());
    assert_eq!(clean_run.status, ResearchRunStatusV1::Admitted);
    assert!(export_bundle(
        &clean_run,
        &clean_manifest,
        &clean_receipt,
        Some(&clean_batch),
        None
    )
    .is_err());

    let parent_export = export_bundle(
        &restarted,
        &manifest,
        &receipt,
        Some(&batch),
        Some(&parent_approval),
    )
    .expect("parent export");
    assert_export_round_trip(&parent_export, &manifest);
    let parent_envelopes: Vec<ResearchWireEnvelopeV1> =
        serde_json::from_slice(&parent_export).unwrap();
    let exported_batch = parent_envelopes
        .iter()
        .find(|envelope| envelope.kind() == ResearchWireKindV1::ResearchReviewBatch)
        .unwrap()
        .payload_as::<ResearchReviewBatchV1>(ResearchWireKindV1::ResearchReviewBatch)
        .unwrap();
    assert_eq!(exported_batch.findings.len(), 3);
    assert!(exported_batch
        .findings
        .iter()
        .all(|finding| finding.status == ResearchReviewStatusV1::Open));
    assert_eq!(restarted.status, ResearchRunStatusV1::Running);

    let fork_approval = approval_event(&fork, &fork_manifest);
    let fork_export = export_bundle(
        &fork,
        &fork_manifest,
        &fork_receipt,
        None,
        Some(&fork_approval),
    )
    .expect("fork export");
    assert_export_round_trip(&fork_export, &fork_manifest);
    assert_eq!(restarted.to_vec().unwrap(), parent_bytes);
    assert!(export_bundle(
        &restarted,
        &manifest,
        &receipt,
        Some(&batch),
        Some(&fork_approval)
    )
    .is_err());

    let runs = Arc::new(InMemoryRunStore::new());
    let incomplete_run = runs
        .create_run_with_id(
            "run-literature-incomplete".into(),
            "session-literature",
            "literature-fixture",
        )
        .await;
    runs.record_event(
        &incomplete_run.id,
        AgentEvent::TextDelta {
            text: "analysis that exceeds the bounded evidence budget".into(),
        },
    )
    .await
    .unwrap();
    let incomplete_target = ExecutionTargetV1::new("session-literature", &incomplete_run.id);
    let mut incomplete_request = EvidenceReadRequestV1::new(incomplete_target.clone());
    incomplete_request.content_mode = EvidenceContentModeV1::BoundedPayload;
    incomplete_request.limits.max_event_bytes = 1;
    let incomplete = RunEvidenceReader::new(runs)
        .read(incomplete_request)
        .await
        .unwrap();
    assert!(!incomplete.complete);
    let service = InMemoryAuxiliaryRunService::new(Arc::new(LiteratureReviewer {
        expected_run: incomplete_run.id.clone(),
    }));
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(incomplete_target),
        "research-review",
        "must not gate on incomplete evidence",
        incomplete.snapshot_digest.clone(),
    )
    .with_id("aux-literature-incomplete")
    .with_mode(AuxiliaryModeV1::Gate)
    .with_capabilities(AuxiliaryCapabilityProfileV1::tool_free());
    assert!(matches!(
        service.spawn(spec, incomplete, None).await,
        Err(AuxiliaryRunError::EvidenceIncomplete)
    ));
}
