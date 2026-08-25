use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::llm::{ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage};
use a3s_code_core::permissions::PermissionPolicy;
use a3s_code_core::store::{MemorySessionStore, SessionStore};
use a3s_code_core::{
    Agent, AgentEvent, PlanningMode, SessionCheckpointExportSink, SessionCheckpointExportV1,
    SessionOptions,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct ScriptedClient {
    responses: Arc<Mutex<Vec<LlmResponse>>>,
}

impl ScriptedClient {
    fn new(mut responses: Vec<LlmResponse>) -> Self {
        responses.reverse();
        Self {
            responses: Arc::new(Mutex::new(responses)),
        }
    }

    fn next(&self) -> anyhow::Result<LlmResponse> {
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| anyhow::anyhow!("scripted checkpoint client exhausted"))
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.next()
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[a3s_code_core::llm::ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let response = self.next()?;
        let (sender, receiver) = mpsc::channel(4);
        tokio::spawn(async move {
            let text = response.text();
            if !text.is_empty() {
                let _ = sender.send(StreamEvent::TextDelta(text)).await;
            }
            let _ = sender.send(StreamEvent::Done(response)).await;
        });
        Ok(receiver)
    }
}

#[derive(Default)]
struct RecordingExportSink {
    exports: Mutex<Vec<SessionCheckpointExportV1>>,
    fail: bool,
}

impl RecordingExportSink {
    fn failing() -> Self {
        Self {
            exports: Mutex::new(Vec::new()),
            fail: true,
        }
    }

    fn take(&self) -> Vec<SessionCheckpointExportV1> {
        std::mem::take(&mut *self.exports.lock().unwrap())
    }
}

struct BlockingExportSink {
    exports: Mutex<Vec<SessionCheckpointExportV1>>,
    entered: Semaphore,
    release: Semaphore,
}

impl BlockingExportSink {
    fn new() -> Self {
        Self {
            exports: Mutex::new(Vec::new()),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }
}

#[async_trait::async_trait]
impl SessionCheckpointExportSink for RecordingExportSink {
    async fn export_checkpoint(&self, checkpoint: SessionCheckpointExportV1) -> anyhow::Result<()> {
        self.exports.lock().unwrap().push(checkpoint);
        if self.fail {
            anyhow::bail!("deterministic export failure")
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl SessionCheckpointExportSink for BlockingExportSink {
    async fn export_checkpoint(&self, checkpoint: SessionCheckpointExportV1) -> anyhow::Result<()> {
        self.exports.lock().unwrap().push(checkpoint);
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        Ok(())
    }
}

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("fixture/static".into()),
        providers: vec![ProviderConfig {
            name: "fixture".into(),
            api_key: Some("offline".into()),
            base_url: None,
            headers: HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "static".into(),
                name: "Static".into(),
                family: "fixture".into(),
                api_key: None,
                base_url: None,
                headers: HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        ..Default::default()
    }
}

fn tool_response(tool_id: &str, file_path: &str) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::ToolUse {
                id: tool_id.into(),
                name: "read".into(),
                input: serde_json::json!({"file_path": file_path}),
            }],
            reasoning_content: None,
        },
        usage: usage(),
        stop_reason: Some("tool_use".into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn final_response() -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".into(),
            content: vec![ContentBlock::Text {
                text: "boundary export complete".into(),
            }],
            reasoning_content: None,
        },
        usage: usage(),
        stop_reason: Some("end_turn".into()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn usage() -> TokenUsage {
    TokenUsage {
        prompt_tokens: 1,
        completion_tokens: 1,
        total_tokens: 2,
        cache_read_tokens: None,
        cache_write_tokens: None,
    }
}

async fn run_with_sink(
    sink: Arc<RecordingExportSink>,
) -> (a3s_code_core::AgentSession, a3s_code_core::AgentResult) {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("evidence.txt"),
        "same-boundary evidence\n",
    )
    .unwrap();
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("live-export-session")
                    .with_llm_client(Arc::new(ScriptedClient::new(vec![
                        tool_response("read-boundary-1", "evidence.txt"),
                        final_response(),
                    ])))
                    .with_session_checkpoint_export_sink(sink)
                    .with_permission_policy(PermissionPolicy::new().allow("read(*)"))
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_continuation(false),
            ),
        )
        .await
        .unwrap();
    let result = session.send("read the evidence", None).await.unwrap();
    (session, result)
}

#[tokio::test(flavor = "multi_thread")]
async fn live_tool_boundary_exports_one_self_consistent_portable_checkpoint() {
    let sink = Arc::new(RecordingExportSink::default());
    let (session, result) = run_with_sink(Arc::clone(&sink)).await;
    assert_eq!(result.text, "boundary export complete");

    let mut exports = sink.take();
    assert_eq!(exports.len(), 1);
    let payload = exports.pop().unwrap().open().unwrap();
    let logical = payload.logical_resume.unwrap();
    assert_eq!(logical.session_id, session.id());
    assert_eq!(logical.turn, 1);
    assert!(logical.messages.iter().any(|message| {
        message
            .content
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
    }));

    let source = payload
        .snapshot
        .run_records
        .iter()
        .find(|record| record.snapshot.id == logical.run_id)
        .expect("the same snapshot must contain the source Run");
    let capability_binding = logical
        .capability_binding
        .as_ref()
        .expect("new live checkpoints bind the admitted capability generation");
    assert_eq!(capability_binding.code_catalog_generation(), 0);
    assert_eq!(
        source.snapshot.capability_binding.as_ref(),
        Some(capability_binding)
    );
    assert!(!source.snapshot.status.is_terminal());
    assert!(source.events.iter().any(|record| {
        matches!(
            &record.event,
            AgentEvent::ToolEnd { id, .. } if id == "read-boundary-1"
        )
    }));
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn successive_tool_boundaries_form_an_ordered_checkpoint_series() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("first.txt"), "first boundary\n").unwrap();
    std::fs::write(workspace.path().join("second.txt"), "second boundary\n").unwrap();
    let sink = Arc::new(RecordingExportSink::default());
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("ordered-export-session")
                    .with_llm_client(Arc::new(ScriptedClient::new(vec![
                        tool_response("read-boundary-1", "first.txt"),
                        tool_response("read-boundary-2", "second.txt"),
                        final_response(),
                    ])))
                    .with_session_checkpoint_export_sink(sink.clone())
                    .with_permission_policy(PermissionPolicy::new().allow("read(*)"))
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_continuation(false),
            ),
        )
        .await
        .unwrap();

    let result = session.send("read both files", None).await.unwrap();
    assert_eq!(result.tool_calls_count, 2);
    let exports = sink.take();
    assert_eq!(exports.len(), 2);

    let mut previous_event_count = 0;
    let mut previous_digest = None;
    for (index, export) in exports.into_iter().enumerate() {
        let digest = export.descriptor().content_digest.clone();
        assert_ne!(previous_digest.as_ref(), Some(&digest));
        previous_digest = Some(digest);
        let payload = export.open().unwrap();
        let logical = payload.logical_resume.unwrap();
        assert_eq!(logical.turn, index + 1);
        let source = payload
            .snapshot
            .run_records
            .iter()
            .find(|record| record.snapshot.id == logical.run_id)
            .unwrap();
        assert!(source.snapshot.event_count > previous_event_count);
        previous_event_count = source.snapshot.event_count;
        let expected_id = format!("read-boundary-{}", index + 1);
        assert!(source.events.iter().any(|record| {
            matches!(&record.event, AgentEvent::ToolEnd { id, .. } if id == &expected_id)
        }));
    }
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn streaming_runs_use_the_same_acknowledged_export_boundary() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(workspace.path().join("evidence.txt"), "stream boundary\n").unwrap();
    let sink = Arc::new(RecordingExportSink::default());
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("stream-export-session")
                    .with_llm_client(Arc::new(ScriptedClient::new(vec![
                        tool_response("stream-read-boundary", "evidence.txt"),
                        final_response(),
                    ])))
                    .with_session_checkpoint_export_sink(sink.clone())
                    .with_permission_policy(PermissionPolicy::new().allow("read(*)"))
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_continuation(false),
            ),
        )
        .await
        .unwrap();

    let (mut events, lifecycle) = session.stream("read the evidence", None).await.unwrap();
    let mut saw_end = false;
    while let Some(event) = events.recv().await {
        saw_end |= matches!(event, AgentEvent::End { .. });
    }
    lifecycle.await.unwrap();
    assert!(saw_end);

    let mut exports = sink.take();
    assert_eq!(exports.len(), 1);
    let payload = exports.pop().unwrap().open().unwrap();
    let logical = payload.logical_resume.unwrap();
    let source = payload
        .snapshot
        .run_records
        .iter()
        .find(|record| record.snapshot.id == logical.run_id)
        .unwrap();
    assert!(source.events.iter().any(|record| {
        matches!(
            &record.event,
            AgentEvent::ToolEnd { id, .. } if id == "stream-read-boundary"
        )
    }));
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn session_store_and_host_export_observe_the_same_live_boundary() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        workspace.path().join("evidence.txt"),
        "dual sink boundary\n",
    )
    .unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let sink = Arc::new(BlockingExportSink::new());
    let agent = Agent::from_config(offline_config()).await.unwrap();
    let session = Arc::new(
        agent
            .session_async(
                workspace.path().display().to_string(),
                Some(
                    SessionOptions::new()
                        .with_session_id("dual-sink-session")
                        .with_session_store(store.clone())
                        .with_llm_client(Arc::new(ScriptedClient::new(vec![
                            tool_response("dual-sink-read", "evidence.txt"),
                            final_response(),
                        ])))
                        .with_session_checkpoint_export_sink(sink.clone())
                        .with_permission_policy(PermissionPolicy::new().allow("read(*)"))
                        .with_planning_mode(PlanningMode::Disabled)
                        .with_continuation(false),
                ),
            )
            .await
            .unwrap(),
    );

    let inspect_boundary = async {
        tokio::time::timeout(std::time::Duration::from_secs(3), sink.entered.acquire())
            .await
            .expect("host export sink was not reached")
            .unwrap()
            .forget();

        let export = sink.exports.lock().unwrap().pop().unwrap();
        let exported = export.open().unwrap().logical_resume.unwrap();
        let stored = store
            .load_loop_checkpoint(&exported.run_id)
            .await
            .unwrap()
            .expect("the logical store write must precede host export acknowledgement");
        assert_eq!(
            serde_json::to_value(&stored).unwrap(),
            serde_json::to_value(&exported).unwrap()
        );
        sink.release.add_permits(1);
        exported
    };
    let (result, exported) =
        tokio::join!(session.send("read the evidence", None), inspect_boundary);
    let result = result.unwrap();
    assert_eq!(result.text, "boundary export complete");
    assert!(store
        .load_loop_checkpoint(&exported.run_id)
        .await
        .unwrap()
        .is_none());
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_export_failure_is_observable_but_does_not_halt_the_live_run() {
    let sink = Arc::new(RecordingExportSink::failing());
    let (session, result) = run_with_sink(Arc::clone(&sink)).await;
    assert_eq!(result.text, "boundary export complete");
    assert_eq!(sink.take().len(), 1);
    session.close().await;
}
