use a3s_code_core::evaluation::{
    digest_bytes, AuxiliaryCapabilityProfileV1, AuxiliaryExecutor, AuxiliaryRunContextV1,
    AuxiliaryRunError, AuxiliaryRunService, AuxiliaryRunSpecV1, AuxiliaryRunStateV1,
    EvaluationRecordV1, EvaluationResultSink, EvaluationResultV1, EvidenceContentModeV1,
    EvidenceReadRequestV1, ExecutionFactRecorder, ExecutionFrameV1, ExecutionTargetV1,
    InMemoryAuxiliaryRunService, InMemoryEvaluationResultStore, InMemoryExecutionFactJournal,
    RunEvidenceReader,
};
use a3s_code_core::{AgentEvent, InMemoryRunStore, RunEventRecord};
use a3s_code_core::{
    ResearchReviewCategoryV1, ResearchReviewFindingV1, ResearchReviewSeverityV1,
    ResearchReviewStatusV1,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

struct FixtureExecutor;

#[async_trait]
impl AuxiliaryExecutor for FixtureExecutor {
    async fn execute(
        &self,
        context: AuxiliaryRunContextV1,
    ) -> Result<serde_json::Value, AuxiliaryRunError> {
        Ok(serde_json::json!({
            "run": context.spec.parent.target.run_id,
            "events": context.evidence.events.len(),
        }))
    }
}

async fn prepared_run() -> (
    Arc<InMemoryRunStore>,
    ExecutionTargetV1,
    Arc<InMemoryExecutionFactJournal>,
) {
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id(
            "run-integration".to_string(),
            "session-integration",
            "private prompt",
        )
        .await;
    let event = AgentEvent::TextDelta {
        text: "private output".to_string(),
    };
    let record = runs.record_event(&run.id, event).await.unwrap();
    assert_eq!(record.event_count, 1);
    let target = ExecutionTargetV1::new("session-integration", &run.id);
    let journal = Arc::new(InMemoryExecutionFactJournal::new());
    let recorder =
        ExecutionFactRecorder::new(journal.clone(), ExecutionFrameV1::root(target.clone()));
    recorder
        .record(&RunEventRecord {
            sequence: 0,
            timestamp_ms: record.updated_at_ms,
            event: AgentEvent::TextDelta {
                text: "private output".to_string(),
            },
        })
        .unwrap();
    (runs, target, journal)
}

#[tokio::test]
async fn evidence_to_auxiliary_to_result_is_publicly_composable() {
    let (runs, target, journal) = prepared_run().await;
    let reader = RunEvidenceReader::new(Arc::clone(&runs)).with_facts(journal);
    let mut request = EvidenceReadRequestV1::new(target.clone());
    request.content_mode = EvidenceContentModeV1::BoundedPayload;
    request.include_prompt = true;
    let evidence = reader.read(request).await.unwrap();
    assert!(evidence.validate().is_ok());
    assert_eq!(evidence.state.prompt.as_deref(), Some("private prompt"));
    assert!(evidence.events[0]
        .event
        .payload
        .to_string()
        .contains("private output"));

    let service = InMemoryAuxiliaryRunService::new(Arc::new(FixtureExecutor));
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(target.clone()),
        "integration",
        "return a bounded object",
        evidence.snapshot_digest.clone(),
    )
    .with_id("aux-integration")
    .with_capabilities(AuxiliaryCapabilityProfileV1::tool_free());
    let output = service
        .spawn(spec, evidence.clone(), None)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    let result = EvaluationResultV1::new(
        "integration-evaluator",
        target.clone(),
        "aux-integration",
        "accepted-by-host",
        output.value,
        evidence.snapshot_digest,
    )
    .unwrap();
    let store = InMemoryEvaluationResultStore::new();
    let record = EvaluationRecordV1::new(result, 1).unwrap();
    assert!(store.write(record.clone()).await.unwrap().written);
    assert_eq!(store.list_for_target(&target).await, vec![record]);
}

#[tokio::test]
async fn evaluator_result_binds_to_an_open_review_finding_without_implying_approval() {
    let (runs, target, journal) = prepared_run().await;
    let evidence = RunEvidenceReader::new(Arc::clone(&runs))
        .with_facts(journal)
        .read(EvidenceReadRequestV1::new(target.clone()))
        .await
        .unwrap();
    let service = InMemoryAuxiliaryRunService::new(Arc::new(FixtureExecutor));
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(target.clone()),
        "review",
        "inspect the bounded evidence",
        evidence.snapshot_digest.clone(),
    )
    .with_id("aux-review")
    .with_capabilities(AuxiliaryCapabilityProfileV1::tool_free());
    let output = service
        .spawn(spec, evidence.clone(), None)
        .await
        .unwrap()
        .wait()
        .await
        .unwrap();
    let record = EvaluationRecordV1::new(
        EvaluationResultV1::new(
            "integration-evaluator",
            target,
            "aux-review",
            "needs_review",
            output.value,
            evidence.snapshot_digest.clone(),
        )
        .unwrap(),
        2,
    )
    .unwrap();

    let finding = ResearchReviewFindingV1::new(
        "finding-1",
        "project-1",
        "run-integration",
        digest_bytes("review-artifact", b"report").to_string(),
        ResearchReviewCategoryV1::Reproducibility,
        ResearchReviewSeverityV1::Warning,
        "The report needs an explicit environment receipt.",
        None,
        vec![evidence.snapshot_digest],
        "integration-evaluator",
        3,
    )
    .unwrap()
    .bind_evaluation_record(&record)
    .unwrap();

    assert_eq!(finding.status, ResearchReviewStatusV1::Open);
    assert_eq!(
        finding.evaluation_record_digest.as_deref(),
        Some(record.record_digest.as_str())
    );
    assert!(finding.validate().is_ok());
}

#[tokio::test]
async fn auxiliary_timeout_is_terminal_and_cancellable() {
    struct SlowExecutor;
    #[async_trait]
    impl AuxiliaryExecutor for SlowExecutor {
        async fn execute(
            &self,
            _context: AuxiliaryRunContextV1,
        ) -> Result<serde_json::Value, AuxiliaryRunError> {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok(serde_json::json!({"late": true}))
        }
    }
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs.create_run("session-timeout", "prompt").await;
    let target = ExecutionTargetV1::new("session-timeout", &run.id);
    let evidence = RunEvidenceReader::new(runs)
        .read(EvidenceReadRequestV1::new(target.clone()))
        .await
        .unwrap();
    let service = InMemoryAuxiliaryRunService::new(Arc::new(SlowExecutor));
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(target),
        "timeout",
        "sleep",
        evidence.snapshot_digest.clone(),
    )
    .with_id("aux-timeout")
    .with_timeout_ms(10);
    let handle = service.spawn(spec, evidence, None).await.unwrap();
    assert!(matches!(
        handle.wait().await,
        Err(AuxiliaryRunError::TimedOut)
    ));
    assert_eq!(handle.snapshot().await.state, AuxiliaryRunStateV1::TimedOut);
    assert!(!handle.cancel().await);
}

#[test]
fn fact_serialization_is_digest_only_and_tamper_evident() {
    let target = ExecutionTargetV1::new("session-secure", "run-secure");
    let frame = ExecutionFrameV1::root(target);
    let fact = a3s_code_core::evaluation::ExecutionFactV1::from_input(
        a3s_code_core::evaluation::ExecutionFactInputV1::from_event(
            frame,
            0,
            1,
            &AgentEvent::TextDelta {
                text: "secret payload".to_string(),
            },
        )
        .unwrap(),
    )
    .unwrap();
    let encoded = serde_json::to_string(&fact).unwrap();
    assert!(!encoded.contains("secret payload"));
    let mut tampered = fact.clone();
    tampered.payload_bytes += 1;
    assert!(tampered.validate().is_err());
}

#[tokio::test]
async fn parent_token_can_cancel_a_queued_executor() {
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs.create_run("session-cancel", "prompt").await;
    let target = ExecutionTargetV1::new("session-cancel", &run.id);
    let evidence = RunEvidenceReader::new(runs)
        .read(EvidenceReadRequestV1::new(target.clone()))
        .await
        .unwrap();
    let service = InMemoryAuxiliaryRunService::new(Arc::new(FixtureExecutor));
    let parent = CancellationToken::new();
    let spec = AuxiliaryRunSpecV1::new(
        ExecutionFrameV1::root(target),
        "cancel",
        "run",
        evidence.snapshot_digest.clone(),
    )
    .with_id("aux-parent-cancel");
    let handle = service
        .spawn(spec, evidence, Some(parent.clone()))
        .await
        .unwrap();
    parent.cancel();
    // The executor is fast, so either terminal success before cancellation or
    // a cooperative cancellation is valid; the important invariant is that
    // wait always settles and never leaves a queued handle hanging.
    let result = handle.wait().await;
    assert!(result.is_ok() || matches!(result, Err(AuxiliaryRunError::Cancelled)));
}

#[test]
fn versioned_contracts_round_trip_and_reject_unknown_fields() {
    let target = ExecutionTargetV1::new("session-wire", "run-wire");
    let encoded = serde_json::to_value(&target).unwrap();
    let decoded: ExecutionTargetV1 = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(decoded, target);

    let mut unknown = encoded.as_object().unwrap().clone();
    unknown.insert("future_field".to_string(), serde_json::json!(true));
    assert!(
        serde_json::from_value::<ExecutionTargetV1>(serde_json::Value::Object(unknown)).is_err()
    );
}
