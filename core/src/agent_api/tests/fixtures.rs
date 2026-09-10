use super::super::*;
use crate::config::{ModelConfig, ModelModalities, ProviderConfig};
use crate::llm::{ContentBlock, LlmResponse, StreamEvent, TokenUsage};
use crate::store::SessionStore;

#[derive(Clone)]
pub(crate) struct StaticStreamingClient {
    pub(crate) text: String,
}

#[derive(Clone)]
pub(crate) struct ScriptedStreamingClient {
    pub(crate) responses: Arc<std::sync::Mutex<Vec<LlmResponse>>>,
}

pub(crate) struct NamedSessionTool(pub(crate) String);
pub(crate) struct CountingScopedWorkflowTool {
    pub(crate) calls: Arc<std::sync::atomic::AtomicUsize>,
}
pub(crate) struct NoopSessionCommand;

pub(crate) struct FailingCloseSessionTransport;

#[derive(Default)]
pub(crate) struct CountingMemoryObserver(pub(crate) std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl crate::memory::MemoryObserver for CountingMemoryObserver {
    async fn on_memory_stored(
        &self,
        _observation: crate::memory::MemoryObservation,
    ) -> anyhow::Result<()> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait::async_trait]
impl crate::tools::Tool for NamedSessionTool {
    fn name(&self) -> &str {
        &self.0
    }

    fn description(&self) -> &str {
        "test session tool"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &crate::tools::ToolContext,
    ) -> anyhow::Result<crate::tools::ToolOutput> {
        Ok(crate::tools::ToolOutput::success("ok"))
    }
}

#[async_trait::async_trait]
impl crate::tools::Tool for CountingScopedWorkflowTool {
    fn name(&self) -> &str {
        "scoped_workflow_evidence"
    }

    fn description(&self) -> &str {
        "Return host-scoped evidence to one workflow child."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {},
        })
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &crate::tools::ToolContext,
    ) -> anyhow::Result<crate::tools::ToolOutput> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(crate::tools::ToolOutput::success("scoped evidence"))
    }
}

impl crate::commands::SlashCommand for NoopSessionCommand {
    fn name(&self) -> &str {
        "late-command"
    }

    fn description(&self) -> &str {
        "test command"
    }

    fn execute(
        &self,
        _args: &str,
        _ctx: &crate::commands::CommandContext,
    ) -> crate::commands::CommandOutput {
        crate::commands::CommandOutput::text("ok")
    }
}

#[async_trait::async_trait]
impl crate::mcp::transport::McpTransport for FailingCloseSessionTransport {
    async fn request(
        &self,
        _request: crate::mcp::protocol::JsonRpcRequest,
    ) -> anyhow::Result<crate::mcp::protocol::JsonRpcResponse> {
        anyhow::bail!("request is not used by this test transport")
    }

    async fn notify(
        &self,
        _notification: crate::mcp::protocol::JsonRpcNotification,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn notifications(&self) -> tokio::sync::mpsc::Receiver<crate::mcp::McpNotification> {
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        rx
    }

    async fn close(&self) -> anyhow::Result<()> {
        anyhow::bail!("deterministic session transport close failure")
    }

    fn is_connected(&self) -> bool {
        true
    }
}

impl StaticStreamingClient {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    pub(crate) fn response(&self) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: self.text.clone(),
                }],
                reasoning_content: None,
            },
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some("end_turn".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }
}

impl ScriptedStreamingClient {
    pub(crate) fn new(mut responses: Vec<LlmResponse>) -> Self {
        responses.reverse();
        Self {
            responses: Arc::new(std::sync::Mutex::new(responses)),
        }
    }

    pub(crate) fn next_response(&self) -> anyhow::Result<LlmResponse> {
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| anyhow::anyhow!("scripted streaming client exhausted"))
    }
}

pub(crate) fn scripted_text_response(text: &str) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            reasoning_content: None,
        },
        usage: TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("end_turn".to_string()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

pub(crate) fn scripted_tool_call_response(
    tool_id: &str,
    tool_name: &str,
    args: serde_json::Value,
) -> LlmResponse {
    LlmResponse {
        message: Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: tool_id.to_string(),
                name: tool_name.to_string(),
                input: args,
            }],
            reasoning_content: None,
        },
        usage: TokenUsage {
            prompt_tokens: 1,
            completion_tokens: 1,
            total_tokens: 2,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        stop_reason: Some("tool_use".to_string()),
        token_logprobs: Vec::new(),
        meta: None,
    }
}

#[derive(Clone)]
pub(crate) struct FailingStreamingClient;

#[derive(Clone, Default)]
pub(crate) struct NonRetryableStreamingClient {
    pub(crate) streaming_calls: Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) complete_calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Clone, Default)]
pub(crate) struct SessionAdmissionClient {
    pub(crate) active: Arc<std::sync::atomic::AtomicUsize>,
    pub(crate) max_active: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Clone)]
pub(crate) struct TaskSchedulerProbeClient {
    pub(crate) started: tokio::sync::mpsc::UnboundedSender<String>,
    pub(crate) release: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone)]
pub(crate) struct CancellableStreamingClient {
    pub(crate) text: String,
}

#[derive(Debug, Default)]
pub(crate) struct RecordingRuntimeHook {
    pub(crate) events: std::sync::Mutex<Vec<(String, String, AgentEvent)>>,
    pub(crate) hook_events: std::sync::Mutex<Vec<crate::hooks::HookEvent>>,
}

#[derive(Debug)]
pub(crate) struct RewritingPromptHook;

#[derive(Debug)]
pub(crate) struct BlockingPromptHook;

#[derive(Debug, Default)]
pub(crate) struct CapturingContextProvider {
    pub(crate) session_ids: std::sync::Mutex<Vec<Option<String>>>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TestCognitiveProviderMode {
    Valid,
    DriftGeneration,
    Fail,
}

#[derive(Debug)]
pub(crate) struct TestCognitiveProvider {
    pub(crate) mode: TestCognitiveProviderMode,
    pub(crate) requests: std::sync::Mutex<Vec<crate::cognitive_context::CognitiveContextRequestV1>>,
}

impl TestCognitiveProvider {
    pub(crate) fn new(mode: TestCognitiveProviderMode) -> Self {
        Self {
            mode,
            requests: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[derive(Default)]
pub(crate) struct TestWorkspaceFs {
    pub(crate) files: std::sync::RwLock<std::collections::HashMap<String, String>>,
}

impl TestWorkspaceFs {
    pub(crate) fn insert(&self, path: &str, content: &str) {
        self.files
            .write()
            .unwrap()
            .insert(path.to_string(), content.to_string());
    }

    pub(crate) fn read_raw(&self, path: &str) -> Option<String> {
        self.files.read().unwrap().get(path).cloned()
    }
}

#[async_trait::async_trait]
impl crate::workspace::WorkspaceFileSystem for TestWorkspaceFs {
    async fn read_text(
        &self,
        path: &crate::workspace::WorkspacePath,
    ) -> crate::workspace::WorkspaceResult<String> {
        self.files
            .read()
            .unwrap()
            .get(path.as_str())
            .cloned()
            .ok_or_else(|| crate::workspace::WorkspaceError::NotFound {
                path: path.as_str().to_string(),
            })
    }

    async fn write_text(
        &self,
        path: &crate::workspace::WorkspacePath,
        content: &str,
    ) -> crate::workspace::WorkspaceResult<crate::workspace::WorkspaceWriteOutcome> {
        self.insert(path.as_str(), content);
        Ok(crate::workspace::WorkspaceWriteOutcome {
            bytes: content.len(),
            lines: content.lines().count(),
        })
    }

    async fn list_dir(
        &self,
        path: &crate::workspace::WorkspacePath,
    ) -> crate::workspace::WorkspaceResult<Vec<crate::workspace::WorkspaceDirEntry>> {
        let prefix = if path.is_root() {
            String::new()
        } else {
            format!("{}/", path.as_str())
        };
        let files = self.files.read().unwrap();
        let mut entries = Vec::new();

        for (file_path, content) in files.iter() {
            if !file_path.starts_with(&prefix) {
                continue;
            }
            let remaining = &file_path[prefix.len()..];
            if remaining.is_empty() || remaining.contains('/') {
                continue;
            }
            entries.push(crate::workspace::WorkspaceDirEntry {
                name: remaining.to_string(),
                kind: crate::workspace::WorkspaceFileType::File,
                size: content.len() as u64,
            });
        }

        Ok(entries)
    }
}

#[derive(Default)]
pub(crate) struct TestWorkspaceRunner {
    pub(crate) commands: std::sync::RwLock<Vec<String>>,
}

#[async_trait::async_trait]
impl crate::workspace::WorkspaceCommandRunner for TestWorkspaceRunner {
    async fn exec(
        &self,
        request: crate::workspace::CommandRequest,
    ) -> anyhow::Result<crate::workspace::CommandOutput> {
        self.commands.write().unwrap().push(request.command.clone());
        Ok(crate::workspace::CommandOutput {
            output: format!("session runner: {}\n", request.command),
            exit_code: 0,
            timed_out: false,
        })
    }
}

#[async_trait::async_trait]
impl crate::context::ContextProvider for CapturingContextProvider {
    fn name(&self) -> &str {
        "capturing-context"
    }

    async fn query(
        &self,
        query: &crate::context::ContextQuery,
    ) -> anyhow::Result<crate::context::ContextResult> {
        self.session_ids
            .lock()
            .unwrap()
            .push(query.session_id.clone());
        Ok(crate::context::ContextResult::new(self.name()))
    }
}

#[async_trait::async_trait]
impl crate::cognitive_context::CognitiveContextProvider for TestCognitiveProvider {
    fn name(&self) -> &str {
        "test-a3s-use-cognitive-package"
    }

    async fn query(
        &self,
        request: &crate::cognitive_context::CognitiveContextRequestV1,
    ) -> crate::cognitive_context::CognitiveContextResult<
        crate::cognitive_context::CognitiveContextResponseV1,
    > {
        self.requests.lock().unwrap().push(request.clone());
        if matches!(self.mode, TestCognitiveProviderMode::Fail) {
            return Err(crate::cognitive_context::CognitiveContextError::Provider(
                "exact generation lease is unavailable".to_string(),
            ));
        }

        let citation = crate::cognitive_context::CognitiveKnowledgeCitationV1::new(
            &request.binding,
            "concepts/retry-policy.md",
            "Retry policy",
            vec![
                "sha256:ce8af6b5fb9a69bc2de147dbb0fd8754c9e1b555a96002878560dd9f8914db73"
                    .to_string(),
            ],
        )?;
        let document = crate::cognitive_context::CognitiveContextDocumentV1::new(
            citation,
            "Retry only before an observable side effect.",
        )?;
        let mut response = crate::cognitive_context::CognitiveContextResponseV1::new(
            request,
            vec![document],
            false,
        )?;
        if matches!(self.mode, TestCognitiveProviderMode::DriftGeneration) {
            response.binding.lifecycle_generation += 1;
        }
        Ok(response)
    }
}

pub(crate) fn test_cognitive_binding() -> crate::cognitive_context::CognitivePackageBindingV1 {
    let generation_digest =
        "sha256:aa0beeb62f1b7b21bf70f21e6f0e858a1e4b720d313f0907209b5b9dad2eeb20";
    let knowledge = crate::cognitive_context::CognitiveKnowledgeBindingV1::new(
        "domain-knowledge",
        "0.2",
        "sha256:1def786da6d190b7b3ce0176e71d99ff1cac3f8c8cc7c0f8b76a893c544e7a90",
        7,
        generation_digest,
    )
    .unwrap();
    crate::cognitive_context::CognitivePackageBindingV1::new(
        "contra-sense/handbook",
        "0.1.0",
        7,
        generation_digest,
        "sha256:1e0f0a0162f5b290887ade8886af69fbba4548c863df026178e3550c77813455",
        knowledge,
        crate::cognitive_context::CognitiveContextLimits::default(),
    )
    .unwrap()
}

pub(crate) fn test_cognitive_context(
    mode: TestCognitiveProviderMode,
) -> (
    crate::cognitive_context::CognitiveContextSession,
    Arc<TestCognitiveProvider>,
) {
    let provider = Arc::new(TestCognitiveProvider::new(mode));
    let provider_port: Arc<dyn crate::cognitive_context::CognitiveContextProvider> =
        provider.clone();
    let context = crate::cognitive_context::CognitiveContextSession::new(
        test_cognitive_binding(),
        provider_port,
    )
    .unwrap();
    (context, provider)
}

#[async_trait::async_trait]
impl crate::hooks::HookExecutor for RecordingRuntimeHook {
    async fn fire(&self, event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        self.hook_events.lock().unwrap().push(event.clone());
        crate::hooks::HookResult::Continue(None)
    }

    async fn record_agent_event(&self, event: &AgentEvent, run_id: &str, session_id: &str) {
        self.events.lock().unwrap().push((
            run_id.to_string(),
            session_id.to_string(),
            event.clone(),
        ));
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookExecutor for RewritingPromptHook {
    async fn fire(&self, event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        if matches!(event, crate::hooks::HookEvent::PrePrompt(_)) {
            return crate::hooks::HookResult::continue_with(serde_json::json!({
                "prompt": "HOOK_REWRITTEN_PROMPT",
                "additionalContext": "HOOK_ADDITIONAL_CONTEXT"
            }));
        }
        crate::hooks::HookResult::Continue(None)
    }
}

#[async_trait::async_trait]
impl crate::hooks::HookExecutor for BlockingPromptHook {
    async fn fire(&self, event: &crate::hooks::HookEvent) -> crate::hooks::HookResult {
        if matches!(event, crate::hooks::HookEvent::PrePrompt(_)) {
            return crate::hooks::HookResult::block("prompt policy denied this request");
        }
        crate::hooks::HookResult::Continue(None)
    }
}

impl CancellableStreamingClient {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

#[async_trait::async_trait]
impl LlmClient for StaticStreamingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(self.response())
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (tx, rx) = mpsc::channel(8);
        let text = self.text.clone();
        let response = self.response();
        tokio::spawn(async move {
            let _ = tx.send(StreamEvent::TextDelta(text)).await;
            let _ = tx.send(StreamEvent::Done(response)).await;
        });
        Ok(rx)
    }
}

#[async_trait::async_trait]
impl LlmClient for ScriptedStreamingClient {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        if system.is_some_and(|value| value.contains(crate::prompts::PRE_ANALYSIS_SYSTEM)) {
            let prompt = messages.last().map(Message::text).unwrap_or_default();
            let response = serde_json::json!({
                "intent": "GeneralPurpose",
                "requires_planning": false,
                "goal": {
                    "description": prompt,
                    "success_criteria": []
                },
                "execution_plan": {
                    "complexity": "Simple",
                    "steps": [
                        {
                            "id": "s1",
                            "description": prompt,
                            "dependencies": [],
                            "success_criteria": "Complete the request"
                        }
                    ]
                },
                "optimized_input": prompt
            });
            return Ok(scripted_text_response(&response.to_string()));
        }
        self.next_response()
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let response = self.next_response()?;
        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            let text = response.text();
            if !text.is_empty() {
                let _ = tx.send(StreamEvent::TextDelta(text)).await;
            }
            let _ = tx.send(StreamEvent::Done(response)).await;
        });
        Ok(rx)
    }
}

#[async_trait::async_trait]
impl LlmClient for FailingStreamingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("non-streaming fallback failed")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming setup failed")
    }
}

#[async_trait::async_trait]
impl LlmClient for NonRetryableStreamingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.complete_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(crate::llm::NonRetryableLlmError::new(
            "Codex Pro usage limit reached. Quota resets in about 2h 45m.",
        )
        .into())
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.streaming_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(crate::llm::NonRetryableLlmError::new(
            "Codex Pro usage limit reached. Quota resets in about 2h 45m.",
        )
        .into())
    }
}

#[async_trait::async_trait]
impl LlmClient for SessionAdmissionClient {
    fn model_generation_pool(&self) -> Option<crate::llm::ModelGenerationPool> {
        crate::llm::ModelGenerationPool::for_endpoint(
            "test-provider",
            "test-model",
            "https://provider.test",
            crate::llm::ModelGenerationConcurrency::single_flight(),
        )
        .ok()
    }

    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let active = self
            .active
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        self.max_active
            .fetch_max(active, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        self.active
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        Ok(scripted_text_response(r#"{"ok":true}"#))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("session admission test uses blocking structured generation")
    }
}

#[async_trait::async_trait]
impl LlmClient for TaskSchedulerProbeClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let prompt = messages.last().map(Message::text).unwrap_or_default();
        self.started.send(prompt.clone()).unwrap();
        self.release.acquire().await.unwrap().forget();
        Ok(scripted_text_response(&format!("completed: {prompt}")))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        _cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("task scheduler probe uses blocking completion")
    }
}

#[async_trait::async_trait]
impl LlmClient for CancellableStreamingClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("cancellable client does not support fallback completion")
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (tx, rx) = mpsc::channel(8);
        let text = self.text.clone();
        tokio::spawn(async move {
            let _ = tx.send(StreamEvent::TextDelta(text)).await;
            cancel_token.cancelled().await;
        });
        Ok(rx)
    }
}

pub(crate) struct BlockingLoadSessionStore {
    pub(crate) inner: Arc<crate::store::MemorySessionStore>,
    pub(crate) entered: tokio::sync::Semaphore,
    pub(crate) release: tokio::sync::Semaphore,
}

impl BlockingLoadSessionStore {
    pub(crate) fn new(inner: Arc<crate::store::MemorySessionStore>) -> Self {
        Self {
            inner,
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        }
    }

    pub(crate) async fn wait_until_load_is_blocked(&self) {
        self.entered
            .acquire()
            .await
            .expect("test semaphore remains open")
            .forget();
    }

    pub(crate) fn release_one_load(&self) {
        self.release.add_permits(1);
    }
}

#[async_trait::async_trait]
impl SessionStore for BlockingLoadSessionStore {
    async fn save(&self, session: &crate::store::SessionData) -> anyhow::Result<()> {
        self.inner.save(session).await
    }

    async fn load(&self, id: &str) -> anyhow::Result<Option<crate::store::SessionData>> {
        self.inner.load(id).await
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.inner.delete(id).await
    }

    async fn list(&self) -> anyhow::Result<Vec<String>> {
        self.inner.list().await
    }

    async fn exists(&self, id: &str) -> anyhow::Result<bool> {
        self.inner.exists(id).await
    }

    async fn save_snapshot(
        &self,
        snapshot: &crate::store::SessionSnapshotV1,
    ) -> anyhow::Result<()> {
        self.inner.save_snapshot(snapshot).await
    }

    async fn load_snapshot(
        &self,
        id: &str,
    ) -> anyhow::Result<Option<crate::store::SessionSnapshotV1>> {
        self.entered.add_permits(1);
        self.release
            .acquire()
            .await
            .expect("test semaphore remains open")
            .forget();
        self.inner.load_snapshot(id).await
    }

    fn capabilities(&self) -> crate::store::SessionStoreCapabilities {
        self.inner.capabilities()
    }
}

pub(crate) fn test_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        providers: vec![
            ProviderConfig {
                name: "anthropic".to_string(),
                api_key: Some("test-key".to_string()),
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                models: vec![ModelConfig {
                    id: "claude-sonnet-4-20250514".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                    family: "claude-sonnet".to_string(),
                    api_key: None,
                    base_url: None,
                    headers: std::collections::HashMap::new(),
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
            },
            ProviderConfig {
                name: "openai".to_string(),
                api_key: Some("test-openai-key".to_string()),
                base_url: None,
                headers: std::collections::HashMap::new(),
                session_id_header: None,
                models: vec![ModelConfig {
                    id: "gpt-4o".to_string(),
                    name: "GPT-4o".to_string(),
                    family: "gpt-4".to_string(),
                    api_key: None,
                    base_url: None,
                    headers: std::collections::HashMap::new(),
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
            },
        ],
        ..Default::default()
    }
}

pub(crate) fn assert_session_busy<T>(result: crate::error::Result<T>, expected_session_id: &str) {
    match result {
        Err(crate::error::CodeError::SessionBusy { session_id }) => {
            assert_eq!(session_id, expected_session_id);
        }
        Ok(_) => panic!("expected SessionBusy, operation was admitted"),
        Err(other) => panic!("expected SessionBusy, got {other:?}"),
    }
}

pub(crate) fn build_effective_registry_for_test(
    agent_registry: Option<Arc<crate::skills::SkillRegistry>>,
    opts: &SessionOptions,
) -> Arc<crate::skills::SkillRegistry> {
    super::super::capabilities::build_effective_skill_registry(agent_registry.as_deref(), opts)
}

/// Custom BudgetGuard that denies the first LLM call — used to verify
/// that the framework consults the guard and bails before touching
/// the LLM client. Records whether `check_before_llm` was called.
#[derive(Debug, Default)]
pub(crate) struct DenyingBudgetGuard {
    pub(crate) checks: std::sync::atomic::AtomicUsize,
    pub(crate) llm_records: std::sync::atomic::AtomicUsize,
}

#[derive(Debug, Default)]
pub(crate) struct DenyingToolBudgetGuard {
    pub(crate) checks: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl crate::budget::BudgetGuard for DenyingBudgetGuard {
    async fn check_before_llm(
        &self,
        _session_id: &str,
        _est_tokens: usize,
    ) -> crate::budget::BudgetDecision {
        self.checks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::budget::BudgetDecision::Deny {
            resource: "llm_tokens".to_string(),
            reason: "test cap exceeded".to_string(),
        }
    }

    async fn record_after_llm(&self, _session_id: &str, _usage: &crate::llm::TokenUsage) {
        self.llm_records
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl crate::budget::BudgetGuard for DenyingToolBudgetGuard {
    async fn check_before_tool(
        &self,
        _session_id: &str,
        tool_name: &str,
    ) -> crate::budget::BudgetDecision {
        self.checks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        crate::budget::BudgetDecision::Deny {
            resource: "tool_calls".to_string(),
            reason: format!("test budget denied {tool_name}"),
        }
    }
}
