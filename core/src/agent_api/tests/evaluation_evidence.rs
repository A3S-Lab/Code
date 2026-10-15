#![cfg(test)]

use super::*;
use crate::evaluation::{
    EvidenceContentModeV1, EvidenceError, EvidenceReadRequestV1, ExecutionTargetV1,
    RunEvidenceReader,
};
use crate::store::MemorySessionStore;
use crate::tools::{Tool, ToolArtifact, ToolContext, ToolOutput};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
struct EvidenceExecutionProbe {
    model_calls: AtomicUsize,
    tool_calls: AtomicUsize,
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for EvidenceExecutionProbe {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.model_calls.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("an evidence read must never invoke a model")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        self.model_calls.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("an evidence read must never invoke a streaming model")
    }
}

#[async_trait::async_trait]
impl Tool for EvidenceExecutionProbe {
    fn name(&self) -> &str {
        "evidence_execution_probe"
    }

    fn description(&self) -> &str {
        "Fail if an evidence read attempts execution."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object", "properties":{}, "additionalProperties":false})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _context: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        self.tool_calls.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("an evidence read must never execute a tool")
    }
}

struct Fixture {
    workspace: tempfile::TempDir,
    session: AgentSession,
    probe: Arc<EvidenceExecutionProbe>,
    store: Arc<MemorySessionStore>,
    target: ExecutionTargetV1,
}

async fn fixture(retention: Option<crate::retention::SessionRetentionLimits>) -> Fixture {
    let workspace = tempfile::tempdir().unwrap();
    let mut config = test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();
    let probe = Arc::new(EvidenceExecutionProbe::default());
    let store = Arc::new(MemorySessionStore::new());
    let mut options = SessionOptions::new()
        .with_session_id("evidence-owner")
        .with_session_store(store.clone())
        .with_planning_mode(crate::prompts::PlanningMode::Disabled);
    if let Some(retention) = retention {
        options = options.with_retention_limits(retention);
    }
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().into_owned(),
            probe.clone(),
            &options,
        )
        .unwrap();
    session.register_dynamic_tool(probe.clone()).unwrap();
    let run = session
        .run_store
        .create_run_with_id(
            "maker-original".into(),
            session.session_id(),
            "original maker prompt",
        )
        .await;
    session
        .run_store
        .record_event(
            &run.id,
            AgentEvent::ToolEnd {
                id: "original-read".into(),
                name: "read".into(),
                args: None,
                output: "original tool output".into(),
                exit_code: 0,
                metadata: Some(serde_json::json!({"artifact_uri":"a3s://artifact/original"})),
                error_kind: None,
            },
        )
        .await;
    session
        .run_store
        .record_event(
            &run.id,
            AgentEvent::TextDelta {
                text: "original text".into(),
            },
        )
        .await;
    session
        .run_store
        .mark_failed(&run.id, "original terminal failure")
        .await;
    session.tool_executor.artifact_store().put(ToolArtifact {
        artifact_id: "original".into(),
        artifact_uri: "a3s://artifact/original".into(),
        tool_name: "read".into(),
        content: "original artifact bytes".into(),
        original_bytes: 23,
        shown_bytes: 23,
    });
    Fixture {
        workspace,
        target: ExecutionTargetV1::new(session.session_id(), &run.id),
        session,
        probe,
        store,
    }
}

fn request(fixture: &Fixture) -> EvidenceReadRequestV1 {
    EvidenceReadRequestV1::new(fixture.target.clone())
}

fn assert_no_execution(fixture: &Fixture) {
    assert_eq!(fixture.probe.model_calls.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.probe.tool_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn foreign_and_unknown_evidence_targets_share_not_found_without_reading_foreign_data() {
    let fixture = fixture(None).await;
    // Include a foreign record in the underlying store to exercise both the
    // facade's session binding and the reader's source binding independently.
    fixture
        .session
        .run_store
        .create_run_with_id(
            "foreign-known".into(),
            "foreign-session",
            "private foreign prompt",
        )
        .await;
    let before = serde_json::to_value(fixture.session.run_store.records().await).unwrap();
    for target in [
        ExecutionTargetV1::new("foreign-session", "foreign-known"),
        ExecutionTargetV1::new("foreign-session", &fixture.target.run_id),
        ExecutionTargetV1::new(fixture.session.session_id(), "foreign-known"),
        ExecutionTargetV1::new(fixture.session.session_id(), "unknown-run"),
    ] {
        assert!(matches!(
            fixture
                .session
                .read_evaluation_evidence(EvidenceReadRequestV1::new(target))
                .await,
            Err(EvidenceError::RunNotFound)
        ));
    }
    let mut invalid = request(&fixture);
    invalid.limits.max_events = 0;
    assert!(matches!(
        fixture.session.read_evaluation_evidence(invalid).await,
        Err(EvidenceError::InvalidLimit)
    ));
    assert_eq!(
        serde_json::to_value(fixture.session.run_store.records().await).unwrap(),
        before
    );
    assert_no_execution(&fixture);
}

#[tokio::test]
async fn facade_preserves_the_original_generation_and_artifact_store_projection() {
    let fixture = fixture(None).await;
    let mut request = request(&fixture);
    request.content_mode = EvidenceContentModeV1::BoundedPayload;
    request.include_prompt = true;
    request.include_terminal_text = true;
    request.include_artifact_content = true;
    let direct = RunEvidenceReader::new(Arc::clone(&fixture.session.run_store))
        .with_artifacts(fixture.session.tool_executor.artifact_store())
        .read(request.clone())
        .await
        .unwrap();
    let observed = fixture
        .session
        .read_evaluation_evidence(request.clone())
        .await
        .unwrap();
    assert_eq!(observed, direct);
    assert!(observed.complete && !observed.retention_gap);
    assert_eq!(
        observed.state.event_count,
        observed.latest_sequence_exclusive
    );
    assert_eq!(
        observed.state.prompt.as_deref(),
        Some("original maker prompt")
    );
    assert_eq!(
        observed.state.error.as_deref(),
        Some("original terminal failure")
    );
    assert_eq!(
        observed.artifacts[0].content.as_deref(),
        Some("original artifact bytes")
    );
    assert_eq!(
        observed
            .facts
            .iter()
            .map(|fact| fact.sequence)
            .collect::<Vec<_>>(),
        observed
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>()
    );
    // A later write belongs to a new observation; the first returned value
    // retains the exact generation and digest it originally read.
    fixture
        .session
        .run_store
        .record_event(
            &fixture.target.run_id,
            AgentEvent::TextDelta {
                text: "later generation".into(),
            },
        )
        .await;
    let later = fixture
        .session
        .read_evaluation_evidence(request)
        .await
        .unwrap();
    assert_eq!(later.state.event_count, observed.state.event_count + 1);
    assert_ne!(later.snapshot_digest, observed.snapshot_digest);
    observed.validate().unwrap();
    later.validate().unwrap();
    assert_no_execution(&fixture);
}

#[tokio::test]
async fn evidence_event_and_requested_content_budgets_remain_incomplete() {
    let fixture = fixture(None).await;
    let mut page = request(&fixture);
    page.limits.max_events = 1;
    let page = fixture
        .session
        .read_evaluation_evidence(page)
        .await
        .unwrap();
    assert!(!page.complete && !page.retention_gap);
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.latest_sequence_exclusive, 2);
    let mut bounded = request(&fixture);
    bounded.content_mode = EvidenceContentModeV1::BoundedPayload;
    bounded.limits.max_event_bytes = 1;
    let bounded = fixture
        .session
        .read_evaluation_evidence(bounded)
        .await
        .unwrap();
    assert!(!bounded.complete);
    assert!(bounded
        .events
        .iter()
        .all(|event| event.event.payload["content"] == "redacted"));
    let mut digest = request(&fixture);
    digest.limits.max_event_bytes = 1;
    let digest = fixture
        .session
        .read_evaluation_evidence(digest)
        .await
        .unwrap();
    assert!(
        digest.complete,
        "intentional digest-only payload redaction preserves requested-window completeness"
    );
    assert!(digest.state.prompt.is_none() && digest.state.error.is_none());
    assert!(digest.artifacts[0].content.is_none());
    let mut content = request(&fixture);
    content.include_prompt = true;
    content.include_terminal_text = true;
    content.limits.max_prompt_bytes = 1;
    content.limits.max_result_bytes = 1;
    let content = fixture
        .session
        .read_evaluation_evidence(content)
        .await
        .unwrap();
    assert!(!content.complete);
    assert!(content.state.prompt.is_none() && content.state.error.is_none());
    for snapshot in [page, bounded, digest, content] {
        snapshot.validate().unwrap();
    }
    assert_no_execution(&fixture);
}

#[tokio::test]
async fn requested_artifact_content_and_missing_references_never_become_complete() {
    let fixture = fixture(None).await;
    let mut content = request(&fixture);
    content.include_artifact_content = true;
    content.limits.max_artifact_bytes = 1;
    let content = fixture
        .session
        .read_evaluation_evidence(content)
        .await
        .unwrap();
    assert!(!content.complete);
    assert_eq!(content.artifacts.len(), 1);
    assert!(content.artifacts[0].content.is_none());
    let mut count = request(&fixture);
    count.limits.max_artifacts = 0;
    let count = fixture
        .session
        .read_evaluation_evidence(count)
        .await
        .unwrap();
    assert!(!count.complete && count.artifacts.is_empty());
    fixture
        .session
        .run_store
        .record_event(
            &fixture.target.run_id,
            AgentEvent::ToolEnd {
                id: "missing-artifact".into(),
                name: "read".into(),
                args: None,
                output: "missing referenced bytes".into(),
                exit_code: 0,
                metadata: Some(serde_json::json!({"artifact_uri":"a3s://artifact/missing"})),
                error_kind: None,
            },
        )
        .await;
    let missing = fixture
        .session
        .read_evaluation_evidence(request(&fixture))
        .await
        .unwrap();
    assert!(!missing.complete);
    assert_eq!(missing.artifacts.len(), 1);
    for snapshot in [content, count, missing] {
        snapshot.validate().unwrap();
    }
    assert_no_execution(&fixture);
}

#[tokio::test]
async fn original_retention_gaps_and_eviction_are_not_reconstructed() {
    let fixture = fixture(Some(
        crate::retention::SessionRetentionLimits::default()
            .with_max_runs(1)
            .with_max_events_per_run(1),
    ))
    .await;
    let retained = fixture
        .session
        .read_evaluation_evidence(request(&fixture))
        .await
        .unwrap();
    assert!(retained.retention_gap && !retained.complete);
    assert_eq!(retained.state.event_count, 2);
    assert_eq!(retained.first_available_sequence, Some(1));
    assert_eq!(retained.events.len(), 1);
    assert_eq!(retained.events[0].sequence, 1);
    retained.validate().unwrap();
    // A complete suffix is explicitly a suffix. The facade must preserve its
    // cursor; host acceptance separately requires the whole original window.
    let mut suffix = request(&fixture);
    suffix.after_sequence = Some(0);
    let suffix = fixture
        .session
        .read_evaluation_evidence(suffix)
        .await
        .unwrap();
    assert_eq!(suffix.after_sequence, Some(0));
    assert!(suffix.complete && !suffix.retention_gap);
    fixture
        .session
        .run_store
        .create_run_with_id(
            "evict-original".into(),
            fixture.session.session_id(),
            "new retained run",
        )
        .await;
    assert!(matches!(
        fixture
            .session
            .read_evaluation_evidence(request(&fixture))
            .await,
        Err(EvidenceError::RunNotFound)
    ));
    assert_no_execution(&fixture);
}

fn workspace_state(root: &std::path::Path) -> Vec<(std::path::PathBuf, Option<Vec<u8>>)> {
    fn visit(
        root: &std::path::Path,
        path: &std::path::Path,
        state: &mut Vec<(std::path::PathBuf, Option<Vec<u8>>)>,
    ) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if entry.file_type().unwrap().is_dir() {
                state.push((relative, None));
                visit(root, &path, state);
            } else {
                state.push((relative, Some(std::fs::read(&path).unwrap())));
            }
        }
    }
    let mut state = Vec::new();
    visit(root, root, &mut state);
    state.sort_by(|left, right| left.0.cmp(&right.0));
    state
}

#[tokio::test]
async fn repeated_evidence_reads_create_no_run_execution_or_store_write_even_after_close() {
    let fixture = fixture(None).await;
    fixture.session.save().await.unwrap();
    fixture.session.close().await;
    let records = serde_json::to_value(fixture.session.run_store.records().await).unwrap();
    let persisted = serde_json::to_value(
        fixture
            .store
            .load_snapshot(fixture.session.session_id())
            .await
            .unwrap(),
    )
    .unwrap();
    let artifacts = fixture.session.tool_executor.artifact_store().artifacts();
    let workspace = workspace_state(fixture.workspace.path());
    let catalog = fixture.session.capability_catalog_stamp();
    for _ in 0..3 {
        let observed = fixture
            .session
            .read_evaluation_evidence(request(&fixture))
            .await
            .unwrap();
        observed.validate().unwrap();
    }
    assert_eq!(
        serde_json::to_value(fixture.session.run_store.records().await).unwrap(),
        records
    );
    assert_eq!(
        serde_json::to_value(
            fixture
                .store
                .load_snapshot(fixture.session.session_id())
                .await
                .unwrap()
        )
        .unwrap(),
        persisted
    );
    assert_eq!(
        fixture.session.tool_executor.artifact_store().artifacts(),
        artifacts
    );
    assert_eq!(workspace_state(fixture.workspace.path()), workspace);
    assert_eq!(fixture.session.capability_catalog_stamp(), catalog);
    assert!(fixture.session.current_run_id.lock().await.is_none());
    assert!(fixture.session.active_run_control.lock().await.is_none());
    assert!(fixture.session.cancel_token.lock().await.is_none());
    assert_no_execution(&fixture);
}
