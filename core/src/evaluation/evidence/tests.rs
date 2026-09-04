use super::*;
use crate::agent::AgentEvent;
use crate::evaluation::journal::{ExecutionFactKindV1, InMemoryExecutionFactJournal};
use crate::run::RunStatus;
use crate::tools::ToolArtifact;

async fn fixture() -> (Arc<InMemoryRunStore>, ExecutionTargetV1, ArtifactStore) {
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs
        .create_run_with_id("run-1".to_string(), "session-1", "secret prompt")
        .await;
    runs.record_event(
        &run.id,
        AgentEvent::ToolEnd {
            id: "tool-1".to_string(),
            name: "read".to_string(),
            args: None,
            output: "secret output".to_string(),
            exit_code: 0,
            metadata: Some(serde_json::json!({"artifact_uri": "a3s://artifact/1"})),
            error_kind: None,
        },
    )
    .await;
    runs.mark_failed(&run.id, "failure detail").await;
    let artifacts = ArtifactStore::new();
    artifacts.put(ToolArtifact {
        artifact_id: "artifact-1".to_string(),
        artifact_uri: "a3s://artifact/1".to_string(),
        tool_name: "read".to_string(),
        content: "artifact content".to_string(),
        original_bytes: 16,
        shown_bytes: 16,
    });
    (
        runs,
        ExecutionTargetV1::new("session-1", "run-1"),
        artifacts,
    )
}

#[tokio::test]
async fn artifact_projection_is_canonical_even_when_store_insertion_is_not() {
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs.create_run("session-artifacts", "prompt").await;
    for (uri, content) in [("a3s://artifact/z", "z"), ("a3s://artifact/a", "a")] {
        runs.record_event(
            &run.id,
            AgentEvent::ToolEnd {
                id: uri.to_string(),
                name: "read".to_string(),
                args: None,
                output: content.to_string(),
                exit_code: 0,
                metadata: Some(serde_json::json!({"artifact_uri": uri})),
                error_kind: None,
            },
        )
        .await;
    }
    let artifacts = ArtifactStore::new();
    // Deliberately insert in reverse lexical order. The evidence contract is
    // URI sorted regardless of the backing store's insertion order.
    for (uri, content) in [
        ("a3s://artifact/z", "z-content"),
        ("a3s://artifact/a", "a-content"),
    ] {
        artifacts.put(ToolArtifact {
            artifact_id: uri.to_string(),
            artifact_uri: uri.to_string(),
            tool_name: "read".to_string(),
            content: content.to_string(),
            original_bytes: content.len(),
            shown_bytes: content.len(),
        });
    }
    let target = ExecutionTargetV1::new("session-artifacts", &run.id);
    let snapshot = RunEvidenceReader::new(runs)
        .with_artifacts(artifacts)
        .read(EvidenceReadRequestV1::new(target))
        .await
        .expect("evidence should not depend on insertion order");
    assert_eq!(
        snapshot
            .artifacts
            .iter()
            .map(|artifact| artifact.artifact_uri.as_str())
            .collect::<Vec<_>>(),
        vec!["a3s://artifact/a", "a3s://artifact/z"]
    );
    snapshot.validate().expect("canonical snapshot");
}

#[tokio::test]
async fn digest_mode_hides_prompt_event_and_artifact_content() {
    let (runs, target, artifacts) = fixture().await;
    let facts = Arc::new(InMemoryExecutionFactJournal::new());
    let event = runs.events(&target.run_id).await.remove(0);
    facts
        .append_event(ExecutionFrameV1::root(target.clone()), &event)
        .unwrap();
    let reader = RunEvidenceReader::new(runs)
        .with_facts(facts)
        .with_artifacts(artifacts);
    let snapshot = reader
        .read(EvidenceReadRequestV1::new(target))
        .await
        .unwrap();
    assert!(snapshot.validate().is_ok());
    assert!(snapshot.state.prompt.is_none());
    assert_eq!(snapshot.state.prompt_bytes, "secret prompt".len() as u64);
    assert!(snapshot.state.prompt_digest.starts_with("sha256:"));
    assert!(!snapshot.events[0]
        .event
        .payload
        .to_string()
        .contains("secret output"));
    assert!(snapshot.artifacts[0].content.is_none());
    assert!(snapshot.complete);
}

#[tokio::test]
async fn plaintext_prompt_tampering_is_rejected() {
    let (runs, target, _) = fixture().await;
    let mut request = EvidenceReadRequestV1::new(target);
    request.include_prompt = true;
    let mut snapshot = RunEvidenceReader::new(runs).read(request).await.unwrap();
    snapshot.state.prompt = Some("secreT prompt".to_string());
    assert!(matches!(
        snapshot.validate(),
        Err(EvidenceError::DigestMismatch("state.prompt_digest"))
    ));
}

#[tokio::test]
async fn bounded_mode_returns_content_within_limits() {
    let (runs, target, artifacts) = fixture().await;
    let mut request = EvidenceReadRequestV1::new(target);
    request.content_mode = EvidenceContentModeV1::BoundedPayload;
    request.include_prompt = true;
    request.include_terminal_text = true;
    request.include_artifact_content = true;
    request.limits.max_event_bytes = 32 * 1024;
    request.limits.max_artifact_bytes = 1024;
    let snapshot = RunEvidenceReader::new(runs)
        .with_artifacts(artifacts)
        .read(request)
        .await
        .unwrap();
    assert_eq!(snapshot.state.prompt.as_deref(), Some("secret prompt"));
    assert_eq!(snapshot.state.prompt_bytes, "secret prompt".len() as u64);
    assert_eq!(snapshot.state.result_bytes, None);
    assert_eq!(
        snapshot.state.error_bytes,
        Some("failure detail".len() as u64)
    );
    assert_eq!(snapshot.state.error.as_deref(), Some("failure detail"));
    assert!(snapshot.events[0]
        .event
        .payload
        .to_string()
        .contains("secret output"));
    assert_eq!(
        snapshot.artifacts[0].content.as_deref(),
        Some("artifact content")
    );
    assert!(snapshot.complete);
}

#[tokio::test]
async fn bounded_mode_marks_oversized_payload_incomplete() {
    let (runs, target, artifacts) = fixture().await;
    let mut request = EvidenceReadRequestV1::new(target);
    request.content_mode = EvidenceContentModeV1::BoundedPayload;
    request.limits.max_event_bytes = 1;
    let snapshot = RunEvidenceReader::new(runs)
        .with_artifacts(artifacts)
        .read(request)
        .await
        .unwrap();
    assert!(!snapshot.complete);
    assert_eq!(snapshot.events.len(), 1);
    assert_eq!(snapshot.events[0].event.payload["content"], "redacted");
}

#[tokio::test]
async fn digest_only_remains_complete_when_payload_is_intentionally_redacted() {
    let (runs, target, artifacts) = fixture().await;
    let mut request = EvidenceReadRequestV1::new(target);
    request.limits.max_event_bytes = 1;
    let snapshot = RunEvidenceReader::new(runs)
        .with_artifacts(artifacts)
        .read(request)
        .await
        .unwrap();
    assert!(snapshot.complete);
    assert_eq!(snapshot.events[0].event.payload["content"], "redacted");
}

#[tokio::test]
async fn truncated_event_page_is_not_claimed_complete() {
    let runs = Arc::new(InMemoryRunStore::new());
    let run = runs.create_run("session-page", "prompt").await;
    runs.record_event(
        &run.id,
        AgentEvent::TextDelta {
            text: "first".to_string(),
        },
    )
    .await;
    runs.record_event(
        &run.id,
        AgentEvent::TextDelta {
            text: "second".to_string(),
        },
    )
    .await;
    let mut request = EvidenceReadRequestV1::new(ExecutionTargetV1::new("session-page", &run.id));
    request.limits.max_events = 1;
    let snapshot = RunEvidenceReader::new(runs).read(request).await.unwrap();
    assert!(!snapshot.complete);
    assert!(snapshot.validate().is_ok());
}

#[tokio::test]
async fn bounded_event_payload_tampering_is_rejected_before_snapshot_digest() {
    let (runs, target, artifacts) = fixture().await;
    let mut request = EvidenceReadRequestV1::new(target);
    request.content_mode = EvidenceContentModeV1::BoundedPayload;
    let mut snapshot = RunEvidenceReader::new(runs)
        .with_artifacts(artifacts)
        .read(request)
        .await
        .unwrap();
    snapshot.events[0].event.payload["output"] = Value::String("tampered".into());
    assert!(matches!(
        snapshot.validate(),
        Err(EvidenceError::DigestMismatch("payload_digest"))
    ));
}

#[tokio::test]
async fn event_metadata_binding_is_checked_even_when_digest_is_recomputed() {
    let (runs, target, _) = fixture().await;
    let mut snapshot = RunEvidenceReader::new(runs)
        .read(EvidenceReadRequestV1::new(target.clone()))
        .await
        .unwrap();
    snapshot.events[0].event.metadata.as_mut().unwrap()["run_id"] =
        Value::String("another-run".into());
    snapshot.events[0].payload_digest = digest_bytes(
        "a3s.code.evidence.event-payload.v1",
        &serde_json::to_vec(&snapshot.events[0].event).unwrap(),
    );
    snapshot.snapshot_digest = snapshot.expected_digest().unwrap();
    assert!(matches!(
        snapshot.validate(),
        Err(EvidenceError::TargetMismatch)
    ));
}

#[tokio::test]
async fn recomputed_event_digest_cannot_break_fact_payload_binding() {
    let (runs, target, artifacts) = fixture().await;
    let facts = Arc::new(InMemoryExecutionFactJournal::new());
    let event = runs.events(&target.run_id).await.remove(0);
    facts
        .append_event(ExecutionFrameV1::root(target.clone()), &event)
        .unwrap();
    let mut request = EvidenceReadRequestV1::new(target);
    request.content_mode = EvidenceContentModeV1::BoundedPayload;
    let mut snapshot = RunEvidenceReader::new(runs)
        .with_facts(facts)
        .with_artifacts(artifacts)
        .read(request)
        .await
        .unwrap();
    snapshot.events[0].event.payload["output"] = Value::String("forged".into());
    let encoded = serde_json::to_vec(&snapshot.events[0].event).unwrap();
    snapshot.events[0].payload_digest =
        digest_bytes("a3s.code.evidence.event-payload.v1", &encoded);
    snapshot.events[0].payload_bytes = encoded.len() as u64;
    snapshot.snapshot_digest = snapshot.expected_digest().unwrap();
    let validation = snapshot.validate();
    assert!(matches!(
        validation,
        Err(EvidenceError::InvalidField("facts.events"))
    ));
}

#[tokio::test]
async fn fact_and_event_cross_binding_cannot_be_claimed_complete() {
    let (runs, target, _) = fixture().await;
    let facts = Arc::new(InMemoryExecutionFactJournal::new());
    let event = runs.events(&target.run_id).await.remove(0);
    let mut fact =
        ExecutionFactV1::from_run_event(ExecutionFrameV1::root(target.clone()), &event).unwrap();
    fact.event_type = "different_event".into();
    fact.kind = ExecutionFactKindV1::Other;
    fact.fact_digest = fact.expected_digest().unwrap();
    facts.append(fact).unwrap();
    let reader = RunEvidenceReader::new(runs).with_facts(facts);
    let snapshot = reader
        .read(EvidenceReadRequestV1::new(target))
        .await
        .expect("mismatched generations remain inspectable");
    assert!(!snapshot.complete);
    snapshot.validate().expect("incomplete snapshot is valid");
}

#[test]
fn limits_reject_zero_event_budget() {
    let mut request = EvidenceReadRequestV1::new(ExecutionTargetV1::new("s", "r"));
    request.limits.max_events = 0;
    assert!(matches!(
        request.validate(),
        Err(EvidenceError::InvalidLimit)
    ));
}

#[test]
fn state_projection_does_not_serialize_result_plaintext() {
    let snapshot = RunSnapshot {
        id: "r".into(),
        session_id: "s".into(),
        status: RunStatus::Completed,
        prompt: "prompt".into(),
        cognitive_package_binding: None,
        capability_binding: None,
        created_at_ms: 1,
        updated_at_ms: 2,
        result_text: Some("result secret".into()),
        error: None,
        event_count: 0,
        workspace_change_set: None,
    };
    let projected = EvidenceRunStateV1::from_snapshot(
        &snapshot,
        &ExecutionTargetV1::new("s", "r"),
        EvidenceLimitsV1::default(),
        false,
        false,
    )
    .unwrap();
    let encoded = serde_json::to_string(&projected).unwrap();
    assert!(!encoded.contains("result secret"));
}

#[test]
fn oversized_optional_text_is_redacted_without_failing_the_snapshot() {
    let snapshot = RunSnapshot {
        id: "r".into(),
        session_id: "s".into(),
        status: RunStatus::Failed,
        prompt: "long prompt".into(),
        cognitive_package_binding: None,
        capability_binding: None,
        created_at_ms: 1,
        updated_at_ms: 2,
        result_text: None,
        error: Some("long error".into()),
        event_count: 0,
        workspace_change_set: None,
    };
    let limits = EvidenceLimitsV1 {
        max_prompt_bytes: 1,
        max_result_bytes: 1,
        ..EvidenceLimitsV1::default()
    };
    let projected = EvidenceRunStateV1::from_snapshot(
        &snapshot,
        &ExecutionTargetV1::new("s", "r"),
        limits,
        true,
        true,
    )
    .unwrap();
    assert!(projected.prompt.is_none());
    assert!(projected.error.is_none());
    assert_eq!(projected.prompt_bytes, "long prompt".len() as u64);
    assert_eq!(projected.error_bytes, Some("long error".len() as u64));
    assert!(state_content_truncated(&snapshot, limits, true, true));
}
