use super::*;
use crate::capability::{
    CapabilityContribution, CapabilityDescriptor, CapabilityKind, CapabilitySet, CapabilitySource,
    CapabilityValue, CodeCatalogGeneration, SessionCapabilityBatch, Sha256Digest,
};
use crate::session_checkpoint::{
    SessionCheckpointExportSink, SessionCheckpointExportV1, SessionLogicalResumeEvidenceV1,
};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct RecordingCheckpointSink(Mutex<Vec<SessionCheckpointExportV1>>);

#[async_trait::async_trait]
impl SessionCheckpointExportSink for RecordingCheckpointSink {
    async fn export_checkpoint(&self, checkpoint: SessionCheckpointExportV1) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(checkpoint);
        Ok(())
    }
}

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
}

fn batch(generation: u64, surface: char) -> SessionCapabilityBatch {
    let source = CapabilitySource::host("prepared-recovery-race", digest('a')).unwrap();
    let descriptor = CapabilityDescriptor::new(
        &source,
        CapabilityKind::Tool,
        "generation-probe",
        "generation_probe",
        digest(surface),
        [],
    )
    .unwrap();
    let id = descriptor.id().clone();
    let set = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(generation),
        [CapabilityContribution::new(source, [descriptor]).unwrap()],
    )
    .unwrap();
    let mut batch = SessionCapabilityBatch::new(set).unwrap();
    batch
        .stage_value(
            id,
            CapabilityValue::Tool(Arc::new(NamedSessionTool("generation_probe".into()))),
        )
        .unwrap();
    batch
}

#[tokio::test]
async fn cutover_after_exact_preparation_cannot_change_the_pinned_recovery_generation() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("evidence.txt"), "generation one\n").unwrap();
    let sink = Arc::new(RecordingCheckpointSink::default());
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "read-generation-one",
            "read",
            serde_json::json!({"file_path": "evidence.txt"}),
        ),
        scripted_text_response("source complete"),
    ]));
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("prepared-capability-cutover")
                    .with_llm_client(client)
                    .with_session_checkpoint_export_sink(sink.clone())
                    .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                    .with_continuation(false),
            ),
        )
        .await
        .unwrap();
    session
        .apply_capability_batch(batch(1, 'b'), CancellationToken::new())
        .await
        .unwrap();
    session.send("capture generation one", None).await.unwrap();
    let checkpoint = sink
        .0
        .lock()
        .unwrap()
        .pop()
        .unwrap()
        .open()
        .unwrap()
        .logical_resume
        .unwrap();
    let evidence = SessionLogicalResumeEvidenceV1::from_checkpoint(&checkpoint).unwrap();
    let prepared = session
        .prepare_recovery_from_checkpoint(
            &evidence,
            &format!("sha256:{}", "d".repeat(64)),
            "prepared-cutover-target",
            checkpoint,
        )
        .await
        .unwrap();
    let ExactRecoveryPreparation::Ready(prepared) = prepared else {
        panic!("a fresh target identity cannot replay");
    };

    session
        .apply_capability_batch(batch(2, 'c'), CancellationToken::new())
        .await
        .unwrap();
    let error = match session.spawn_prepared_recovery(prepared).await {
        Err(error) => error,
        Ok(_) => panic!("the post-validation cutover must be observed before target admission"),
    };
    assert!(matches!(
        error,
        ExactRecoveryError::Checkpoint(
            crate::session_checkpoint::SessionCheckpointError::ContentDrift(_)
        )
    ));
    assert!(session
        .run_snapshot("prepared-cutover-target")
        .await
        .is_none());
    session.close().await;
}
