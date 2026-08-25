use a3s_code_core::llm::{LlmResponse, Message, StreamEvent, ToolDefinition};
use a3s_code_core::store::{MemorySessionStore, SessionStore};
use a3s_code_core::tools::{
    ImmutableContentAdapter, ImmutableContentAdapterBindingV1, ImmutableContentAdapterSession,
    ImmutableContentDescriptorV1, ImmutableContentReferenceV1, ImmutableContentResult, Tool,
    ToolContext, ToolOutput, ToolResultEvidenceV1, ToolResultTransformBindingV1,
    ToolResultTransformPolicyV1, MAX_OUTPUT_SIZE, TOOL_RESULT_TRANSFORM_BINDING_METADATA_KEY,
};
use a3s_code_core::{Agent, CodeConfig, LlmClient, SessionOptions};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct UnusedLlmClient;

#[async_trait]
impl LlmClient for UnusedLlmClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("the immutable-content direct Tool test does not call the model")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("the immutable-content direct Tool test does not call the model")
    }
}

struct LargeResultTool;

#[async_trait]
impl Tool for LargeResultTool {
    fn name(&self) -> &str {
        "large_result"
    }

    fn description(&self) -> &str {
        "Return a large deterministic result"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "additionalProperties": false})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::success("x".repeat(MAX_OUTPUT_SIZE + 1)))
    }
}

#[derive(Default)]
struct RecordingAdapter {
    writes: Mutex<Vec<ImmutableContentDescriptorV1>>,
}

#[async_trait]
impl ImmutableContentAdapter for RecordingAdapter {
    fn name(&self) -> &str {
        "public-api-recording-adapter"
    }

    async fn put(
        &self,
        request: &a3s_code_core::ImmutableContentWriteRequestV1<'_>,
    ) -> ImmutableContentResult<ImmutableContentReferenceV1> {
        self.writes
            .lock()
            .unwrap()
            .push(request.descriptor().clone());
        let digest = request
            .descriptor()
            .content_digest
            .strip_prefix("sha256:")
            .unwrap();
        ImmutableContentReferenceV1::new(
            request.binding(),
            request.descriptor(),
            format!("a3s+test://session-content/{digest}"),
        )
    }
}

#[tokio::test]
async fn session_wires_and_persists_the_exact_immutable_content_binding() {
    let workspace = tempfile::tempdir().unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let adapter = Arc::new(RecordingAdapter::default());
    let binding = ImmutableContentAdapterBindingV1::new(
        format!("sha256:{}", "a".repeat(64)),
        (MAX_OUTPUT_SIZE as u64) * 2,
    )
    .unwrap();
    let adapter_port: Arc<dyn ImmutableContentAdapter> = adapter.clone();
    let immutable_content =
        ImmutableContentAdapterSession::new(binding.clone(), adapter_port).unwrap();
    let store_port: Arc<dyn SessionStore> = store.clone();
    let llm: Arc<dyn LlmClient> = Arc::new(UnusedLlmClient);
    let transform_policy = ToolResultTransformPolicyV1::context_efficient();
    let options = SessionOptions::new()
        .with_session_id("immutable-content-session")
        .with_llm_client(llm)
        .with_session_store(store_port)
        .with_immutable_content_adapter(immutable_content)
        .with_tool_result_transform_policy(transform_policy.clone());
    let agent = Agent::from_config(CodeConfig::default()).await.unwrap();
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .unwrap();
    session
        .register_dynamic_tool(Arc::new(LargeResultTool))
        .unwrap();

    let result = session
        .tool("large_result", serde_json::json!({}))
        .await
        .unwrap();
    let metadata = result.metadata.unwrap();
    let evidence: ToolResultEvidenceV1 =
        serde_json::from_value(metadata["a3s_tool_result_evidence"].clone()).unwrap();
    let transform_binding: ToolResultTransformBindingV1 =
        serde_json::from_value(metadata[TOOL_RESULT_TRANSFORM_BINDING_METADATA_KEY].clone())
            .unwrap();
    let reference: ImmutableContentReferenceV1 =
        serde_json::from_value(metadata["artifact"]["content_reference"].clone()).unwrap();

    assert_eq!(adapter.writes.lock().unwrap().len(), 1);
    transform_binding
        .validate_for_policy(&transform_policy)
        .unwrap();
    assert_eq!(evidence.content_ref, reference.uri);
    assert!(result.output.contains(&reference.uri));
    assert!(!result.output.contains("a3s://tool-output/"));
    assert!(session.get_artifact(&reference.uri).is_none());

    session.save().await.unwrap();
    let snapshot = store
        .load_snapshot("immutable-content-session")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        snapshot.session.immutable_content_adapter_binding,
        Some(binding)
    );
    assert_eq!(
        snapshot.session.config.tool_result_transform_policy,
        transform_policy
    );
    assert!(snapshot.artifacts.is_empty());
    session.close().await;
}
