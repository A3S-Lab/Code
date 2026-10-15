//! Live dual-agent research-review composition against `.a3s/config.acl`.
//!
//! The main agent (real model) performs a bounded analysis turn whose events
//! are captured as run evidence; the reviewer agent (real model, structured
//! output via `StructuredAuxiliaryExecutor`) reviews that evidence as a
//! Gate-mode auxiliary run; the findings bind through the provenance chain —
//! evidence snapshot digest → auxiliary output digest → evaluation record →
//! `ResearchReviewBatchV1::validate_for_run`.
//!
//! This is the live counterpart of `research_review_qualification.rs`
//! (which pins the substrate contracts with a mock reviewer executor).
//!
//! ```bash
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo test -p a3s-code-core --features research \
//!   --test test_research_review_real_llm -- --ignored --nocapture \
//!   --test-threads=1
//! ```

#![cfg(feature = "research")]

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use a3s_code_core::capability::{
    CapabilityCeiling, CapabilityContribution, CapabilityDescriptor, CapabilityKind, CapabilitySet,
    CapabilitySource, CodeCatalogGeneration, GovernanceCapabilityCeiling, RunCapabilityBindingV1,
    Sha256Digest, UseCapabilityGeneration, UsePackageGeneration, WorkspaceCapabilityCeiling,
};
use a3s_code_core::evaluation::{
    AuxiliaryCapabilityProfileV1, AuxiliaryModeV1, AuxiliaryRunContextV1, AuxiliaryRunService,
    AuxiliaryRunSpecV1, EvaluationRecordV1, EvaluationResultSink, EvaluationResultV1,
    EvidenceReadRequestV1, ExecutionFactRecorder, ExecutionFrameV1, ExecutionTargetV1,
    InMemoryAuxiliaryRunService, InMemoryEvaluationResultStore, InMemoryExecutionFactJournal,
    RunEvidenceReader, StructuredAuxiliaryExecutor,
};
use a3s_code_core::llm::create_client_with_config;
use a3s_code_core::llm::structured::{StructuredMode, StructuredRequest};
use a3s_code_core::{
    Agent, AgentEvent, InMemoryRunStore, ResearchArtifactKindV1, ResearchProvenanceReceiptV1,
    ResearchReproducibilityV1, ResearchReviewBatchV1, ResearchReviewCategoryV1,
    ResearchReviewFindingV1, ResearchReviewSeverityV1, ResearchRunStatusV1, ResearchRunV1,
    RunEventRecord, SessionOptions,
};
use support::layer_c_model::load_pinned_layer_c_config;

const TURN_TIMEOUT: Duration = Duration::from_secs(420);
const REVIEW_TIMEOUT: Duration = Duration::from_secs(420);

fn sha_repeat(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn sha_digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(sha_repeat(byte)).unwrap()
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
        a3s_code_core::capability::CapabilityExecutionCeiling::new(8, 4, None, None, None).unwrap(),
    )
    .unwrap();
    RunCapabilityBindingV1::from_set_and_ceiling(&set, &ceiling).unwrap()
}

/// The review rubric the host pins for the reviewer agent. The reviewer sees
/// only the main run's evidence — never the prompt that produced it — so a
/// finding that quotes evidence is provenance the main agent cannot fake.
fn review_request(context: &AuxiliaryRunContextV1) -> StructuredRequest {
    let mut evidence_text = String::new();
    for event in &context.evidence.events {
        if let Some(text) = plain_text(&event.event) {
            evidence_text.push_str(&text);
            evidence_text.push('\n');
        }
    }
    StructuredRequest {
        prompt: format!(
            "You are an independent reviewer agent. Below is the captured event \
             evidence of another agent's analysis run. Review it strictly against \
             this rubric: (1) citation — every factual claim must be grounded in \
             evidence text; (2) reproducibility — the stated conclusion must \
             follow from the evidence. Output one finding per violation plus an \
             overall verdict.\n\nEVIDENCE:\n{evidence_text}"
        ),
        system: Some(
            "You are a rigorous review agent. Judge only from the evidence \
             provided; never assume facts outside it."
                .to_owned(),
        ),
        schema: serde_json::json!({
            "type": "object",
            "properties": {
                "verdict": {
                    "type": "string",
                    "enum": ["pass", "needs_review", "fail"]
                },
                "findings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "category": {
                                "type": "string",
                                "enum": ["citation", "reproducibility", "completeness"]
                            },
                            "severity": {
                                "type": "string",
                                "enum": ["info", "minor", "major", "blocker"]
                            },
                            "message": { "type": "string" }
                        },
                        "required": ["category", "severity", "message"]
                    }
                }
            },
            "required": ["verdict", "findings"]
        }),
        schema_name: "research_review_findings".to_owned(),
        schema_description: Some(
            "Independent review findings against the citation and \
             reproducibility rubric."
                .to_owned(),
        ),
        mode: StructuredMode::Auto,
        max_repair_attempts: 2,
    }
}

/// Extract plain assistant text from a captured evidence envelope.
fn plain_text(event: &a3s_code_core::EventEnvelopeV1) -> Option<String> {
    if event.event_type != "text_delta" && event.event_type != "end" {
        return None;
    }
    let text = event.payload.get("text").and_then(|value| value.as_str())?;
    Some(format!("[{}] {text}", event.event_type))
}

fn severity(value: &str) -> ResearchReviewSeverityV1 {
    match value {
        "blocker" => ResearchReviewSeverityV1::Blocker,
        "major" | "error" => ResearchReviewSeverityV1::Error,
        "minor" | "warning" => ResearchReviewSeverityV1::Warning,
        _ => ResearchReviewSeverityV1::Info,
    }
}

fn category(value: &str) -> ResearchReviewCategoryV1 {
    match value {
        "reproducibility" => ResearchReviewCategoryV1::Reproducibility,
        "numeric" => ResearchReviewCategoryV1::Numeric,
        "method" => ResearchReviewCategoryV1::Method,
        "source" => ResearchReviewCategoryV1::Source,
        "completeness" | "other" => ResearchReviewCategoryV1::Other,
        _ => ResearchReviewCategoryV1::Citation,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn real_reviewer_agent_reviews_real_main_run_with_provenance_binding() {
    let started = Instant::now();
    let mut config = load_pinned_layer_c_config();
    let model = config
        .default_model
        .clone()
        .expect("pinned Layer C config must declare default_model");
    config.default_model = Some(model.clone());

    // ---- Main agent: a real analysis turn over a seeded workspace. ----
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::write(
        workspace.path().join("notes.md"),
        "Measurement log: the nightlies completed in 41s (run A) and 43s \
         (run B). No other timings were recorded.\n",
    )
    .expect("seed evidence file");

    let agent = Agent::from_config(config.clone()).await.expect("agent");
    let mut permissions =
        a3s_code_core::permissions::PermissionPolicy::new().allow_all(&["read", "ls", "grep"]);
    permissions.default_decision = a3s_code_core::permissions::PermissionDecision::Deny;
    let options = SessionOptions::new()
        .with_session_id("research-review-main")
        .with_model(model.clone())
        .with_permission_policy(permissions)
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_temperature(0.0)
        .with_max_tool_rounds(4);
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("main session");

    let (mut events, worker) = session
        .stream(
            "Read notes.md, then report: what were the two measured nightly \
             completion times, and what is their average? Base every number on \
             the file.",
            None,
        )
        .await
        .expect("start main turn");
    let mut captured = Vec::new();
    let final_text = tokio::time::timeout(TURN_TIMEOUT, async {
        loop {
            match events.recv().await.expect("main stream open") {
                AgentEvent::End { text, .. } => break text,
                AgentEvent::Error { message } => panic!("main turn failed: {message}"),
                other => captured.push(other),
            }
        }
    })
    .await
    .expect("main turn timed out");
    worker.await.expect("main worker joins");
    session.close().await;
    assert!(
        final_text.contains("41") && final_text.contains("43"),
        "the main agent must ground its report in the evidence file: {final_text:?}"
    );

    // ---- Capture the main run as evidence (run store + fact journal). ----
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id(
            "run-live-review-1".to_owned(),
            "session-live-review",
            "prompt",
        )
        .await;
    let record_event = runs
        .record_event(
            &run.id,
            AgentEvent::TextDelta {
                text: final_text.clone(),
            },
        )
        .await
        .unwrap();
    let target = ExecutionTargetV1::new("session-live-review", &run.id);
    let journal = Arc::new(InMemoryExecutionFactJournal::new());
    ExecutionFactRecorder::new(journal.clone(), ExecutionFrameV1::root(target.clone()))
        .record(&RunEventRecord {
            sequence: 0,
            timestamp_ms: record_event.updated_at_ms,
            event: AgentEvent::TextDelta {
                text: final_text.clone(),
            },
        })
        .unwrap();

    let evidence = RunEvidenceReader::new(Arc::clone(&runs))
        .with_facts(journal)
        .read(EvidenceReadRequestV1::new(target.clone()))
        .await
        .unwrap();
    let evidence_digest = evidence.snapshot_digest.clone();
    assert!(
        evidence.complete,
        "a settled main turn must produce a complete evidence snapshot"
    );

    // ---- Reviewer agent: real structured review over the evidence. ----
    let llm_config = config
        .default_llm_config()
        .expect("default llm config for the reviewer");
    let reviewer_client = create_client_with_config(llm_config);
    let executor = StructuredAuxiliaryExecutor::new(reviewer_client, review_request);
    let service = InMemoryAuxiliaryRunService::new(Arc::new(executor));
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(target.clone()),
        "research-review",
        "review the main run's evidence for citation and reproducibility",
        evidence_digest.clone(),
    )
    .with_id("aux-live-review-1")
    .with_mode(AuxiliaryModeV1::Gate)
    .with_capabilities(AuxiliaryCapabilityProfileV1::tool_free());
    let output = tokio::time::timeout(REVIEW_TIMEOUT, async {
        service
            .spawn(spec, evidence, None)
            .await
            .unwrap()
            .wait()
            .await
            .unwrap()
    })
    .await
    .expect("reviewer turn timed out");

    let verdict = output
        .value
        .get("verdict")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let raw_findings = output
        .value
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        matches!(verdict.as_str(), "pass" | "needs_review" | "fail"),
        "the reviewer must return a closed-set verdict: {verdict:?}"
    );
    assert!(
        !raw_findings.is_empty() || verdict == "pass",
        "a non-pass verdict must carry findings: {raw_findings:?}"
    );

    // ---- Provenance binding: findings → evaluation record → run batch. ----
    let result = EvaluationResultV1::new(
        "research-reviewer",
        target.clone(),
        "aux-live-review-1",
        &verdict,
        output.value.clone(),
        evidence_digest.clone(),
    )
    .unwrap();
    let record = EvaluationRecordV1::new(result, 64).unwrap();
    let store = InMemoryEvaluationResultStore::new();
    assert!(store.write(record.clone()).await.unwrap().written);

    let mut research_run = ResearchRunV1::new(
        "run-live-review-1",
        "project-live-review",
        3,
        sha_repeat('f'),
        evidence_digest.clone(),
        use_bound_capability(),
        "provider-live",
        model.as_str(),
        ResearchReproducibilityV1::Reproducible,
        Some(11),
    )
    .unwrap();
    research_run
        .transition_to(ResearchRunStatusV1::Admitted)
        .unwrap();

    let receipt = ResearchProvenanceReceiptV1::new(
        "project-live-review",
        3,
        "run-live-review-1",
        "artifact-report",
        ResearchArtifactKindV1::Report,
        sha_repeat('1'),
        vec![evidence_digest.clone()],
        sha_repeat('2'),
        sha_repeat('3'),
        sha_repeat('4'),
        "provider-live",
        Some(sha_repeat('5')),
        Some(11),
        None,
    )
    .unwrap();

    let observed_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default();
    let findings = raw_findings
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            let finding = ResearchReviewFindingV1::new(
                format!("finding-live-{index}"),
                "project-live-review",
                "run-live-review-1",
                receipt.artifact_digest.clone(),
                category(
                    raw.get("category")
                        .and_then(|v| v.as_str())
                        .unwrap_or("citation"),
                ),
                severity(
                    raw.get("severity")
                        .and_then(|v| v.as_str())
                        .unwrap_or("info"),
                ),
                raw.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unreviewed"),
                None,
                vec![evidence_digest.clone()],
                "research-reviewer",
                observed_at_ms,
            )
            .unwrap()
            .bind_evaluation_record(&record)
            .expect("the finding must bind to the evaluator record")
            .bind_provenance_receipt(&receipt)
            .expect("the finding must bind to the provenance receipt");
            finding
                .validate()
                .expect("each live finding must carry a consistent digest");
            finding
        })
        .collect::<Vec<_>>();
    // A passing review binds an empty finding set; every non-pass verdict
    // above already demanded at least one finding.
    assert!(
        verdict == "pass" || !findings.is_empty(),
        "non-pass verdict must produce bound findings"
    );

    let batch = ResearchReviewBatchV1::new_for_run(
        "batch-live-1",
        &research_run,
        &record,
        evidence_digest.clone(),
        findings,
    )
    .expect("the live review batch must bind to the run and evaluator record");
    batch
        .validate_for_run(&research_run, &record)
        .expect("the provenance chain must verify end-to-end");
    assert_eq!(batch.evidence_digest, evidence_digest);
    assert_eq!(batch.evaluation_record_digest, record.record_digest);

    println!(
        "research-review-real-llm model={} verdict={} findings={} total_ms={} \
         evidence_digest={} result=pass",
        model,
        verdict,
        raw_findings.len(),
        started.elapsed().as_millis(),
        &evidence_digest[..19],
    );
}
