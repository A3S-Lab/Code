use super::*;
use crate::config::{ModelConfig, ModelModalities, ProviderConfig};
use crate::llm::{ContentBlock, LlmResponse, StreamEvent, TokenUsage};
use crate::store::SessionStore;

mod checkpoint_capability_recovery;

#[derive(Clone)]
struct StaticStreamingClient {
    text: String,
}

#[derive(Clone)]
struct ScriptedStreamingClient {
    responses: Arc<std::sync::Mutex<Vec<LlmResponse>>>,
}

struct NamedSessionTool(String);
struct CountingScopedWorkflowTool {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}
struct NoopSessionCommand;

struct FailingCloseSessionTransport;

#[derive(Default)]
struct CountingMemoryObserver(std::sync::atomic::AtomicUsize);

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
    fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }

    fn response(&self) -> LlmResponse {
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
    fn new(mut responses: Vec<LlmResponse>) -> Self {
        responses.reverse();
        Self {
            responses: Arc::new(std::sync::Mutex::new(responses)),
        }
    }

    fn next_response(&self) -> anyhow::Result<LlmResponse> {
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| anyhow::anyhow!("scripted streaming client exhausted"))
    }
}

fn scripted_text_response(text: &str) -> LlmResponse {
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

fn scripted_tool_call_response(
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
struct FailingStreamingClient;

#[derive(Clone, Default)]
struct NonRetryableStreamingClient {
    streaming_calls: Arc<std::sync::atomic::AtomicUsize>,
    complete_calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Clone, Default)]
struct SessionAdmissionClient {
    active: Arc<std::sync::atomic::AtomicUsize>,
    max_active: Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Clone)]
struct TaskSchedulerProbeClient {
    started: tokio::sync::mpsc::UnboundedSender<String>,
    release: Arc<tokio::sync::Semaphore>,
}

#[derive(Clone)]
struct CancellableStreamingClient {
    text: String,
}

#[derive(Debug, Default)]
struct RecordingRuntimeHook {
    events: std::sync::Mutex<Vec<(String, String, AgentEvent)>>,
    hook_events: std::sync::Mutex<Vec<crate::hooks::HookEvent>>,
}

#[derive(Debug)]
struct RewritingPromptHook;

#[derive(Debug)]
struct BlockingPromptHook;

#[derive(Debug, Default)]
struct CapturingContextProvider {
    session_ids: std::sync::Mutex<Vec<Option<String>>>,
}

#[derive(Debug, Clone, Copy)]
enum TestCognitiveProviderMode {
    Valid,
    DriftGeneration,
    Fail,
}

#[derive(Debug)]
struct TestCognitiveProvider {
    mode: TestCognitiveProviderMode,
    requests: std::sync::Mutex<Vec<crate::cognitive_context::CognitiveContextRequestV1>>,
}

impl TestCognitiveProvider {
    fn new(mode: TestCognitiveProviderMode) -> Self {
        Self {
            mode,
            requests: std::sync::Mutex::new(Vec::new()),
        }
    }
}

#[derive(Default)]
struct TestWorkspaceFs {
    files: std::sync::RwLock<std::collections::HashMap<String, String>>,
}

impl TestWorkspaceFs {
    fn insert(&self, path: &str, content: &str) {
        self.files
            .write()
            .unwrap()
            .insert(path.to_string(), content.to_string());
    }

    fn read_raw(&self, path: &str) -> Option<String> {
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
struct TestWorkspaceRunner {
    commands: std::sync::RwLock<Vec<String>>,
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

fn test_cognitive_binding() -> crate::cognitive_context::CognitivePackageBindingV1 {
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

fn test_cognitive_context(
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
    fn new(text: impl Into<String>) -> Self {
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

struct BlockingLoadSessionStore {
    inner: Arc<crate::store::MemorySessionStore>,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}

impl BlockingLoadSessionStore {
    fn new(inner: Arc<crate::store::MemorySessionStore>) -> Self {
        Self {
            inner,
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        }
    }

    async fn wait_until_load_is_blocked(&self) {
        self.entered
            .acquire()
            .await
            .expect("test semaphore remains open")
            .forget();
    }

    fn release_one_load(&self) {
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

pub(super) fn test_config() -> CodeConfig {
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

fn assert_session_busy<T>(result: crate::error::Result<T>, expected_session_id: &str) {
    match result {
        Err(crate::error::CodeError::SessionBusy { session_id }) => {
            assert_eq!(session_id, expected_session_id);
        }
        Ok(_) => panic!("expected SessionBusy, operation was admitted"),
        Err(other) => panic!("expected SessionBusy, got {other:?}"),
    }
}

fn build_effective_registry_for_test(
    agent_registry: Option<Arc<crate::skills::SkillRegistry>>,
    opts: &SessionOptions,
) -> Arc<crate::skills::SkillRegistry> {
    super::capabilities::build_effective_skill_registry(agent_registry.as_deref(), opts)
}

#[tokio::test]
async fn test_from_config() {
    let agent = Agent::from_config(test_config()).await;
    assert!(agent.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_scheduler_prioritizes_interactive_work_across_sessions() {
    let workspace = tempfile::tempdir().unwrap();
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let client = Arc::new(TaskSchedulerProbeClient {
        started: started_tx,
        release: Arc::clone(&release),
    });
    let mut config = test_config();
    config.task_scheduler = crate::task_scheduler::TaskSchedulerConfig {
        max_active: 1,
        aging_interval_ms: 60_000,
    };
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();

    let make_session = |id: &str, priority| {
        SessionOptions::new()
            .with_session_id(id)
            .with_llm_client(client.clone())
            .with_planning_mode(crate::prompts::PlanningMode::Disabled)
            .with_continuation(false)
            .with_task_priority(priority)
    };
    let blocker = Arc::new(
        agent
            .session_async(
                workspace.path().to_string_lossy(),
                Some(make_session(
                    "scheduler-blocker",
                    crate::task_scheduler::TaskPriority::Foreground,
                )),
            )
            .await
            .unwrap(),
    );
    let background = Arc::new(
        agent
            .session_async(
                workspace.path().to_string_lossy(),
                Some(make_session(
                    "scheduler-background",
                    crate::task_scheduler::TaskPriority::Background,
                )),
            )
            .await
            .unwrap(),
    );
    let interactive = Arc::new(
        agent
            .session_async(
                workspace.path().to_string_lossy(),
                Some(make_session(
                    "scheduler-interactive",
                    crate::task_scheduler::TaskPriority::Interactive,
                )),
            )
            .await
            .unwrap(),
    );

    let blocker_run = tokio::spawn({
        let blocker = Arc::clone(&blocker);
        async move { blocker.send("blocker", None).await }
    });
    assert_eq!(started_rx.recv().await.as_deref(), Some("blocker"));

    let background_run = tokio::spawn({
        let background = Arc::clone(&background);
        async move { background.send("background", None).await }
    });
    while agent.task_scheduler_stats().await.unwrap().pending < 1 {
        tokio::task::yield_now().await;
    }
    let interactive_run = tokio::spawn({
        let interactive = Arc::clone(&interactive);
        async move { interactive.send("interactive", None).await }
    });
    while agent.task_scheduler_stats().await.unwrap().pending < 2 {
        tokio::task::yield_now().await;
    }
    let queued = agent.task_scheduler_stats().await.unwrap();
    assert_eq!(queued.pending_by_priority.background, 1);
    assert_eq!(queued.pending_by_priority.interactive, 1);

    release.add_permits(1);
    assert_eq!(started_rx.recv().await.as_deref(), Some("interactive"));
    release.add_permits(1);
    assert_eq!(started_rx.recv().await.as_deref(), Some("background"));
    release.add_permits(1);

    blocker_run.await.unwrap().unwrap();
    interactive_run.await.unwrap().unwrap();
    background_run.await.unwrap().unwrap();
    assert_eq!(agent.task_scheduler_stats().await.unwrap().active, 0);
    agent.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_direct_tools_share_the_agent_scheduler_with_conversation_runs() {
    let workspace = tempfile::tempdir().unwrap();
    let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let client = Arc::new(TaskSchedulerProbeClient {
        started: started_tx,
        release: Arc::clone(&release),
    });
    let mut config = test_config();
    config.task_scheduler = crate::task_scheduler::TaskSchedulerConfig {
        max_active: 1,
        aging_interval_ms: 60_000,
    };
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();
    let base_options = |id: &str| {
        SessionOptions::new()
            .with_session_id(id)
            .with_llm_client(client.clone())
            .with_planning_mode(crate::prompts::PlanningMode::Disabled)
            .with_continuation(false)
    };
    let conversation = Arc::new(
        agent
            .session_async(
                workspace.path().to_string_lossy(),
                Some(base_options("direct-scheduler-conversation")),
            )
            .await
            .unwrap(),
    );
    let direct = Arc::new(
        agent
            .session_async(
                workspace.path().to_string_lossy(),
                Some(base_options("direct-scheduler-tool")),
            )
            .await
            .unwrap(),
    );
    direct
        .register_dynamic_tool(Arc::new(NamedSessionTool("scheduled-direct".to_string())))
        .unwrap();

    let conversation_run = tokio::spawn({
        let conversation = Arc::clone(&conversation);
        async move { conversation.send("hold-global-slot", None).await }
    });
    assert_eq!(started_rx.recv().await.as_deref(), Some("hold-global-slot"));
    let direct_run = tokio::spawn({
        let direct = Arc::clone(&direct);
        async move { direct.tool("scheduled-direct", serde_json::json!({})).await }
    });
    while agent.task_scheduler_stats().await.unwrap().pending < 1 {
        tokio::task::yield_now().await;
    }
    assert!(!direct_run.is_finished());

    release.add_permits(1);
    conversation_run.await.unwrap().unwrap();
    assert_eq!(direct_run.await.unwrap().unwrap().output, "ok");
    assert_eq!(agent.task_scheduler_stats().await.unwrap().active, 0);
    agent.close().await;
}

#[tokio::test]
async fn test_session_default() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async("/tmp/test-workspace", None).await;
    assert!(session.is_ok());
    let debug = format!("{:?}", session.unwrap());
    assert!(debug.contains("AgentSession"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_session_direct_generations_share_provider_admission() {
    let dir = tempfile::tempdir().unwrap();
    let client = Arc::new(SessionAdmissionClient::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(
            dir.path().to_string_lossy().to_string(),
            Some(SessionOptions::new().with_llm_client(client.clone())),
        )
        .await
        .unwrap();
    let args = serde_json::json!({
        "schema": {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "ok": {"type": "boolean"}
            },
            "required": ["ok"]
        },
        "schema_name": "session_admission",
        "prompt": "Return an object whose ok field is true.",
        "mode": "prompt",
        "max_repair_attempts": 0,
        "timeout_ms": 2_000
    });

    let (first, second) = tokio::join!(
        session.tool("generate_object", args.clone()),
        session.tool("generate_object", args)
    );
    let results = [first.unwrap(), second.unwrap()];

    assert!(results
        .iter()
        .all(|result| result.exit_code == 0 && result.output.contains(r#""ok":true"#)));
    assert_eq!(
        client.max_active.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "all host-direct loops in one session must share the provider gate"
    );
    let mut queue_waits = results
        .iter()
        .map(|result| {
            result
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.pointer("/generation_admission/queue_wait_ms"))
                .and_then(serde_json::Value::as_u64)
                .expect("generation admission metadata")
        })
        .collect::<Vec<_>>();
    queue_waits.sort_unstable();
    assert!(
        queue_waits[1] >= 80,
        "the second direct generation should wait for session capacity: {queue_waits:?}"
    );
}

#[tokio::test]
async fn test_session_uses_workspace_backend_for_direct_tools() {
    let fs = Arc::new(TestWorkspaceFs::default());
    fs.insert("app.txt", "hello from backend\n");
    let fs_backend: Arc<dyn crate::workspace::WorkspaceFileSystem> = fs.clone();
    let runner = Arc::new(TestWorkspaceRunner::default());
    let runner_backend: Arc<dyn crate::workspace::WorkspaceCommandRunner> = runner.clone();
    let services = crate::workspace::WorkspaceServices::builder(
        crate::workspace::WorkspaceRef::new("session-workspace", "session://workspace"),
        fs_backend,
    )
    .command_runner(runner_backend)
    .build();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(
            "/server/local-placeholder",
            Some(
                SessionOptions::new()
                    .with_workspace_backend(services)
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new())),
            ),
        )
        .await
        .unwrap();

    let tool_names = session.tool_names();
    assert!(tool_names.contains(&"read".to_string()));
    assert!(tool_names.contains(&"write".to_string()));
    assert!(tool_names.contains(&"ls".to_string()));
    assert!(tool_names.contains(&"bash".to_string()));
    assert!(!tool_names.contains(&"search".to_string()));
    assert!(!tool_names.contains(&"git".to_string()));

    let read = session.read_file("app.txt").await.unwrap();
    assert!(read.contains("hello from backend"));

    fs.insert("long.txt", "one\ntwo\nthree\nfour\n");
    let window = session
        .read_file_with_options(
            "long.txt",
            crate::ReadFileOptions {
                offset: Some(1),
                limit: Some(2),
            },
        )
        .await
        .unwrap();
    assert!(window.contains("two"));
    assert!(window.contains("three"));
    assert!(!window.contains("one"));
    assert!(!window.contains("four"));

    let write = session
        .write_file("created.txt", "one\ntwo\n")
        .await
        .unwrap();
    assert_eq!(write.exit_code, 0, "{}", write.output);
    assert_eq!(fs.read_raw("created.txt").as_deref(), Some("one\ntwo\n"));

    let listing = session.ls(None).await.unwrap();
    assert_eq!(listing.exit_code, 0, "{}", listing.output);
    assert!(listing.output.contains("created.txt"));

    let edit = session
        .edit_file("created.txt", "one", "uno", false)
        .await
        .unwrap();
    assert_eq!(edit.exit_code, 0, "{}", edit.output);
    assert_eq!(fs.read_raw("created.txt").as_deref(), Some("uno\ntwo\n"));

    let patch = session
        .patch_file("created.txt", "@@ -1,2 +1,2 @@\n uno\n-two\n+dos")
        .await
        .unwrap();
    assert_eq!(patch.exit_code, 0, "{}", patch.output);
    assert_eq!(fs.read_raw("created.txt").as_deref(), Some("uno\ndos\n"));

    let bash = session.bash("pwd").await.unwrap();
    assert_eq!(bash, "session runner: pwd\n");
}

#[tokio::test]
async fn test_session_routes_agents_md_through_context_provider() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("AGENTS.md"),
        "Always run focused tests before reporting completion.",
    )
    .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(temp_dir.path().display().to_string(), None)
        .await
        .unwrap();

    let agents_provider = session
        .config
        .context_providers
        .iter()
        .find(|provider| provider.name() == "agents_md")
        .expect("AGENTS.md provider should be registered");
    assert!(!session
        .config
        .prompt_slots
        .extra
        .as_deref()
        .unwrap_or_default()
        .contains("Instructions (personal + project AGENTS.md chain)"));

    let result = agents_provider
        .query(&crate::context::ContextQuery::new("complete the task"))
        .await
        .unwrap();

    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].id, "agents_md");
    assert!(result.items[0].is_required());
    assert!(result.items[0]
        .content
        .contains("Always run focused tests before reporting completion."));
    assert_eq!(result.items[0].relevance, 1.0);
}

#[tokio::test]
async fn test_session_initializes_without_legacy_agentic_tools() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let _session = agent
        .session_async("/tmp/test-workspace", None)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_session_with_model_override() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_model("openai/gpt-4o");
    let session = agent.session_async("/tmp/test-workspace", Some(opts)).await;
    assert!(session.is_ok());
}

#[tokio::test]
async fn test_session_with_invalid_model_format() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_model("gpt-4o");
    let error = agent
        .session_async("/tmp/test-workspace", Some(opts))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::SessionConfiguration { field: "model", .. }
    ));
}

#[tokio::test]
async fn test_session_with_model_not_found() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_model("openai/nonexistent");
    let error = agent
        .session_async("/tmp/test-workspace", Some(opts))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::SessionConfiguration { field: "model", .. }
    ));
}

#[tokio::test]
async fn test_session_skill_dirs_preserve_agent_registry_validator() {
    use crate::skills::validator::DefaultSkillValidator;
    use crate::skills::SkillRegistry;

    let registry = Arc::new(SkillRegistry::new());
    registry.set_validator(Arc::new(DefaultSkillValidator::default()));

    let temp_dir = tempfile::tempdir().unwrap();
    let invalid_skill = temp_dir.path().join("invalid.md");
    std::fs::write(
        &invalid_skill,
        r#"---
name: BadName
description: "invalid skill name"
kind: instruction
---
# Invalid Skill
"#,
    )
    .unwrap();

    let opts = SessionOptions::new().with_skill_dirs([temp_dir.path()]);
    let effective_registry = build_effective_registry_for_test(Some(registry), &opts);
    assert!(effective_registry.get("BadName").is_none());
}

#[tokio::test]
async fn test_session_skill_registry_overrides_agent_registry_without_polluting_parent() {
    use crate::skills::{Skill, SkillKind, SkillRegistry};

    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "shared-skill".to_string(),
        description: "agent level".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "agent content".to_string(),
        tags: vec![],
        version: None,
    }));

    let session_registry = Arc::new(SkillRegistry::new());
    session_registry.register_unchecked(Arc::new(Skill {
        name: "shared-skill".to_string(),
        description: "session level".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "session content".to_string(),
        tags: vec![],
        version: None,
    }));

    let opts = SessionOptions::new().with_skill_registry(session_registry);
    let effective_registry = build_effective_registry_for_test(Some(registry.clone()), &opts);

    assert_eq!(
        effective_registry.get("shared-skill").unwrap().content,
        "session content"
    );
    assert_eq!(
        registry.get("shared-skill").unwrap().content,
        "agent content"
    );
}

#[tokio::test]
async fn test_session_skill_dirs_override_session_registry_and_skip_invalid_entries() {
    use crate::skills::{Skill, SkillKind, SkillRegistry};

    let session_registry = Arc::new(SkillRegistry::new());
    session_registry.register_unchecked(Arc::new(Skill {
        name: "shared-skill".to_string(),
        description: "session registry".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "registry content".to_string(),
        tags: vec![],
        version: None,
    }));

    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("shared.md"),
        r#"---
name: shared-skill
description: "skill dir override"
kind: instruction
---
# Shared Skill
dir content
"#,
    )
    .unwrap();
    std::fs::write(temp_dir.path().join("README.md"), "# not a skill").unwrap();

    let opts = SessionOptions::new()
        .with_skill_registry(session_registry)
        .with_skill_dirs([temp_dir.path()]);
    let effective_registry = build_effective_registry_for_test(None, &opts);

    assert_eq!(
        effective_registry.get("shared-skill").unwrap().description,
        "skill dir override"
    );
    assert!(effective_registry.get("README").is_none());
}

#[tokio::test]
async fn test_session_specific_skills_do_not_leak_across_sessions() {
    use crate::skills::{Skill, SkillKind, SkillRegistry};

    let mut agent = Agent::from_config(test_config()).await.unwrap();
    let agent_registry = Arc::new(SkillRegistry::with_builtins());
    agent.config.skill_registry = Some(agent_registry);

    let session_registry = Arc::new(SkillRegistry::new());
    session_registry.register_unchecked(Arc::new(Skill {
        name: "session-only".to_string(),
        description: "only for first session".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "session one".to_string(),
        tags: vec![],
        version: None,
    }));

    let session_one = agent
        .session_async(
            "/tmp/test-workspace",
            Some(SessionOptions::new().with_skill_registry(session_registry)),
        )
        .await
        .unwrap();
    let session_two = agent
        .session_async("/tmp/test-workspace", None)
        .await
        .unwrap();

    assert!(session_one
        .config
        .skill_registry
        .as_ref()
        .unwrap()
        .get("session-only")
        .is_some());
    assert!(session_two
        .config
        .skill_registry
        .as_ref()
        .unwrap()
        .get("session-only")
        .is_none());
}

#[tokio::test]
async fn test_session_for_agent_applies_definition_and_keeps_skill_overrides_isolated() {
    use crate::skills::{Skill, SkillKind, SkillRegistry};
    use crate::subagent::AgentDefinition;

    let mut agent = Agent::from_config(test_config()).await.unwrap();
    agent.config.skill_registry = Some(Arc::new(SkillRegistry::with_builtins()));

    let definition = AgentDefinition::new("reviewer", "Review code")
        .with_prompt("Agent definition prompt")
        .with_max_steps(7);

    let session_registry = Arc::new(SkillRegistry::new());
    session_registry.register_unchecked(Arc::new(Skill {
        name: "agent-session-skill".to_string(),
        description: "agent session only".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "agent session content".to_string(),
        tags: vec![],
        version: None,
    }));

    let session_one = agent
        .session_for_agent_async(
            "/tmp/test-workspace",
            &definition,
            Some(SessionOptions::new().with_skill_registry(session_registry)),
        )
        .await
        .unwrap();
    let session_two = agent
        .session_for_agent_async("/tmp/test-workspace", &definition, None)
        .await
        .unwrap();

    assert_eq!(session_one.config.max_tool_rounds, 7);
    let extra = session_one.config.prompt_slots.extra.as_deref().unwrap();
    assert!(extra.contains("Agent definition prompt"));
    assert!(!extra.contains("agent-session-skill"));
    assert!(session_one
        .config
        .context_providers
        .iter()
        .any(|provider| provider.name() == "skills_catalog"));
    assert!(session_one
        .config
        .skill_registry
        .as_ref()
        .unwrap()
        .get("agent-session-skill")
        .is_some());
    assert!(session_two
        .config
        .skill_registry
        .as_ref()
        .unwrap()
        .get("agent-session-skill")
        .is_none());
}

#[tokio::test]
async fn test_session_for_agent_preserves_existing_prompt_slots_when_injecting_definition_prompt() {
    use crate::prompts::SystemPromptSlots;
    use crate::subagent::AgentDefinition;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let definition = AgentDefinition::new("planner", "Plan work")
        .with_prompt("Definition extra prompt")
        .with_max_steps(3);

    let opts = SessionOptions::new().with_prompt_slots(SystemPromptSlots {
        style: None,
        role: Some("Custom role".to_string()),
        guidelines: None,
        response_style: None,
        extra: None,
    });

    let session = agent
        .session_for_agent_async("/tmp/test-workspace", &definition, Some(opts))
        .await
        .unwrap();

    assert_eq!(
        session.config.prompt_slots.role.as_deref(),
        Some("Custom role")
    );
    assert!(session
        .config
        .prompt_slots
        .extra
        .as_deref()
        .unwrap()
        .contains("Definition extra prompt"));
    assert_eq!(session.config.max_tool_rounds, 3);
}

#[tokio::test]
async fn test_new_with_acl_string() {
    let acl = r#"
            default_model = "anthropic/claude-sonnet-4-20250514"
            providers "anthropic" {
                apiKey = "test-key"
                models "claude-sonnet-4-20250514" {
                    name = "Claude Sonnet 4"
                }
            }
        "#;
    let agent = Agent::new(acl).await;
    assert!(agent.is_ok());
}

#[tokio::test]
async fn test_create_alias_acl() {
    let acl = r#"
            default_model = "anthropic/claude-sonnet-4-20250514"
            providers "anthropic" {
                apiKey = "test-key"
                models "claude-sonnet-4-20250514" {
                    name = "Claude Sonnet 4"
                }
            }
        "#;
    let agent = Agent::create(acl).await;
    assert!(agent.is_ok());
}

#[tokio::test]
async fn test_create_and_new_produce_same_result() {
    let acl = r#"
            default_model = "anthropic/claude-sonnet-4-20250514"
            providers "anthropic" {
                apiKey = "test-key"
                models "claude-sonnet-4-20250514" {
                    name = "Claude Sonnet 4"
                }
            }
        "#;
    let agent_new = Agent::new(acl).await;
    let agent_create = Agent::create(acl).await;
    assert!(agent_new.is_ok());
    assert!(agent_create.is_ok());

    // Both should produce working sessions
    let session_new = agent_new
        .unwrap()
        .session_async("/tmp/test-ws-new", None)
        .await;
    let session_create = agent_create
        .unwrap()
        .session_async("/tmp/test-ws-create", None)
        .await;
    assert!(session_new.is_ok());
    assert!(session_create.is_ok());
}

#[tokio::test]
async fn test_new_with_existing_acl_file_uses_file_loading() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir.path().join("agent.acl");
    std::fs::write(&config_path, "providers {").unwrap();

    let err = Agent::new(config_path.display().to_string())
        .await
        .unwrap_err();
    let msg = err.to_string();

    assert!(msg.contains("Failed to load config"));
    assert!(msg.contains("agent.acl"));
    assert!(!msg.contains("Failed to parse config as ACL string"));
}

#[tokio::test]
async fn test_new_with_missing_acl_file_reports_not_found() {
    let temp_dir = tempfile::tempdir().unwrap();
    let missing_path = temp_dir.path().join("agent.acl");

    let err = Agent::new(missing_path.display().to_string())
        .await
        .unwrap_err();
    let msg = err.to_string();

    assert!(msg.contains("Config file not found"));
    assert!(msg.contains("agent.acl"));
    assert!(!msg.contains("Failed to parse config as ACL string"));
}

#[tokio::test]
async fn test_new_rejects_hcl_files() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config_path = temp_dir.path().join("agent.hcl");
    std::fs::write(&config_path, "default_model = \"openai/test\"").unwrap();

    let err = Agent::new(config_path.display().to_string())
        .await
        .unwrap_err();
    let msg = err.to_string();

    assert!(msg.contains("HCL config files are not supported in 2.0"));
    assert!(msg.contains(".acl"));
}

#[test]
fn test_from_config_defers_default_model_validation_to_session() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = CodeConfig {
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("test-key".to_string()),
            base_url: None,
            headers: std::collections::HashMap::new(),
            session_id_header: None,
            models: vec![],
        }],
        ..Default::default()
    };
    let agent = rt
        .block_on(Agent::from_config(config))
        .expect("agent bootstrap must allow a host-supplied session client");
    let workspace = tempfile::tempdir().unwrap();
    let error = rt
        .block_on(agent.session_async(workspace.path().display().to_string(), None))
        .unwrap_err();

    assert!(error.to_string().contains("default_model"), "{error:#}");
}

#[tokio::test]
async fn test_history_empty_on_new_session() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace", None)
        .await
        .unwrap();
    assert!(session.history().is_empty());
}

#[tokio::test]
async fn test_stream_updates_history_and_auto_saves() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("stream-history-test")
        .with_auto_save(true);
    let session = agent
        .build_session(
            "/tmp/test-stream-history".into(),
            Arc::new(StaticStreamingClient::new("streamed answer")),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    let mut saw_end = false;
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::End { .. }) {
            saw_end = true;
            break;
        }
    }
    handle.await.unwrap();

    assert!(saw_end);
    let history = session.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].text(), "hello");
    assert_eq!(history[1].text(), "streamed answer");

    let saved = store
        .load("stream-history-test")
        .await
        .unwrap()
        .expect("saved session");
    assert_eq!(saved.messages.len(), 2);
    assert_eq!(saved.messages[1].text(), "streamed answer");

    let run_records = store
        .load_run_records("stream-history-test")
        .await
        .unwrap()
        .expect("saved run records");
    assert_eq!(run_records.len(), 1);
    assert_eq!(
        run_records[0].snapshot.status,
        crate::run::RunStatus::Completed
    );
    assert!(run_records[0]
        .events
        .iter()
        .any(|record| matches!(record.event, AgentEvent::End { .. })));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_stream_bridges_subagent_lifecycle_events() {
    use crate::prompts::PlanningMode;
    use crate::subagent_task_tracker::SubagentStatus;

    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "call-parallel",
            "parallel_task",
            serde_json::json!({
                "tasks": [
                    {
                        "agent": "explore",
                        "description": "Find auth code",
                        "prompt": "Find the auth code."
                    },
                    {
                        "agent": "explore",
                        "description": "Find docs",
                        "prompt": "Find the docs."
                    }
                ]
            }),
        ),
        scripted_text_response("auth child result"),
        scripted_text_response("docs child result"),
        scripted_text_response("final answer"),
    ]));
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_id("stream-subagents-test")
        .with_confirmation_policy(crate::hitl::ConfirmationPolicy::default())
        .with_planning_mode(PlanningMode::Disabled);
    let session = agent
        .build_session("/tmp/test-stream-subagents".into(), client, &opts)
        .unwrap();

    let (mut rx, handle) = session.stream("fan out this work", None).await.unwrap();
    let mut subagent_starts = 0;
    let mut subagent_ends = 0;
    let mut event_index = 0usize;
    let mut last_subagent_end = None;
    let mut parent_tool_end = None;
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::SubagentStart { .. } => subagent_starts += 1,
            AgentEvent::SubagentEnd { .. } => {
                subagent_ends += 1;
                last_subagent_end = Some(event_index);
            }
            AgentEvent::ToolEnd { id, .. } if id == "call-parallel" => {
                parent_tool_end = Some(event_index);
            }
            AgentEvent::End { .. } => break,
            _ => {}
        }
        event_index += 1;
    }
    handle.await.unwrap();

    assert_eq!(subagent_starts, 2);
    assert_eq!(subagent_ends, 2);
    assert!(
        last_subagent_end.expect("foreground tasks must emit SubagentEnd")
            < parent_tool_end.expect("parallel_task must emit ToolEnd"),
        "all foreground SubagentEnd events must precede the parent ToolEnd"
    );

    let tasks = session.subagent_tasks().await;
    assert_eq!(tasks.len(), 2);
    assert!(tasks
        .iter()
        .all(|task| task.status == SubagentStatus::Completed));
}

#[tokio::test]
async fn test_stream_with_custom_history_does_not_update_session_history() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-stream-custom-history".into(),
            Arc::new(StaticStreamingClient::new("custom history answer")),
            &SessionOptions::new(),
        )
        .unwrap();
    let custom_history = vec![Message::user("custom prompt")];

    let (mut rx, handle) = session
        .stream("ignored", Some(&custom_history))
        .await
        .unwrap();
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::End { .. }) {
            break;
        }
    }
    handle.await.unwrap();

    assert!(session.history().is_empty());
}

#[tokio::test]
async fn test_stream_error_does_not_update_history_or_auto_save() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("stream-error-test")
        .with_auto_save(true);
    let session = agent
        .build_session(
            "/tmp/test-stream-error".into(),
            Arc::new(FailingStreamingClient),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    let mut saw_error = false;
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::Error { .. }) {
            saw_error = true;
            break;
        }
    }
    handle.await.unwrap();

    assert!(saw_error);
    assert!(session.history().is_empty());
    assert!(store.load("stream-error-test").await.unwrap().is_none());
}

#[tokio::test]
async fn test_non_retryable_stream_error_skips_fallback_and_circuit_retries() {
    let client = Arc::new(NonRetryableStreamingClient::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-non-retryable-stream-error".into(),
            client.clone(),
            &SessionOptions::new().with_planning_mode(PlanningMode::Disabled),
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    let mut error_message = None;
    while let Some(event) = rx.recv().await {
        if let AgentEvent::Error { message } = event {
            error_message = Some(message);
            break;
        }
    }
    handle.await.unwrap();

    assert_eq!(
        error_message.as_deref(),
        Some("Codex Pro usage limit reached. Quota resets in about 2h 45m.")
    );
    assert_eq!(
        client
            .streaming_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a non-retryable provider error must make one streaming call"
    );
    assert_eq!(
        client
            .complete_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a non-retryable provider error must not use non-streaming fallback"
    );
}

#[tokio::test]
async fn test_non_retryable_pre_analysis_stops_before_main_turn() {
    let client = Arc::new(NonRetryableStreamingClient::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-non-retryable-pre-analysis".into(),
            client.clone(),
            &SessionOptions::new().with_planning_mode(PlanningMode::Auto),
        )
        .unwrap();

    let (mut rx, handle) = session
        .stream("review this repository", None)
        .await
        .unwrap();
    let mut error_message = None;
    while let Some(event) = rx.recv().await {
        if let AgentEvent::Error { message } = event {
            error_message = Some(message);
            break;
        }
    }
    handle.await.unwrap();

    assert_eq!(
        error_message.as_deref(),
        Some("Codex Pro usage limit reached. Quota resets in about 2h 45m.")
    );
    assert_eq!(
        client
            .complete_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "pre-analysis should make exactly one provider request"
    );
    assert_eq!(
        client
            .streaming_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a terminal pre-analysis error must stop before the main turn"
    );
}

#[tokio::test]
async fn test_stream_cancel_records_interrupted_history_and_auto_saves() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("stream-cancel-test")
        .with_auto_save(true);
    let session = agent
        .build_session(
            "/tmp/test-stream-cancel".into(),
            Arc::new(CancellableStreamingClient::new("partial answer")),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    let mut saw_delta = false;
    for _ in 0..16 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("stream event before timeout")
            .expect("stream should stay open until cancelled");
        if matches!(event, AgentEvent::TextDelta { ref text } if text == "partial answer") {
            saw_delta = true;
            break;
        }
    }
    assert!(saw_delta);
    assert!(session.cancel().await);

    while rx.recv().await.is_some() {}
    handle.await.unwrap();

    let history = session.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].text(), "hello");
    assert_eq!(history[1].role, "assistant");
    assert!(history[1].text().contains("interrupted"));

    let saved = store
        .load("stream-cancel-test")
        .await
        .unwrap()
        .expect("interrupted stream should auto-save");
    assert_eq!(saved.messages.len(), 2);
    assert_eq!(saved.messages[0].text(), "hello");
    assert!(saved.messages[1].text().contains("interrupted"));
    assert!(!session.cancel().await);
}

#[tokio::test]
async fn test_stream_with_attachments_cancel_records_interrupted_history_and_auto_saves() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("stream-attachments-cancel-test")
        .with_auto_save(true);
    let session = agent
        .build_session(
            "/tmp/test-stream-attachments-cancel".into(),
            Arc::new(CancellableStreamingClient::new("partial attachment answer")),
            &opts,
        )
        .unwrap();
    let attachments = vec![crate::llm::Attachment::png(vec![1, 2, 3])];

    let (mut rx, handle) = session
        .stream_with_attachments("hello", &attachments, None)
        .await
        .unwrap();
    let mut saw_delta = false;
    for _ in 0..16 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("stream event before timeout")
            .expect("stream should stay open until cancelled");
        if matches!(event, AgentEvent::TextDelta { .. }) {
            saw_delta = true;
            break;
        }
    }
    assert!(saw_delta);
    assert!(session.cancel().await);

    while rx.recv().await.is_some() {}
    handle.await.unwrap();

    let history = session.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[0].text(), "hello");
    assert_eq!(history[1].role, "assistant");
    assert!(history[1].text().contains("interrupted"));

    let saved = store
        .load("stream-attachments-cancel-test")
        .await
        .unwrap()
        .expect("interrupted attachment stream should auto-save");
    assert_eq!(saved.messages.len(), 2);
    assert_eq!(saved.messages[0].text(), "hello");
    assert!(saved.messages[1].text().contains("interrupted"));
    assert_eq!(
        session.runs().await[0].status,
        crate::run::RunStatus::Cancelled
    );
    assert!(!session.cancel().await);
}

#[tokio::test]
async fn test_run_handle_cancels_send_with_attachments() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-send-attachments-run-handle-cancel".into(),
                Arc::new(CancellableStreamingClient::new("partial answer")),
                &SessionOptions::new(),
            )
            .unwrap(),
    );
    let worker_session = Arc::clone(&session);
    let attachments = vec![crate::llm::Attachment::png(vec![1, 2, 3])];

    let worker = tokio::spawn(async move {
        worker_session
            .send_with_attachments("hello", &attachments, None)
            .await
    });

    let mut run = None;
    for _ in 0..20 {
        if let Some(current) = session.current_run().await {
            run = Some(current);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let run = run.expect("current run should be visible");
    assert!(run.cancel().await);

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .expect("send_with_attachments should stop after cancellation")
        .expect("worker should not panic");
    let result = result.expect("cancellation should preserve interrupted history");
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].text(), "hello");
    assert!(result.messages[1].text().contains("interrupted"));
    assert_eq!(run.status().await, Some(crate::run::RunStatus::Cancelled));
    let history = session.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].text(), "hello");
    assert!(history[1].text().contains("interrupted"));
    assert!(!session.cancel().await);
}

#[tokio::test]
async fn test_cancel_run_only_cancels_matching_current_run() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-cancel-run-by-id".into(),
                Arc::new(CancellableStreamingClient::new("partial answer")),
                &SessionOptions::new(),
            )
            .unwrap(),
    );
    let worker_session = Arc::clone(&session);
    let worker = tokio::spawn(async move { worker_session.send("hello", None).await });

    let mut run_id = None;
    for _ in 0..20 {
        if let Some(current) = session.current_run().await {
            run_id = Some(current.id().to_string());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let run_id = run_id.expect("current run should be visible");

    assert!(!session.cancel_run("stale-run").await);
    assert!(session.cancel_run(&run_id).await);

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .expect("send should stop after cancellation")
        .expect("worker should not panic");
    let result = result.expect("cancellation should preserve interrupted history");
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].text(), "hello");
    assert!(result.messages[1].text().contains("interrupted"));
    assert_eq!(
        session.run_snapshot(&run_id).await.unwrap().status,
        crate::run::RunStatus::Cancelled
    );
    assert!(!session.cancel_run(&run_id).await);
}

#[tokio::test]
async fn test_is_closed_starts_false() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-close-default", None)
        .await
        .unwrap();
    assert!(!session.is_closed());
}

#[tokio::test]
async fn test_close_marks_session_closed_and_is_idempotent() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-close-idempotent", None)
        .await
        .unwrap();
    assert!(!session.is_closed());

    session.close().await;
    assert!(session.is_closed());

    session.close().await;
    assert!(session.is_closed());
}

#[tokio::test]
async fn test_send_after_close_returns_session_closed_error() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("send-after-close");
    let session = agent
        .build_session(
            "/tmp/test-send-after-close".into(),
            Arc::new(StaticStreamingClient::new("never delivered")),
            &opts,
        )
        .unwrap();

    session.close().await;
    let err = session.send("hello", None).await.unwrap_err();
    match err {
        crate::error::CodeError::SessionClosed { session_id } => {
            assert_eq!(session_id, "send-after-close");
        }
        other => panic!("expected SessionClosed, got {other:?}"),
    }
}

#[tokio::test]
async fn test_direct_tools_after_close_return_session_closed_error() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("tool-after-close");
    let session = agent
        .build_session(
            "/tmp/test-tool-after-close".into(),
            Arc::new(StaticStreamingClient::new("never delivered")),
            &opts,
        )
        .unwrap();

    session.close().await;
    let error = session
        .tool("read", serde_json::json!({ "file_path": "missing.txt" }))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::SessionClosed { ref session_id }
            if session_id == "tool-after-close"
    ));

    let (_events, handle) = session.tool_with_events("read", serde_json::json!({}));
    let error = handle.await.unwrap().unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::SessionClosed { ref session_id }
            if session_id == "tool-after-close"
    ));
}

#[tokio::test]
async fn test_immediate_capability_mutations_after_close_fail_closed() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("extensions-after-close");
    let session = agent
        .build_session(
            "/tmp/test-extensions-after-close".into(),
            Arc::new(StaticStreamingClient::new("never delivered")),
            &opts,
        )
        .unwrap();
    session.close().await;

    let dynamic_name = "late-dynamic-tool";
    let errors = [
        session
            .register_dynamic_tool(Arc::new(NamedSessionTool(dynamic_name.to_string())))
            .unwrap_err(),
        session.register_dynamic_workflow_runtime().unwrap_err(),
        session.unregister_dynamic_tool(dynamic_name).unwrap_err(),
        session
            .register_command(Arc::new(NoopSessionCommand))
            .unwrap_err(),
        session
            .register_hook(crate::hooks::Hook::new(
                "late-hook",
                crate::hooks::HookEventType::PreToolUse,
            ))
            .unwrap_err(),
        session.unregister_hook("late-hook").unwrap_err(),
        session
            .register_worker_agent(crate::subagent::WorkerAgentSpec::planner(
                "late-worker",
                "Must not register",
            ))
            .unwrap_err(),
        session
            .register_agent_dir(std::path::Path::new("/nonexistent/late-agent-dir"))
            .unwrap_err(),
        session.set_budget_guard(None).unwrap_err(),
    ];
    assert!(errors.iter().all(|error| matches!(
        error,
        crate::error::CodeError::SessionClosed { session_id }
            if session_id == "extensions-after-close"
    )));
    let lane_error = session
        .set_lane_handler(
            crate::queue::SessionLane::Execute,
            crate::queue::LaneHandlerConfig {
                mode: crate::queue::TaskHandlerMode::External,
                timeout_ms: 1_000,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        lane_error,
        crate::error::CodeError::SessionClosed { ref session_id }
            if session_id == "extensions-after-close"
    ));
    assert!(!session.tool_names().iter().any(|name| name == dynamic_name));
    assert!(!session.agent_registry.exists("late-worker"));
    assert_eq!(session.hook_count(), 0);
}

#[tokio::test]
async fn test_stream_after_close_returns_session_closed_error() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("stream-after-close");
    let session = agent
        .build_session(
            "/tmp/test-stream-after-close".into(),
            Arc::new(StaticStreamingClient::new("never delivered")),
            &opts,
        )
        .unwrap();

    session.close().await;
    let err = session.stream("hello", None).await.unwrap_err();
    assert!(matches!(
        err,
        crate::error::CodeError::SessionClosed { ref session_id }
            if session_id == "stream-after-close"
    ));
}

#[tokio::test]
async fn test_close_cancels_in_flight_send() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-close-in-flight".into(),
                Arc::new(CancellableStreamingClient::new("partial answer")),
                &SessionOptions::new(),
            )
            .unwrap(),
    );

    let worker_session = Arc::clone(&session);
    let worker = tokio::spawn(async move { worker_session.send("hello", None).await });

    let mut run_id = None;
    for _ in 0..50 {
        if let Some(current) = session.current_run().await {
            run_id = Some(current.id().to_string());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let run_id = run_id.expect("current run should be visible before close()");

    session.close().await;
    assert!(session.is_closed());

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .expect("send should stop after close")
        .expect("worker should not panic");
    let result = result.expect("close cancellation should preserve interrupted history");
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].text(), "hello");
    assert!(result.messages[1].text().contains("interrupted"));
    assert_eq!(
        session.run_snapshot(&run_id).await.unwrap().status,
        crate::run::RunStatus::Cancelled
    );
}

/// Custom BudgetGuard that denies the first LLM call — used to verify
/// that the framework consults the guard and bails before touching
/// the LLM client. Records whether `check_before_llm` was called.
#[derive(Debug, Default)]
struct DenyingBudgetGuard {
    checks: std::sync::atomic::AtomicUsize,
    llm_records: std::sync::atomic::AtomicUsize,
}

#[derive(Debug, Default)]
struct DenyingToolBudgetGuard {
    checks: std::sync::atomic::AtomicUsize,
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

#[tokio::test]
async fn test_budget_guard_deny_aborts_llm_call() {
    let guard = Arc::new(DenyingBudgetGuard::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_id("budget-deny-test")
        .with_budget_guard(guard.clone() as Arc<dyn crate::budget::BudgetGuard>);
    let session = agent
        .build_session(
            "/tmp/test-budget-deny".into(),
            Arc::new(StaticStreamingClient::new("never-delivered")),
            &opts,
        )
        .unwrap();

    let err = session.send("hello", None).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Budget exhausted") || msg.contains("llm_tokens"),
        "expected budget-exhausted error, got: {msg}"
    );
    assert_eq!(
        guard.checks.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "BudgetGuard::check_before_llm must be consulted exactly once"
    );
    assert_eq!(
        guard.llm_records.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "record_after_llm must not fire when the call was denied"
    );
    assert!(
        session.history().is_empty(),
        "denied call must not pollute conversation history"
    );
}

#[test]
fn test_cluster_agent_events_serialize_with_expected_tags() {
    // Lock the wire schema for cluster-event variants — these are
    // emitted by the host through HookExecutor and need
    // stable JSON tags so external producers can target them.
    let budget = AgentEvent::BudgetThresholdHit {
        resource: "llm_tokens".to_string(),
        kind: "soft".to_string(),
        consumed: 12000.0,
        limit: 10000.0,
        message: Some("approaching daily cap".to_string()),
    };
    let json = serde_json::to_string(&budget).unwrap();
    assert!(
        json.contains("\"type\":\"budget_threshold_hit\""),
        "got: {json}"
    );
    assert!(json.contains("\"resource\":\"llm_tokens\""), "got: {json}");

    let passivate = AgentEvent::PassivationRequested {
        reason: "node_drain".to_string(),
        deadline_ms: Some(1_700_000_000_000),
    };
    let json = serde_json::to_string(&passivate).unwrap();
    assert!(
        json.contains("\"type\":\"passivation_requested\""),
        "got: {json}"
    );

    let peer = AgentEvent::PeerInvocation {
        from_session_id: "peer-1".to_string(),
        from_tenant_id: Some("acme".to_string()),
        correlation_id: None, // omitted via skip_serializing_if
    };
    let json = serde_json::to_string(&peer).unwrap();
    assert!(json.contains("\"type\":\"peer_invocation\""), "got: {json}");
    assert!(
        !json.contains("correlation_id"),
        "None field must be skipped, got: {json}"
    );

    // Round-trip — ensures the #[serde(default)] hints don't break loading
    // from a payload that omits the optional fields.
    let minimal_peer = r#"{"type":"peer_invocation","from_session_id":"x"}"#;
    let parsed: AgentEvent = serde_json::from_str(minimal_peer).unwrap();
    assert!(
        matches!(parsed, AgentEvent::PeerInvocation { ref from_session_id, .. } if from_session_id == "x")
    );
}

#[tokio::test]
async fn test_custom_host_env_yields_deterministic_session_and_run_ids() {
    use crate::host_env::{FixedClock, HostEnv, SequentialIdGenerator};

    let env = Arc::new(HostEnv::new(
        Arc::new(SequentialIdGenerator::new("test")),
        Arc::new(FixedClock::new(1_700_000_000_000)),
    ));

    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts_a = SessionOptions::new().with_host_env(env.clone());
    let session_a = agent
        .session_async("/tmp/test-host-env-a", Some(opts_a))
        .await
        .expect("session a");

    // First call to next_id() yields "test-0" — used as session_id.
    assert_eq!(
        session_a.id(),
        "test-0",
        "session_id must come from HostEnv"
    );

    // run_id derives from next_id() too, prefixed with "run-".
    let session_a = Arc::new(session_a);
    let worker = {
        let s = Arc::clone(&session_a);
        tokio::spawn(async move {
            // Use a static streaming client by building manually so the
            // call resolves without an actual provider.
            let _ = s;
        })
    };
    let _ = worker.await;

    // Second session reuses the same generator → continues the sequence.
    let opts_b = SessionOptions::new().with_host_env(env);
    let session_b = agent
        .session_async("/tmp/test-host-env-b", Some(opts_b))
        .await
        .expect("session b");
    assert_eq!(session_b.id(), "test-1");
}

#[tokio::test]
async fn test_runtime_budget_guard_overrides_session_options_value() {
    // A guard installed via set_budget_guard() *after* construction
    // must take effect on the next send/stream — that's the entry
    // point Node SDK relies on (JsFunction can't live inside a
    // value-typed SessionOptions).
    let runtime_guard = Arc::new(DenyingBudgetGuard::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("runtime-guard-override");
    let session = agent
        .build_session(
            "/tmp/test-runtime-guard".into(),
            Arc::new(StaticStreamingClient::new("never-delivered")),
            &opts,
        )
        .unwrap();

    // No guard installed at build time -> send would succeed. Install
    // a denying guard now and assert the next send is aborted.
    session
        .set_budget_guard(Some(
            runtime_guard.clone() as Arc<dyn crate::budget::BudgetGuard>
        ))
        .unwrap();
    let err = session.send("hello", None).await.unwrap_err();
    assert!(err.to_string().contains("Budget exhausted"));
    assert_eq!(
        runtime_guard
            .checks
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    // Clearing the override should let a follow-up send succeed.
    session.set_budget_guard(None).unwrap();
    let result = session.send("hello again", None).await.unwrap();
    assert_eq!(result.text, "never-delivered");
}

#[tokio::test]
async fn test_disconnect_idle_mcp_is_safe_no_op_without_global_mcp() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    // test_config carries no mcp_servers, so global_mcp is None and
    // the idle sweep must short-circuit to an empty Vec without
    // panicking — the contract surface a host's sweeper will rely on.
    let dropped = agent.disconnect_idle_mcp(0).await;
    assert!(dropped.is_empty());
}

#[tokio::test]
async fn test_identity_labels_default_to_none() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-id-default", None)
        .await
        .unwrap();
    assert!(session.tenant_id().is_none());
    assert!(session.principal().is_none());
    assert!(session.agent_template_id().is_none());
    assert!(session.correlation_id().is_none());
}

#[tokio::test]
async fn test_identity_labels_round_trip_via_session_options() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_tenant_id("acme-corp")
        .with_principal("user-42")
        .with_agent_template_id("planner-v3")
        .with_correlation_id("trace-deadbeef");
    let session = agent
        .session_async("/tmp/test-id-set", Some(opts))
        .await
        .expect("session");

    assert_eq!(session.tenant_id(), Some("acme-corp"));
    assert_eq!(session.principal(), Some("user-42"));
    assert_eq!(session.agent_template_id(), Some("planner-v3"));
    assert_eq!(session.correlation_id(), Some("trace-deadbeef"));
}

#[tokio::test]
async fn test_agent_list_sessions_tracks_live_sessions() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    assert!(agent.list_sessions().await.is_empty());

    let opts_a = SessionOptions::new().with_session_id("registry-a");
    let opts_b = SessionOptions::new().with_session_id("registry-b");
    let session_a = agent
        .build_session(
            "/tmp/test-registry-a".into(),
            Arc::new(StaticStreamingClient::new("answer-a")),
            &opts_a,
        )
        .unwrap();
    let session_b = agent
        .build_session(
            "/tmp/test-registry-b".into(),
            Arc::new(StaticStreamingClient::new("answer-b")),
            &opts_b,
        )
        .unwrap();

    let ids = agent.list_sessions().await;
    assert_eq!(
        ids,
        vec!["registry-a".to_string(), "registry-b".to_string()]
    );

    drop(session_a);
    // After drop, the registry's Weak becomes dangling; list_sessions prunes it.
    let after = agent.list_sessions().await;
    assert_eq!(after, vec!["registry-b".to_string()]);

    drop(session_b);
    assert!(agent.list_sessions().await.is_empty());
}

#[tokio::test]
async fn test_agent_rejects_duplicate_live_ids_across_sync_and_async_factories() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let memory = || Arc::new(a3s_memory::InMemoryStore::new());

    let sync_session = agent
        .session(
            "/tmp/test-duplicate-sync-owner",
            Some(
                SessionOptions::new()
                    .with_session_id("duplicate-factory-id")
                    .with_memory(memory()),
            ),
        )
        .expect("first sync session");
    let duplicate_async = agent
        .session_async(
            "/tmp/test-duplicate-async-contender",
            Some(
                SessionOptions::new()
                    .with_session_id("duplicate-factory-id")
                    .with_memory(memory()),
            ),
        )
        .await
        .expect_err("async factory must not replace a live sync session");
    assert!(matches!(
        duplicate_async,
        crate::error::CodeError::SessionConfiguration {
            field: "session_id",
            ..
        }
    ));

    drop(sync_session);
    let async_session = agent
        .session_async(
            "/tmp/test-duplicate-async-owner",
            Some(
                SessionOptions::new()
                    .with_session_id("duplicate-factory-id")
                    .with_memory(memory()),
            ),
        )
        .await
        .expect("dropped weak entry must permit ID reuse");
    let duplicate_sync = agent
        .session(
            "/tmp/test-duplicate-sync-contender",
            Some(
                SessionOptions::new()
                    .with_session_id("duplicate-factory-id")
                    .with_memory(memory()),
            ),
        )
        .expect_err("sync factory must not replace a live async session");
    assert!(matches!(
        duplicate_sync,
        crate::error::CodeError::SessionConfiguration {
            field: "session_id",
            ..
        }
    ));
    assert_eq!(
        agent.list_sessions().await,
        vec!["duplicate-factory-id".to_string()]
    );
    drop(async_session);
}

#[tokio::test]
async fn synchronous_session_build_keeps_configured_memory_observers() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let observer = Arc::new(CountingMemoryObserver::default());
    let session = agent
        .session(
            "/tmp/test-sync-memory-observer",
            Some(
                SessionOptions::new()
                    .with_session_id("sync-memory-observer")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                    .with_memory_observer(observer.clone()),
            ),
        )
        .unwrap();

    session
        .memory()
        .unwrap()
        .remember(a3s_memory::MemoryItem::new("A durable learned preference."))
        .await
        .unwrap();

    assert_eq!(observer.0.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[tokio::test]
async fn test_closed_session_releases_id_while_old_handle_is_still_held() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session_id = "closed-id-reuse";
    let first = agent
        .session_async(
            "/tmp/test-closed-id-first",
            Some(SessionOptions::new().with_session_id(session_id)),
        )
        .await
        .unwrap();

    first.close().await;
    assert!(first.is_closed());
    assert!(agent.list_sessions().await.is_empty());

    let replacement = agent
        .session_async(
            "/tmp/test-closed-id-replacement",
            Some(SessionOptions::new().with_session_id(session_id)),
        )
        .await
        .expect("a closed session is no longer a live ID owner");
    assert_eq!(replacement.session_id(), session_id);

    // Keep `first` alive through replacement construction to prove registry
    // admission depends on lifecycle state, not garbage collection timing.
    assert!(first.is_closed());
}

#[tokio::test]
async fn test_failed_sync_and_async_builds_release_session_id_reservations() {
    let agent = Agent::from_config(test_config()).await.unwrap();

    let invalid_sync = SessionOptions::new()
        .with_session_id("failed-sync-reservation")
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_tool_timeout(0);
    agent
        .session("/tmp/test-failed-sync-reservation", Some(invalid_sync))
        .expect_err("conflicting sync options must fail");
    let sync_session = agent
        .session(
            "/tmp/test-retry-sync-reservation",
            Some(
                SessionOptions::new()
                    .with_session_id("failed-sync-reservation")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new())),
            ),
        )
        .expect("failed sync build must release its reservation");
    drop(sync_session);

    let invalid_async = SessionOptions::new()
        .with_session_id("failed-async-reservation")
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_tool_timeout(0);
    agent
        .session_async("/tmp/test-failed-async-reservation", Some(invalid_async))
        .await
        .expect_err("conflicting async options must fail");
    let async_session = agent
        .session_async(
            "/tmp/test-retry-async-reservation",
            Some(
                SessionOptions::new()
                    .with_session_id("failed-async-reservation")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new())),
            ),
        )
        .await
        .expect("failed async build must release its reservation");
    drop(async_session);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_agent_close_rejects_blocked_resume_finalization() {
    let backing = Arc::new(crate::store::MemorySessionStore::new());
    let writer = Agent::from_config(test_config()).await.unwrap();
    let source = writer
        .session_async(
            "/tmp/test-close-blocked-resume-source",
            Some(
                SessionOptions::new()
                    .with_session_id("close-blocked-resume")
                    .with_session_store(backing.clone()),
            ),
        )
        .await
        .unwrap();
    source.save().await.unwrap();
    drop(source);
    drop(writer);

    let blocking = Arc::new(BlockingLoadSessionStore::new(Arc::clone(&backing)));
    let agent = Arc::new(Agent::from_config(test_config()).await.unwrap());
    let resume = tokio::spawn({
        let agent = Arc::clone(&agent);
        let store: Arc<dyn SessionStore> = blocking.clone();
        async move {
            agent
                .resume_session_async(
                    "close-blocked-resume",
                    SessionOptions::new().with_session_store(store),
                )
                .await
        }
    });
    blocking.wait_until_load_is_blocked().await;

    assert!(agent.list_sessions().await.is_empty());
    assert!(!agent.close_session("close-blocked-resume").await);
    let duplicate = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        agent.resume_session_async(
            "close-blocked-resume",
            SessionOptions::new().with_session_store(blocking.clone() as Arc<dyn SessionStore>),
        ),
    )
    .await
    .expect("duplicate build must fail before entering the blocking store")
    .expect_err("a building session ID must be reserved");
    assert!(matches!(
        duplicate,
        crate::error::CodeError::SessionConfiguration {
            field: "session_id",
            ..
        }
    ));

    agent.close().await;
    blocking.release_one_load();
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), resume)
        .await
        .expect("resume task must finish after its load is released")
        .expect("resume task must not panic");
    let error = result.expect_err("close must reject a pre-admitted resume at finalization");
    assert!(matches!(
        error,
        crate::error::CodeError::SessionClosed { .. }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_cancelled_resume_releases_session_id_reservation() {
    let backing = Arc::new(crate::store::MemorySessionStore::new());
    let writer = Agent::from_config(test_config()).await.unwrap();
    let source = writer
        .session_async(
            "/tmp/test-cancelled-resume-source",
            Some(
                SessionOptions::new()
                    .with_session_id("cancelled-resume-reservation")
                    .with_session_store(backing.clone()),
            ),
        )
        .await
        .unwrap();
    source.save().await.unwrap();
    drop(source);
    drop(writer);

    let blocking = Arc::new(BlockingLoadSessionStore::new(Arc::clone(&backing)));
    let agent = Arc::new(Agent::from_config(test_config()).await.unwrap());
    let resume = tokio::spawn({
        let agent = Arc::clone(&agent);
        let store: Arc<dyn SessionStore> = blocking.clone();
        async move {
            agent
                .resume_session_async(
                    "cancelled-resume-reservation",
                    SessionOptions::new().with_session_store(store),
                )
                .await
        }
    });
    blocking.wait_until_load_is_blocked().await;
    resume.abort();
    assert!(resume
        .await
        .expect_err("task must be cancelled")
        .is_cancelled());

    let resumed = agent
        .resume_session_async(
            "cancelled-resume-reservation",
            SessionOptions::new().with_session_store(backing.clone() as Arc<dyn SessionStore>),
        )
        .await
        .expect("cancelling a build future must release its reservation");
    assert_eq!(resumed.session_id(), "cancelled-resume-reservation");
}

#[tokio::test]
async fn test_agent_close_session_closes_target_session() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("close-by-id");
    let session = agent
        .build_session(
            "/tmp/test-agent-close-session".into(),
            Arc::new(StaticStreamingClient::new("never")),
            &opts,
        )
        .unwrap();
    assert!(!session.is_closed());

    assert!(agent.close_session("close-by-id").await);
    assert!(session.is_closed());

    // Idempotent: second call still reports `true` (we found a live handle)
    // OR `false` (target already closed) — accept either; what matters is no panic.
    let _ = agent.close_session("close-by-id").await;

    // Unknown ids report false.
    assert!(!agent.close_session("does-not-exist").await);
}

#[tokio::test]
async fn test_session_close_waits_for_accepted_memory_extraction_to_persist() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_id("close-drains-memory")
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()));
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-close-drains-memory".into(),
                Arc::new(StaticStreamingClient::new("unused")),
                &opts,
            )
            .unwrap(),
    );
    let memory = Arc::clone(session.memory().expect("session memory"));
    let mut ticket = memory.enqueue_llm_extraction();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let extraction = tokio::spawn({
        let memory = Arc::clone(&memory);
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        async move {
            ticket.wait_for_turn().await;
            started.notify_one();
            release.notified().await;
            memory
                .remember(
                    a3s_memory::MemoryItem::new(
                        "Session close preserves accepted durable memory extraction.",
                    )
                    .with_type(a3s_memory::MemoryType::Semantic),
                )
                .await
                .unwrap();
        }
    });
    started.notified().await;

    let close = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.close().await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert!(
        !close.is_finished(),
        "close must wait while an accepted extraction is still pending"
    );

    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(1), close)
        .await
        .expect("close should finish after memory persistence")
        .unwrap();
    extraction.await.unwrap();
    assert!(session.is_closed());
    assert_eq!(memory.stats().await.unwrap().long_term_count, 1);
}

#[tokio::test]
async fn test_agent_close_closes_every_live_session() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts_a = SessionOptions::new().with_session_id("agent-close-a");
    let opts_b = SessionOptions::new().with_session_id("agent-close-b");
    let session_a = agent
        .build_session(
            "/tmp/test-agent-close-a".into(),
            Arc::new(StaticStreamingClient::new("a")),
            &opts_a,
        )
        .unwrap();
    let session_b = agent
        .build_session(
            "/tmp/test-agent-close-b".into(),
            Arc::new(StaticStreamingClient::new("b")),
            &opts_b,
        )
        .unwrap();

    agent.close().await;
    assert!(session_a.is_closed());
    assert!(session_b.is_closed());

    // After Agent::close(), session creation must fail fast — the agent has
    // already disposed of its resources.
    let err = agent
        .session_async("/tmp/test-agent-closed", None)
        .await
        .expect_err("session() after close() must error");
    let msg = err.to_string();
    assert!(msg.contains("closed") || msg.contains("Closed"));
}

#[tokio::test]
async fn test_session_cancel_token_starts_uncancelled() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-session-cancel-fresh", None)
        .await
        .unwrap();
    let tok = session.session_cancel_token();
    assert!(!tok.is_cancelled());
}

#[tokio::test]
async fn test_close_cancels_session_token() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-session-cancel-on-close", None)
        .await
        .unwrap();
    let observer = session.session_cancel_token();
    assert!(!observer.is_cancelled());

    session.close().await;
    assert!(observer.is_cancelled());
}

#[tokio::test]
async fn test_session_cancel_token_propagates_to_in_flight_run() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-session-cancel-cascades".into(),
                Arc::new(CancellableStreamingClient::new("partial answer")),
                &SessionOptions::new(),
            )
            .unwrap(),
    );

    let worker_session = Arc::clone(&session);
    let worker = tokio::spawn(async move { worker_session.send("hello", None).await });

    let mut run_id = None;
    for _ in 0..50 {
        if let Some(current) = session.current_run().await {
            run_id = Some(current.id().to_string());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let run_id = run_id.expect("current run should be visible");

    // Fire the session-level token directly, bypassing close()/cancel().
    // The in-flight run's token must be a *child* of this one for
    // cancellation to propagate.
    session.session_cancel_token().cancel();

    let result = tokio::time::timeout(std::time::Duration::from_secs(1), worker)
        .await
        .expect("send should stop after session_cancel fires")
        .expect("worker should not panic");
    let result = result.expect("session cancellation should preserve interrupted history");
    assert_eq!(result.messages.len(), 2);
    assert_eq!(result.messages[0].text(), "hello");
    assert!(result.messages[1].text().contains("interrupted"));
    assert_eq!(
        session.run_snapshot(&run_id).await.unwrap().status,
        crate::run::RunStatus::Cancelled
    );
}

#[tokio::test]
async fn test_send_with_attachments_passes_session_id_to_context_providers() {
    let provider = Arc::new(CapturingContextProvider::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_id("attachments-context-session")
        .with_context_provider(provider.clone());
    let session = agent
        .build_session(
            "/tmp/test-send-attachments-context".into(),
            Arc::new(StaticStreamingClient::new("attachment answer")),
            &opts,
        )
        .unwrap();
    let attachments = vec![crate::llm::Attachment::png(vec![1, 2, 3])];

    session
        .send_with_attachments("hello", &attachments, None)
        .await
        .unwrap();

    let session_ids = provider.session_ids.lock().unwrap();
    assert!(!session_ids.is_empty());
    assert!(session_ids
        .iter()
        .all(|id| id.as_deref() == Some("attachments-context-session")));
}

#[tokio::test]
async fn test_send_records_run_snapshot_and_events() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-send-run-store".into(),
            Arc::new(StaticStreamingClient::new("run answer")),
            &SessionOptions::new(),
        )
        .unwrap();

    let result = session.send("hello", None).await.unwrap();
    assert_eq!(result.text, "run answer");

    let runs = session.runs().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].status, crate::run::RunStatus::Completed);
    assert_eq!(runs[0].result_text.as_deref(), Some("run answer"));

    let events = session.run_events(&runs[0].id).await;
    assert!(events
        .iter()
        .any(|record| matches!(record.event, AgentEvent::Start { .. })));
    assert!(events
        .iter()
        .any(|record| matches!(record.event, AgentEvent::End { .. })));
}

#[tokio::test]
async fn test_send_publishes_runtime_events_to_hook_executor() {
    let hook = Arc::new(RecordingRuntimeHook::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_hook_executor(hook.clone());
    let session = agent
        .build_session(
            "/tmp/test-runtime-event-hook".into(),
            Arc::new(StaticStreamingClient::new("hooked answer")),
            &opts,
        )
        .unwrap();

    session.send("hello", None).await.unwrap();

    let events = hook.events.lock().unwrap();
    assert!(events
        .iter()
        .any(|(_, session_id, event)| session_id == session.id()
            && matches!(event, AgentEvent::Start { .. })));
    assert!(events
        .iter()
        .any(|(_, session_id, event)| session_id == session.id()
            && matches!(event, AgentEvent::End { .. })));
    assert!(events
        .iter()
        .all(|(run_id, _, _)| run_id.starts_with("run-")));
}

#[tokio::test]
async fn session_pre_prompt_rewrite_reaches_model_history() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let options = SessionOptions::new()
        .with_hook_executor(Arc::new(RewritingPromptHook))
        .with_continuation(false);
    let session = agent
        .build_session(
            "/tmp/test-pre-prompt-rewrite".into(),
            Arc::new(StaticStreamingClient::new("hooked answer")),
            &options,
        )
        .unwrap();

    let result = session.send("ORIGINAL_PROMPT", None).await.unwrap();
    let user_prompt = result
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "user")
        .expect("rewritten user message")
        .text();
    assert!(user_prompt.contains("HOOK_REWRITTEN_PROMPT"));
    assert!(user_prompt.contains("HOOK_ADDITIONAL_CONTEXT"));
    assert!(!user_prompt.contains("ORIGINAL_PROMPT"));
}

#[tokio::test]
async fn session_pre_prompt_block_emits_stream_error() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let options = SessionOptions::new()
        .with_hook_executor(Arc::new(BlockingPromptHook))
        .with_continuation(false);
    let session = agent
        .build_session(
            "/tmp/test-pre-prompt-block".into(),
            Arc::new(StaticStreamingClient::new("must not reach model")),
            &options,
        )
        .unwrap();

    let (mut events, worker) = session.stream("BLOCK_ME", None).await.unwrap();
    let mut error = None;
    while let Some(event) = events.recv().await {
        if let AgentEvent::Error { message } = event {
            error = Some(message);
        }
    }
    worker.await.unwrap();

    assert_eq!(
        error.as_deref(),
        Some("User prompt blocked by hook: prompt policy denied this request")
    );
}

#[tokio::test]
async fn async_session_build_and_close_fire_lifecycle_hooks_once() {
    let hook = Arc::new(RecordingRuntimeHook::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(
            "/tmp/test-session-lifecycle-hooks",
            Some(SessionOptions::new().with_hook_executor(hook.clone())),
        )
        .await
        .unwrap();

    assert_eq!(
        hook.hook_events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, crate::hooks::HookEvent::SessionStart(_)))
            .count(),
        1
    );
    session.close().await;
    let events = hook.hook_events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, crate::hooks::HookEvent::SessionEnd(_)))
            .count(),
        1
    );
}

#[tokio::test]
async fn test_stream_exposes_current_run_handle_and_replay() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-stream-run-handle".into(),
            Arc::new(CancellableStreamingClient::new("partial answer")),
            &SessionOptions::new(),
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    let mut saw_delta = false;
    for _ in 0..16 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("stream event before timeout")
            .expect("stream emits event");
        if matches!(event, AgentEvent::TextDelta { .. }) {
            saw_delta = true;
            break;
        }
    }
    assert!(saw_delta);

    let run = session.current_run().await.expect("current run handle");
    assert_eq!(run.session_id(), session.id());
    assert!(matches!(
        run.status().await,
        Some(crate::run::RunStatus::Executing | crate::run::RunStatus::Planning)
    ));
    assert!(run.cancel().await);

    while rx.recv().await.is_some() {}
    handle.await.unwrap();

    let snapshot = run
        .snapshot()
        .await
        .expect("run snapshot remains replayable");
    assert_eq!(snapshot.status, crate::run::RunStatus::Cancelled);
    assert!(!run.events().await.is_empty());
}

#[tokio::test]
async fn test_active_stream_rejects_slash_command_with_session_busy() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("single-flight-slash");
    let session = agent
        .build_session(
            "/tmp/test-single-flight-slash".into(),
            Arc::new(CancellableStreamingClient::new("partial answer")),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("hello", None).await.unwrap();
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::TextDelta { .. }) {
            break;
        }
    }

    // Slash commands read and may mutate session state, so they use the same
    // fail-fast admission gate as model-backed conversation operations. Hold
    // the history lock to prove admission happens before any transcript read.
    let history = Arc::clone(&session.history);
    let (locked_tx, locked_rx) = std::sync::mpsc::sync_channel(0);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
    let history_holder = std::thread::spawn(move || {
        let _history_guard = history
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locked_tx.send(()).expect("test lock receiver remains open");
        release_rx
            .recv()
            .expect("test lock release sender remains open");
    });
    locked_rx
        .recv()
        .expect("history holder acquires the lock before admission check");
    let concurrent = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        session.send("/help", None),
    )
    .await
    .expect("busy admission must not wait for the history lock");
    release_tx
        .send(())
        .expect("history holder remains alive until released");
    history_holder
        .join()
        .expect("history holder must not panic");

    assert!(session.cancel().await);
    while rx.recv().await.is_some() {}
    handle.await.unwrap();

    match concurrent {
        Err(crate::error::CodeError::SessionBusy { session_id }) => {
            assert_eq!(session_id, "single-flight-slash");
        }
        other => panic!("expected SessionBusy, got {other:?}"),
    }

    let command = session.send("/help", None).await.unwrap();
    assert!(command.text.contains("/help"));
}

#[tokio::test]
async fn test_slash_command_outputs_obey_session_security_provider() {
    struct SensitiveCommand;

    impl crate::commands::SlashCommand for SensitiveCommand {
        fn name(&self) -> &str {
            "sensitive"
        }

        fn description(&self) -> &str {
            "Returns sensitive test data"
        }

        fn execute(
            &self,
            _args: &str,
            _ctx: &crate::commands::CommandContext,
        ) -> crate::commands::CommandOutput {
            crate::commands::CommandOutput::text("contact user@example.com")
        }
    }

    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_security_provider(Arc::new(crate::security::DefaultSecurityProvider::new()));
    let session = agent
        .build_session(
            "/tmp/test-secure-slash-command".into(),
            Arc::new(StaticStreamingClient::new("unused")),
            &opts,
        )
        .unwrap();
    session
        .register_command(Arc::new(SensitiveCommand))
        .unwrap();

    let result = session.send("/sensitive", None).await.unwrap();
    assert!(!result.text.contains("user@example.com"));
    assert!(result.text.contains("REDACTED:EMAIL"));

    let (mut rx, handle) = session.stream("/sensitive", None).await.unwrap();
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    handle.await.unwrap();
    assert!(matches!(events.last(), Some(AgentEvent::End { .. })));
    for event in events {
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            !json.contains("user@example.com"),
            "unsanitized event: {json}"
        );
    }
}

#[tokio::test]
async fn test_active_stream_rejects_all_conversation_entrypoints() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("single-flight-all-entrypoints");
    let session = agent
        .build_session(
            "/tmp/test-single-flight-all-entrypoints".into(),
            Arc::new(CancellableStreamingClient::new("partial answer")),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("first", None).await.unwrap();
    while let Some(event) = rx.recv().await {
        if matches!(event, AgentEvent::TextDelta { .. }) {
            break;
        }
    }

    let attachments = vec![crate::llm::Attachment::png(vec![1, 2, 3])];
    assert_session_busy(
        session.send("second", None).await,
        "single-flight-all-entrypoints",
    );
    assert_session_busy(
        session.stream("second", None).await,
        "single-flight-all-entrypoints",
    );
    assert_session_busy(
        session
            .send_with_attachments("second", &attachments, None)
            .await,
        "single-flight-all-entrypoints",
    );
    assert_session_busy(
        session
            .stream_with_attachments("second", &attachments, None)
            .await,
        "single-flight-all-entrypoints",
    );
    assert_session_busy(
        session.resume_run("not-loaded-while-busy").await,
        "single-flight-all-entrypoints",
    );
    assert_session_busy(session.save().await, "single-flight-all-entrypoints");

    assert!(session.cancel().await);
    while rx.recv().await.is_some() {}
    handle.await.unwrap();

    // The public stream handle completes only after its guardian has released
    // admission, so the next operation can start immediately after awaiting it.
    assert!(session.send("/help", None).await.is_ok());
}

#[tokio::test]
async fn test_active_blocking_send_rejects_send_and_stream_then_releases() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("single-flight-blocking");
    let session = Arc::new(
        agent
            .build_session(
                "/tmp/test-single-flight-blocking".into(),
                Arc::new(CancellableStreamingClient::new("partial answer")),
                &opts,
            )
            .unwrap(),
    );

    let worker_session = Arc::clone(&session);
    let first = tokio::spawn(async move { worker_session.send("first", None).await });

    let mut active_run = None;
    for _ in 0..50 {
        if let Some(run) = session.current_run().await {
            active_run = Some(run);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        active_run.is_some(),
        "the first blocking send must become active"
    );

    assert_session_busy(session.send("second", None).await, "single-flight-blocking");
    assert_session_busy(
        session.stream("second", None).await,
        "single-flight-blocking",
    );

    assert!(session.cancel().await);
    let first_result = tokio::time::timeout(std::time::Duration::from_secs(1), first)
        .await
        .expect("the cancelled blocking send must finish")
        .expect("the blocking send task must not panic")
        .expect("cancellation preserves interrupted history");
    assert!(first_result
        .messages
        .last()
        .is_some_and(|message| message.text().contains("interrupted")));

    // Awaiting the blocking operation releases the lease for the next call.
    assert!(session.send("/help", None).await.is_ok());
}

#[tokio::test]
async fn test_session_options_with_agent_dir() {
    let opts = SessionOptions::new()
        .with_agent_dir("/tmp/agents")
        .with_agent_dir("/tmp/more-agents");
    assert_eq!(opts.agent_dirs.len(), 2);
    assert_eq!(opts.agent_dirs[0], PathBuf::from("/tmp/agents"));
    assert_eq!(opts.agent_dirs[1], PathBuf::from("/tmp/more-agents"));
}

// ========================================================================
// Queue Integration Tests
// ========================================================================

#[test]
fn test_session_options_with_queue_config() {
    let qc = SessionQueueConfig::default().with_lane_features();
    let opts = SessionOptions::new().with_queue_config(qc.clone());
    assert!(opts.queue_config.is_some());

    let config = opts.queue_config.unwrap();
    assert!(config.enable_dlq);
    assert!(config.enable_metrics);
    assert!(config.enable_alerts);
    assert_eq!(config.default_timeout_ms, Some(60_000));
}

#[tokio::test]
async fn test_session_uses_single_delegation_tool_surface() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace-delegation-tools", None)
        .await
        .unwrap();
    let names = session.tool_names();

    assert!(names.contains(&"task".to_string()));
    assert!(!names.contains(&"parallel_task".to_string()));
    assert!(session.tool_executor.registry().contains("parallel_task"));
    assert!(!names.contains(&"run_team".to_string()));
}

#[tokio::test]
async fn test_session_can_disable_manual_delegation_tools_without_dropping_registry() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_worker_agent(crate::subagent::WorkerAgentSpec::planner(
            "release-planner",
            "Plan releases",
        ))
        .with_manual_delegation_enabled(false);
    let session = agent
        .session_async("/tmp/test-workspace-no-manual-delegation", Some(opts))
        .await
        .unwrap();
    let names = session.tool_names();

    assert!(!names.contains(&"task".to_string()));
    assert!(!names.contains(&"parallel_task".to_string()));
    assert!(session.agent_registry.exists("release-planner"));
    assert!(!session.config.auto_delegation.allow_manual_delegation);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_with_queue_config() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-queue", Some(opts))
        .await;
    assert!(session.is_ok());
    let session = session.unwrap();
    assert!(session.has_queue());
}

#[tokio::test]
async fn test_session_without_queue_config() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace-noqueue", None)
        .await
        .unwrap();
    assert!(!session.has_queue());
}

#[tokio::test]
async fn test_session_queue_stats_without_queue() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace-stats", None)
        .await
        .unwrap();
    let stats = session.queue_stats().await;
    // Without a queue, stats should have zero values
    assert_eq!(stats.total_pending, 0);
    assert_eq!(stats.total_active, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_queue_stats_with_queue() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-qstats", Some(opts))
        .await
        .unwrap();
    let stats = session.queue_stats().await;
    // Fresh queue with no commands should have zero stats
    assert_eq!(stats.total_pending, 0);
    assert_eq!(stats.total_active, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_pending_external_tasks_empty() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-ext", Some(opts))
        .await
        .unwrap();
    let tasks = session.pending_external_tasks().await;
    assert!(tasks.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_confirmation_api_resolves_pending_request() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);
    let manager = Arc::new(crate::hitl::ConfirmationManager::new(
        crate::hitl::ConfirmationPolicy::enabled(),
        event_tx,
    ));
    let opts = SessionOptions::new().with_confirmation_manager(manager.clone());
    let session = agent
        .session_async("/tmp/test-workspace", Some(opts))
        .await
        .unwrap();

    let receiver = manager
        .request_confirmation(
            "tool-1",
            "bash",
            &serde_json::json!({ "command": "echo hi" }),
        )
        .await;

    let pending = session.pending_confirmations().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].tool_id, "tool-1");
    assert_eq!(pending[0].tool_name, "bash");

    let found = session
        .confirm_tool_use("tool-1", true, Some("approved by test".to_string()))
        .await
        .unwrap();
    assert!(found);

    let response = receiver.await.unwrap();
    assert!(response.approved);
    assert_eq!(response.reason.as_deref(), Some("approved by test"));
    assert!(session.pending_confirmations().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn session_close_rejects_and_clears_every_pending_confirmation() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);
    let manager = Arc::new(crate::hitl::ConfirmationManager::new(
        crate::hitl::ConfirmationPolicy::enabled(),
        event_tx,
    ));
    let opts = SessionOptions::new().with_confirmation_manager(manager.clone());
    let session = agent
        .session_async("/tmp/test-workspace-close-confirmations", Some(opts))
        .await
        .unwrap();

    let first = manager
        .request_confirmation("tool-close-1", "bash", &serde_json::json!({}))
        .await;
    let second = manager
        .request_confirmation("tool-close-2", "write", &serde_json::json!({}))
        .await;
    assert_eq!(session.pending_confirmations().await.len(), 2);

    session.close().await;

    for response in [first, second] {
        let response = tokio::time::timeout(std::time::Duration::from_secs(1), response)
            .await
            .expect("session close must settle confirmations promptly")
            .expect("confirmation sender must return a rejection");
        assert!(!response.approved);
        assert_eq!(response.reason.as_deref(), Some("Confirmation cancelled"));
    }
    assert!(session.pending_confirmations().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn session_close_settles_a_stream_blocked_on_confirmation() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (event_tx, _) = tokio::sync::broadcast::channel(8);
    let manager = Arc::new(crate::hitl::ConfirmationManager::new(
        crate::hitl::ConfirmationPolicy::enabled()
            .with_timeout(5_000, crate::hitl::TimeoutAction::Reject),
        event_tx,
    ));
    let opts = SessionOptions::new()
        .with_confirmation_manager(manager.clone())
        .with_permission_policy(crate::permissions::PermissionPolicy::new());
    let session = agent
        .build_session(
            "/tmp/test-close-stream-confirmation".into(),
            Arc::new(ScriptedStreamingClient::new(vec![
                scripted_tool_call_response(
                    "tool-close-stream",
                    "bash",
                    serde_json::json!({"command": "echo must-not-run"}),
                ),
                scripted_text_response("must not continue"),
            ])),
            &opts,
        )
        .unwrap();

    let (mut rx, handle) = session.stream("run a command", None).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while manager.pending_count().await != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the tool confirmation must become pending");
    let run_id = session.current_run().await.unwrap().id().to_string();

    session.close().await;
    tokio::time::timeout(std::time::Duration::from_secs(1), handle)
        .await
        .expect("close must settle the stream lifecycle")
        .expect("stream lifecycle must not panic");

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ConfirmationRequired {
            tool_id,
            tool_name,
            ..
        } if tool_id == "tool-close-stream" && tool_name == "bash"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ConfirmationReceived {
            tool_id,
            approved: false,
            reason,
        } if tool_id == "tool-close-stream"
            && reason.as_deref() == Some("Confirmation cancelled")
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolExecutionStart { id, .. } if id == "tool-close-stream"
    )));
    assert_eq!(manager.pending_count().await, 0);
    assert!(session.current_run().await.is_none());
    assert_eq!(
        session.run_snapshot(&run_id).await.unwrap().status,
        crate::run::RunStatus::Cancelled
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_confirmation_api_without_manager_is_noop() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace", None)
        .await
        .unwrap();

    assert!(session.pending_confirmations().await.is_empty());
    assert!(!session
        .confirm_tool_use("missing", true, None)
        .await
        .unwrap());
    assert_eq!(session.cancel_confirmations().await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_dead_letters_empty() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default().with_dlq(Some(100));
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-dlq", Some(opts))
        .await
        .unwrap();
    let dead = session.dead_letters().await;
    assert!(dead.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_queue_metrics_disabled() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    // Metrics not enabled
    let qc = SessionQueueConfig::default();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-nomet", Some(opts))
        .await
        .unwrap();
    let metrics = session.queue_metrics().await;
    assert!(metrics.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_queue_metrics_enabled() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default().with_metrics();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-met", Some(opts))
        .await
        .unwrap();
    let metrics = session.queue_metrics().await;
    assert!(metrics.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_set_lane_handler() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let qc = SessionQueueConfig::default();
    let opts = SessionOptions::new().with_queue_config(qc);
    let session = agent
        .session_async("/tmp/test-workspace-handler", Some(opts))
        .await
        .unwrap();

    // Set Execute lane to External mode
    session
        .set_lane_handler(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: crate::queue::TaskHandlerMode::External,
                timeout_ms: 30_000,
            },
        )
        .await
        .unwrap();

    // No panic = success. The handler config is stored internally.
    // We can't directly read it back but we verify no errors.
}

// ========================================================================
// Session Persistence Tests
// ========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_session_has_id() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async("/tmp/test-ws-id", None).await.unwrap();
    // Auto-generated UUID
    assert!(!session.session_id().is_empty());
    assert_eq!(session.session_id().len(), 36); // UUID format
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_explicit_id() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_session_id("my-session-42");
    let session = agent
        .session_async("/tmp/test-ws-eid", Some(opts))
        .await
        .unwrap();
    assert_eq!(session.session_id(), "my-session-42");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_artifact_store_limits_option() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts =
        SessionOptions::new().with_artifact_store_limits(crate::tools::ArtifactStoreLimits {
            max_artifacts: 3,
            max_bytes: 4096,
        });
    let session = agent
        .session_async("/tmp/test-ws-artifact-limits", Some(opts))
        .await
        .unwrap();

    let limits = session.tool_executor.artifact_store().limits();
    assert_eq!(limits.max_artifacts, 3);
    assert_eq!(limits.max_bytes, 4096);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_save_no_store() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-save", None)
        .await
        .unwrap();
    // save() is a no-op when no store is configured
    session.save().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_save_and_load() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("persist-test");
    let session = agent
        .session_async("/tmp/test-ws-persist", Some(opts))
        .await
        .unwrap();

    // Save empty session
    session.save().await.unwrap();

    // Verify it was stored
    assert!(store.exists("persist-test").await.unwrap());

    let data = store.load("persist-test").await.unwrap().unwrap();
    assert_eq!(data.id, "persist-test");
    assert!(data.messages.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_save_persists_runtime_config() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let queue_config = SessionQueueConfig::default().with_metrics();
    let confirmation_policy = crate::hitl::ConfirmationPolicy::enabled();
    let permission_policy = crate::permissions::PermissionPolicy::new().allow("bash(echo:*)");

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("runtime-config-test")
        .with_model("openai/gpt-4o")
        .with_queue_config(queue_config)
        .with_confirmation_policy(confirmation_policy)
        .with_permission_policy(permission_policy)
        .with_active_skill_tool_restrictions(true);
    let session = agent
        .session_async("/tmp/test-ws-runtime-config", Some(opts))
        .await
        .unwrap();
    session.save().await.unwrap();

    let data = store.load("runtime-config-test").await.unwrap().unwrap();
    assert_eq!(data.model_name.as_deref(), Some("openai/gpt-4o"));
    assert_eq!(
        data.llm_config.as_ref().map(|c| c.provider.as_str()),
        Some("openai")
    );
    assert_eq!(
        data.llm_config.as_ref().map(|c| c.model.as_str()),
        Some("gpt-4o")
    );
    assert!(data.config.queue_config.is_some());
    assert!(data
        .config
        .confirmation_policy
        .as_ref()
        .is_some_and(|p| p.enabled));
    assert!(data
        .config
        .permission_policy
        .as_ref()
        .is_some_and(|p| !p.allow.is_empty()));
    assert!(data.config.enforce_active_skill_tool_restrictions);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_restores_runtime_config() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let queue_config = SessionQueueConfig::default().with_metrics();
    let confirmation_policy = crate::hitl::ConfirmationPolicy::enabled();
    let permission_policy = crate::permissions::PermissionPolicy::new().allow("bash(echo:*)");

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-runtime-config-test")
        .with_model("openai/gpt-4o")
        .with_queue_config(queue_config)
        .with_confirmation_policy(confirmation_policy)
        .with_permission_policy(permission_policy)
        .with_active_skill_tool_restrictions(true);
    let session = agent
        .session_async("/tmp/test-ws-resume-runtime", Some(opts))
        .await
        .unwrap();
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-runtime-config-test", opts2)
        .await
        .unwrap();

    assert_eq!(resumed.model_name, "openai/gpt-4o");
    assert!(resumed.has_queue());
    assert!(resumed.config.confirmation_policy.is_some());
    assert!(resumed.config.confirmation_manager.is_some());
    assert!(resumed.config.permission_policy.is_some());
    assert!(resumed.config.permission_checker.is_some());
    assert!(resumed.config.enforce_active_skill_tool_restrictions);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_save_with_history() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("history-test");
    let session = agent
        .session_async("/tmp/test-ws-hist", Some(opts))
        .await
        .unwrap();

    // Manually inject history
    {
        let mut h = session.history.write().unwrap();
        h.push(Message::user("Hello"));
        h.push(Message::user("How are you?"));
    }

    session.save().await.unwrap();

    let data = store.load("history-test").await.unwrap().unwrap();
    assert_eq!(data.messages.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    // Create and save a session with history
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-test");
    let session = agent
        .session_async("/tmp/test-ws-resume", Some(opts))
        .await
        .unwrap();
    {
        let mut h = session.history.write().unwrap();
        h.push(Message::user("What is Rust?"));
        h.push(Message::user("Tell me more"));
    }
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    // Resume the session
    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-test", opts2)
        .await
        .unwrap();

    assert_eq!(resumed.session_id(), "resume-test");
    let history = resumed.history();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].text(), "What is Rust?");
}

#[tokio::test]
async fn test_session_snapshots_accumulate_completed_run_usage() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("usage-snapshot-test");
    let session = agent
        .build_session(
            "/tmp/test-usage-snapshot".into(),
            Arc::new(StaticStreamingClient::new("done")),
            &opts,
        )
        .unwrap();

    session.send("first", None).await.unwrap();
    session.save().await.unwrap();
    let session_store: Arc<dyn crate::store::SessionStore> = store.clone();
    let first = session_store
        .load_snapshot("usage-snapshot-test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.session.total_usage.total_tokens, 2);

    session.send("second", None).await.unwrap();
    session.save().await.unwrap();
    let second = session_store
        .load_snapshot("usage-snapshot-test")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.session.total_usage.prompt_tokens, 2);
    assert_eq!(second.session.total_usage.completion_tokens, 2);
    assert_eq!(second.session.total_usage.total_tokens, 4);
    assert_eq!(second.session.created_at, first.session.created_at);
}

#[tokio::test]
async fn test_session_uses_finite_retention_by_default_and_allows_explicit_unbounded() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let default_session = agent
        .build_session(
            "/tmp/test-default-retention".into(),
            Arc::new(StaticStreamingClient::new("unused")),
            &SessionOptions::new(),
        )
        .unwrap();
    for index in 0..65 {
        default_session
            .run_store
            .create_run(default_session.id(), &format!("run {index}"))
            .await;
    }
    assert_eq!(default_session.runs().await.len(), 64);

    let unbounded_session = agent
        .build_session(
            "/tmp/test-unbounded-retention".into(),
            Arc::new(StaticStreamingClient::new("unused")),
            &SessionOptions::new()
                .with_retention_limits(crate::retention::SessionRetentionLimits::unbounded()),
        )
        .unwrap();
    for index in 0..65 {
        unbounded_session
            .run_store
            .create_run(unbounded_session.id(), &format!("run {index}"))
            .await;
    }
    assert_eq!(unbounded_session.runs().await.len(), 65);
}

/// H4 regression: a run that completes in-process must DELETE its loop
/// checkpoint (the checkpoint exists only to survive a crash). Before
/// the fix, every tool-using run leaked a checkpoint forever.
///
/// We use a deterministic HostEnv so the run id is predictable, seed a
/// checkpoint under that id, run a (no-tool) send that completes through
/// the normal lifecycle, and assert the checkpoint was cleared.
#[tokio::test(flavor = "multi_thread")]
async fn test_completed_run_clears_its_loop_checkpoint() {
    use crate::host_env::{HostEnv, SequentialIdGenerator, SystemClock};
    use crate::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};

    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    // Deterministic ids: session_id is set explicitly (consumes no
    // counter), so the first next_id() goes to the run -> "run-seq-0".
    let env = Arc::new(HostEnv::new(
        Arc::new(SequentialIdGenerator::new("seq")),
        Arc::new(SystemClock),
    ));
    let opts = SessionOptions::new()
        .with_session_id("ckpt-clear-session")
        .with_session_store(store.clone() as Arc<dyn crate::store::SessionStore>)
        .with_host_env(env);
    let session = agent
        .build_session(
            "/tmp/test-ckpt-clear".into(),
            Arc::new(StaticStreamingClient::new("done")),
            &opts,
        )
        .unwrap();

    // Seed a checkpoint under the run id this send will use.
    let predicted_run_id = "run-seq-0";
    let cp_store: Arc<dyn crate::store::SessionStore> = store.clone();
    cp_store
        .save_loop_checkpoint(
            predicted_run_id,
            &LoopCheckpoint {
                schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
                run_id: predicted_run_id.to_string(),
                session_id: "ckpt-clear-session".to_string(),
                capability_binding: None,
                turn: 1,
                messages: vec![Message::user("seed")],
                total_usage: crate::llm::TokenUsage::default(),
                tool_calls_count: 0,
                verification_reports: Vec::new(),
                convergence: Default::default(),
                checkpoint_ms: 1,
            },
        )
        .await
        .unwrap();

    let result = session.send("hello", None).await.unwrap();
    assert_eq!(result.text, "done");

    // Self-document the predicted run id.
    let runs = session.runs().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, predicted_run_id, "run id must be deterministic");

    // The checkpoint must have been cleared by the run lifecycle.
    let after: Arc<dyn crate::store::SessionStore> = store.clone();
    assert!(
        after
            .load_loop_checkpoint(predicted_run_id)
            .await
            .unwrap()
            .is_none(),
        "completed run must delete its loop checkpoint (else unbounded leak)"
    );
}

/// P3 happy path (cut 2 E2E): a manually-seeded `LoopCheckpoint` in
/// the SessionStore can be picked up by `AgentSession::resume_run`,
/// the loop runs from the checkpoint's message vec (no new user
/// prompt is appended — `execute_from_messages` path), and the
/// resumed run is allocated a **fresh** run id (not the
/// checkpoint's).
///
/// This exercises the contract surface the host will sit on: write a
/// checkpoint on node A, hand the run id to node B which builds a
/// session against the shared store and calls `resume_run`. Crash
/// simulation is reduced to a manual checkpoint seed because the
/// in-process agent loop has no "die mid-round" affordance suitable
/// for unit testing.
#[tokio::test(flavor = "multi_thread")]
async fn test_resume_run_picks_up_from_persisted_checkpoint() {
    use crate::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};

    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    // Seed a checkpoint as if a previous run on another node had
    // completed one tool round and persisted the boundary state.
    let seeded_run_id = "ckpt-old-run-x";
    let seeded_messages = vec![
        Message::user("kick off"),
        Message {
            role: "assistant".to_string(),
            content: vec![crate::llm::ContentBlock::Text {
                text: "intermediate work".to_string(),
            }],
            reasoning_content: None,
        },
    ];
    // Seed NON-ZERO cumulative metrics so the test can detect whether
    // resume_run carries them forward (H2 regression: it used to reset
    // them to zero, under-reporting the resumed AgentResult).
    let checkpoint = LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: seeded_run_id.to_string(),
        session_id: "resume-run-target".to_string(),
        capability_binding: None,
        turn: 1,
        messages: seeded_messages.clone(),
        total_usage: crate::llm::TokenUsage {
            prompt_tokens: 800,
            completion_tokens: 200,
            total_tokens: 1000,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        tool_calls_count: 3,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1_700_000_000_000,
    };
    {
        let cp_store: Arc<dyn crate::store::SessionStore> = store.clone();
        cp_store
            .save_loop_checkpoint(seeded_run_id, &checkpoint)
            .await
            .expect("seed checkpoint");
    }

    // Build a session bound to the same store + a mock LLM that
    // produces a final-answer text. resume_run will feed it the
    // seeded `messages` and the loop should finish on this turn.
    let opts = SessionOptions::new()
        .with_session_store(store.clone() as Arc<dyn crate::store::SessionStore>)
        .with_session_id("resume-run-target");
    let session = agent
        .build_session(
            "/tmp/test-resume-run-target".into(),
            Arc::new(StaticStreamingClient::new("resumed and completed")),
            &opts,
        )
        .unwrap();

    let result = session
        .resume_run(seeded_run_id)
        .await
        .expect("resume_run must succeed");
    assert_eq!(result.text, "resumed and completed");

    // H2: the resumed run must CONTINUE accounting from the checkpoint's
    // cumulative metrics, not reset to zero. The mock LLM adds 2 tokens
    // (1 prompt + 1 completion) for its single turn, so the result must
    // reflect the seeded 1000 + 2 = 1002, and the seeded tool-call count
    // (3) must carry forward (this turn ran no tools).
    assert_eq!(
        result.usage.total_tokens, 1002,
        "resumed run must add to the checkpoint's cumulative token usage, not reset it"
    );
    assert_eq!(result.usage.prompt_tokens, 801);
    assert_eq!(result.usage.completion_tokens, 201);
    assert_eq!(
        result.tool_calls_count, 3,
        "resumed run must preserve the checkpoint's tool-call count"
    );

    // The resumed run records its own run id in the in-memory store,
    // and that id must NOT match the seeded checkpoint id — the
    // framework allocates a fresh run rather than pretending to
    // continue the old one.
    let runs = session.runs().await;
    assert_eq!(runs.len(), 1, "resume_run creates exactly one new run");
    let resumed_run = &runs[0];
    assert_ne!(
        resumed_run.id, seeded_run_id,
        "resumed run must have a fresh id, got the seeded one"
    );
    assert_eq!(resumed_run.status, crate::run::RunStatus::Completed);

    // The checkpoint stays in the store under the OLD run id —
    // resume does not delete it. (The host decides retention.)
    let still_there: Arc<dyn crate::store::SessionStore> = store.clone();
    let cp = still_there
        .load_loop_checkpoint(seeded_run_id)
        .await
        .expect("load")
        .expect("old checkpoint preserved");
    assert_eq!(cp.run_id, seeded_run_id);
    assert_eq!(cp.turn, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_run_preserves_exhausted_tool_budget_for_finalization() {
    use crate::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};

    let store = Arc::new(crate::store::MemorySessionStore::new());
    let checkpoint_run_id = "budget-exhausted-checkpoint";
    let checkpoint = LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: checkpoint_run_id.to_string(),
        session_id: "resume-budget-target".to_string(),
        capability_binding: None,
        turn: 2,
        messages: vec![Message::user("continue")],
        total_usage: Default::default(),
        tool_calls_count: 2,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1,
    };
    let checkpoint_store: Arc<dyn crate::store::SessionStore> = store.clone();
    checkpoint_store
        .save_loop_checkpoint(checkpoint_run_id, &checkpoint)
        .await
        .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let options = SessionOptions::new()
        .with_session_store(store as Arc<dyn crate::store::SessionStore>)
        .with_session_id("resume-budget-target")
        .with_max_tool_rounds(2);
    let session = agent
        .build_session(
            "/tmp/test-resume-budget-target".into(),
            Arc::new(StaticStreamingClient::new(
                "Best bounded answer from the checkpoint evidence.",
            )),
            &options,
        )
        .unwrap();

    let result = session.resume_run(checkpoint_run_id).await.unwrap();
    assert_eq!(
        result.text,
        "Best bounded answer from the checkpoint evidence."
    );
    assert_eq!(
        result.tool_calls_count, 2,
        "the reserved tool-free finalization turn must not reset or consume the restored tool budget"
    );
    assert!(result.messages.iter().any(|message| {
        message
            .text()
            .contains("Tool-use budget reached. Stop gathering evidence")
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_run_rejects_checkpoint_owned_by_another_session() {
    use crate::loop_checkpoint::{LoopCheckpoint, LOOP_CHECKPOINT_SCHEMA_VERSION};

    let store = Arc::new(crate::store::MemorySessionStore::new());
    let run_id = "foreign-checkpoint-run";
    let checkpoint = LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        session_id: "foreign-session".to_string(),
        capability_binding: None,
        turn: 1,
        messages: vec![Message::user("private foreign transcript")],
        total_usage: crate::llm::TokenUsage::default(),
        tool_calls_count: 0,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1,
    };
    let checkpoint_store: Arc<dyn crate::store::SessionStore> = store.clone();
    checkpoint_store
        .save_loop_checkpoint(run_id, &checkpoint)
        .await
        .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-resume-run-owner".into(),
            Arc::new(StaticStreamingClient::new("must not execute")),
            &SessionOptions::new()
                .with_session_store(store as Arc<dyn crate::store::SessionStore>)
                .with_session_id("current-session"),
        )
        .unwrap();

    let error = session.resume_run(run_id).await.unwrap_err();
    assert!(error.to_string().contains("ownership mismatch"));
    assert!(
        session.runs().await.is_empty(),
        "rejected resume must not start a run"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_restores_artifacts() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-artifacts-test");
    let session = agent
        .session_async("/tmp/test-ws-artifacts", Some(opts))
        .await
        .unwrap();
    session
        .tool_executor
        .artifact_store()
        .put(crate::tools::ToolArtifact {
            artifact_id: "tool-output:test:a".to_string(),
            artifact_uri: "a3s://tool-output/test/a".to_string(),
            tool_name: "test".to_string(),
            content: "artifact content".to_string(),
            original_bytes: 16,
            shown_bytes: 4,
        });

    session.save().await.unwrap();
    session.close().await;
    drop(session);
    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-artifacts-test", opts2)
        .await
        .unwrap();

    let artifact = resumed
        .get_artifact("a3s://tool-output/test/a")
        .expect("artifact");
    assert_eq!(artifact.content, "artifact content");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_preserves_artifacts_beyond_default_store_limits() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let defaults = crate::tools::ArtifactStoreLimits::default();
    let artifact_count = defaults.max_artifacts + 1;
    let bytes_per_artifact = defaults.max_bytes / artifact_count + 1;
    let persisted_bytes = artifact_count * bytes_per_artifact;
    let artifact_content = "x".repeat(bytes_per_artifact);
    assert!(artifact_count > defaults.max_artifacts);
    assert!(persisted_bytes > defaults.max_bytes);

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-large-artifacts-test")
        .with_artifact_store_limits(crate::tools::ArtifactStoreLimits {
            max_artifacts: artifact_count,
            max_bytes: persisted_bytes,
        });
    let session = agent
        .session_async("/tmp/test-ws-large-artifacts", Some(opts))
        .await
        .unwrap();
    for index in 0..artifact_count {
        session
            .tool_executor
            .artifact_store()
            .put(crate::tools::ToolArtifact {
                artifact_id: format!("tool-output:test:{index}"),
                artifact_uri: format!("a3s://tool-output/test/{index}"),
                tool_name: "test".to_string(),
                content: artifact_content.clone(),
                original_bytes: bytes_per_artifact,
                shown_bytes: 0,
            });
    }

    session.save().await.unwrap();
    drop(session);

    let resumed = agent
        .resume_session_async(
            "resume-large-artifacts-test",
            SessionOptions::new().with_session_store(store),
        )
        .await
        .unwrap();
    let restored = resumed.tool_executor.artifact_store();
    assert_eq!(restored.len(), artifact_count);
    assert_eq!(restored.total_bytes(), persisted_bytes);
    assert!(restored.get("a3s://tool-output/test/0").is_some());
    assert!(restored
        .get(&format!("a3s://tool-output/test/{}", artifact_count - 1))
        .is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_restores_trace_events() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let event = crate::trace::TraceEvent::tool_execution(
        "read",
        true,
        0,
        std::time::Duration::from_millis(3),
        32,
        Some(&serde_json::json!({
            "artifact": {
                "artifact_uri": "a3s://tool-output/read/abc"
            }
        })),
    );

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-trace-test");
    let session = agent
        .session_async("/tmp/test-ws-trace", Some(opts))
        .await
        .unwrap();
    session.trace_sink.replace_events(vec![event.clone()]);
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-trace-test", opts2)
        .await
        .unwrap();

    assert_eq!(resumed.trace_events(), vec![event]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_restores_run_records() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-runs-test");
    let session = agent
        .session_async("/tmp/test-ws-runs", Some(opts))
        .await
        .unwrap();
    let run = session
        .run_store
        .create_run(session.session_id(), "persist run")
        .await;
    session
        .run_store
        .record_event(
            &run.id,
            AgentEvent::Start {
                prompt: "persist run".to_string(),
            },
        )
        .await;
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-runs-test", opts2)
        .await
        .unwrap();

    let runs = resumed.runs().await;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].prompt, "persist run");
    assert_eq!(resumed.run_events(&run.id).await.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_restores_verification_reports() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let report = crate::verification::VerificationReport::new(
        "program:test",
        vec![
            crate::verification::VerificationCheck::required("check:test", "test", "Run tests")
                .with_status(crate::verification::VerificationStatus::Passed),
        ],
    );

    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("resume-verification-test");
    let session = agent
        .session_async("/tmp/test-ws-verification", Some(opts))
        .await
        .unwrap();
    session.record_verification_reports([report.clone()]);
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    let opts2 = SessionOptions::new().with_session_store(store.clone());
    let resumed = agent
        .resume_session_async("resume-verification-test", opts2)
        .await
        .unwrap();

    assert_eq!(resumed.verification_reports(), vec![report]);
    assert_eq!(
        resumed.verification_summary().status,
        crate::verification::VerificationStatus::Passed
    );
    assert!(resumed
        .verification_summary_text()
        .contains("Verification passed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_verify_commands_builds_report_from_shell_results() {
    #[cfg(windows)]
    let _permit = crate::test_support::resource_intensive_test_permit().await;
    let temp_dir = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(temp_dir.path().display().to_string(), None)
        .await
        .unwrap();
    let commands = vec![
        crate::verification::VerificationCommand::required(
            "check:smoke",
            "smoke",
            "Run smoke command",
            "echo ok",
        ),
        crate::verification::VerificationCommand::required(
            "check:failure",
            "smoke",
            "Run failing command",
            "exit 7",
        ),
    ];

    let report = session.verify_commands("turn", &commands).await.unwrap();

    assert_eq!(report.subject, "turn");
    assert_eq!(
        report.status,
        crate::verification::VerificationStatus::Failed
    );
    assert_eq!(
        report.checks[0].status,
        crate::verification::VerificationStatus::Passed
    );
    assert_eq!(
        report.checks[1].status,
        crate::verification::VerificationStatus::Failed
    );
    assert_eq!(
        report.checks[1].residual_risk.as_deref(),
        Some("verification command exited with code 7: exit 7")
    );
    assert_eq!(session.verification_reports(), vec![report]);
    assert_eq!(
        session.verification_summary().status,
        crate::verification::VerificationStatus::Failed
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_verify_commands_use_host_direct_policy_but_keep_budget_governance() {
    let temp_dir = tempfile::tempdir().unwrap();
    let marker = temp_dir.path().join("verification-must-not-run.txt");
    let guard = Arc::new(DenyingToolBudgetGuard::default());
    let options = SessionOptions::new()
        .with_permission_policy(crate::permissions::PermissionPolicy::new().deny("bash"))
        .with_budget_guard(guard.clone() as Arc<dyn crate::budget::BudgetGuard>);
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(temp_dir.path().display().to_string(), Some(options))
        .await
        .unwrap();
    let commands = [crate::verification::VerificationCommand::required(
        "check:governed",
        "smoke",
        "Exercise governed verification",
        "echo should-not-run > verification-must-not-run.txt",
    )];

    let report = session.verify_commands("turn", &commands).await.unwrap();

    assert_eq!(
        guard.checks.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "host-direct verification must skip model permission but still consult the budget guard"
    );
    assert_eq!(
        report.checks[0].status,
        crate::verification::VerificationStatus::Failed
    );
    assert!(report.checks[0]
        .residual_risk
        .as_deref()
        .is_some_and(|risk| risk.contains("Budget exhausted")));
    assert!(
        !marker.exists(),
        "budget denial must happen before bash runs"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_verification_presets_reflect_workspace() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("package.json"),
        r#"{"scripts":{"test":"vitest","typecheck":"tsc --noEmit"}}"#,
    )
    .unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(temp_dir.path().display().to_string(), None)
        .await
        .unwrap();

    let presets = session.verification_presets();

    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].project_kind, "node");
    assert_eq!(presets[0].commands[0].command, "npm test");
    assert_eq!(presets[0].commands[1].command, "npm run typecheck");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_not_found() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new().with_session_store(store.clone());
    let result = agent.resume_session_async("nonexistent", opts).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_resume_session_no_store() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new();
    let result = agent.resume_session_async("any-id", opts).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("session_store"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_file_session_store_persistence() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Arc::new(
        crate::store::FileSessionStore::new(dir.path())
            .await
            .unwrap(),
    );
    let agent = Agent::from_config(test_config()).await.unwrap();

    // Save
    let opts = SessionOptions::new()
        .with_session_store(store.clone())
        .with_session_id("file-persist");
    let session = agent
        .session_async("/tmp/test-ws-file-persist", Some(opts))
        .await
        .unwrap();
    {
        let mut h = session.history.write().unwrap();
        h.push(Message::user("test message"));
    }
    session.save().await.unwrap();

    // Load from a fresh store instance pointing to same dir
    let store2 = Arc::new(
        crate::store::FileSessionStore::new(dir.path())
            .await
            .unwrap(),
    );
    let data = store2.load("file-persist").await.unwrap().unwrap();
    assert_eq!(data.messages.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_options_builders() {
    let opts = SessionOptions::new()
        .with_session_id("test-id")
        .with_auto_save(true)
        .with_tool_timeout(5_000)
        .with_llm_api_timeout(30_000)
        .with_max_parallel_tasks(3)
        .with_auto_delegation_enabled(true)
        .with_manual_delegation_enabled(false)
        .with_auto_parallel_delegation(false)
        .with_active_skill_tool_restrictions(true);
    assert_eq!(opts.session_id, Some("test-id".to_string()));
    assert!(opts.auto_save);
    assert_eq!(opts.tool_timeout_ms, Some(5_000));
    assert_eq!(opts.llm_api_timeout_ms, Some(30_000));
    assert_eq!(opts.max_parallel_tasks, Some(3));
    assert_eq!(opts.manual_delegation_enabled, Some(false));
    assert_eq!(opts.auto_parallel_delegation, Some(false));
    assert_eq!(opts.enforce_active_skill_tool_restrictions, Some(true));
    let auto = opts.auto_delegation.expect("auto delegation config");
    assert!(auto.enabled);
    assert!(!auto.allow_manual_delegation);
    assert!(!auto.auto_parallel);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_active_skill_tool_restriction_option_defaults_and_overrides() {
    let agent = Agent::from_config(test_config()).await.unwrap();

    let default_session = agent
        .session_async("/tmp/test-ws-skill-restriction-default", None)
        .await
        .unwrap();
    assert!(
        !default_session
            .config
            .enforce_active_skill_tool_restrictions
    );

    let legacy_session = agent
        .session_async(
            "/tmp/test-ws-skill-restriction-legacy",
            Some(SessionOptions::new().with_active_skill_tool_restrictions(true)),
        )
        .await
        .unwrap();
    assert!(legacy_session.config.enforce_active_skill_tool_restrictions);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_options_with_rl_trajectory_records_jsonl() {
    let dir = tempfile::TempDir::new().unwrap();
    let trajectory_path = dir.path().join("trajectory.jsonl");
    let agent = Agent::from_config(test_config()).await.unwrap();

    let opts = SessionOptions::new().with_rl_trajectory(
        crate::rl_trajectory::RlTrajectoryConfig::new(&trajectory_path).with_max_text_bytes(4),
    );
    let session = agent
        .session_async("/tmp/test-ws-rl-trajectory", Some(opts))
        .await
        .unwrap();

    assert!(session.config.rl_trajectory_recorder.is_enabled());
    session
        .config
        .rl_trajectory_recorder
        .record_execution_start(crate::rl_trajectory::ExecutionStartRecord {
            session_id: "sess-rl",
            workspace: std::path::Path::new("/tmp/test-ws-rl-trajectory"),
            prompt: "abcdef",
            history: &[],
            system_prompt: None,
            max_tool_rounds: 16,
            planning_mode: "disabled",
        });

    let content = std::fs::read_to_string(&trajectory_path).unwrap();
    let record: serde_json::Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
    assert_eq!(record["schema"], crate::rl_trajectory::RL_TRAJECTORY_SCHEMA);
    assert_eq!(record["event_type"], "execution_start");
    assert_eq!(record["session_id"], "sess-rl");
    assert_eq!(record["payload"]["prompt"]["text"], "abcd");
    assert_eq!(record["payload"]["prompt"]["truncated"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_max_parallel_tasks_config_and_override() {
    let mut config = test_config();
    config.max_parallel_tasks = Some(6);
    config.auto_delegation.enabled = true;
    config.auto_delegation.auto_parallel = false;
    let agent = Agent::from_config(config).await.unwrap();

    let default_session = agent
        .session_async("/tmp/test-ws-parallel-default", None)
        .await
        .unwrap();
    assert_eq!(default_session.config.max_parallel_tasks, 6);
    assert!(default_session.config.auto_delegation.enabled);
    assert!(!default_session.config.auto_delegation.auto_parallel);

    let override_session = agent
        .session_async(
            "/tmp/test-ws-parallel-override",
            Some(
                SessionOptions::new()
                    .with_max_parallel_tasks(2)
                    .with_auto_parallel_delegation(true),
            ),
        )
        .await
        .unwrap();
    assert_eq!(override_session.config.max_parallel_tasks, 2);
    assert!(override_session.config.auto_delegation.auto_parallel);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_auto_parallel_override_preserves_base_auto_delegation() {
    let mut config = test_config();
    config.auto_delegation.enabled = true;
    config.auto_delegation.auto_parallel = true;
    let agent = Agent::from_config(config).await.unwrap();

    let session = agent
        .session_async(
            "/tmp/test-ws-auto-parallel-only",
            Some(SessionOptions::new().with_auto_parallel_delegation(false)),
        )
        .await
        .unwrap();

    assert!(session.config.auto_delegation.enabled);
    assert!(!session.config.auto_delegation.auto_parallel);
}

// ========================================================================
// Memory Integration Tests
// ========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_session_with_memory_store() {
    use a3s_memory::InMemoryStore;
    let store = Arc::new(InMemoryStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_memory(store);
    let session = agent
        .session_async("/tmp/test-ws-memory", Some(opts))
        .await
        .unwrap();
    assert!(session.memory().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_without_memory_store() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-no-memory", None)
        .await
        .unwrap();
    assert!(
        session.memory().is_some(),
        "sessions should have a default memory store"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_memory_wired_into_config() {
    use a3s_memory::InMemoryStore;
    let store = Arc::new(InMemoryStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_memory(store);
    let session = agent
        .session_async("/tmp/test-ws-mem-config", Some(opts))
        .await
        .unwrap();
    // memory is accessible via the public session API
    assert!(session.memory().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_memory_uses_code_config_limits() {
    use a3s_memory::{InMemoryStore, MemoryItem};

    let mut config = test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        max_short_term: 1,
        ..Default::default()
    });
    let store = Arc::new(InMemoryStore::new());
    let agent = Agent::from_config(config).await.unwrap();
    let opts = SessionOptions::new().with_memory(store);
    let session = agent
        .session_async("/tmp/test-ws-mem-config-limits", Some(opts))
        .await
        .unwrap();

    let memory = session.memory().unwrap();
    memory.remember(MemoryItem::new("one")).await.unwrap();
    memory.remember(MemoryItem::new("two")).await.unwrap();

    assert_eq!(memory.short_term_count().await, 1);
    assert_eq!(memory.stats().await.unwrap().long_term_count, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_with_file_memory() {
    let dir = tempfile::TempDir::new().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_file_memory(dir.path());
    let session = agent
        .session_async("/tmp/test-ws-file-mem", Some(opts))
        .await
        .unwrap();
    assert!(session.memory().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_uses_configured_default_memory_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut config = test_config();
    config.memory_dir = Some(dir.path().join("memory"));
    let agent = Agent::from_config(config).await.unwrap();

    let session = agent
        .session_async("/tmp/test-ws-default-file-mem", None)
        .await
        .unwrap();
    let memory = session.memory().expect("default memory store");
    memory
        .remember(a3s_memory::MemoryItem::new("configured default memory dir"))
        .await
        .unwrap();

    assert!(dir.path().join("memory/index.json").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_file_memory_initialization_failure_is_typed() {
    let dir = tempfile::TempDir::new().unwrap();
    let blocked_path = dir.path().join("blocked-memory");
    std::fs::write(&blocked_path, "not a directory").unwrap();

    let mut config = test_config();
    config.memory_dir = Some(blocked_path.clone());
    let agent = Agent::from_config(config).await.unwrap();

    let error = agent
        .session_async("/tmp/test-ws-memory-fallback", None)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::SessionInitialization {
            resource: crate::error::SessionBuildResource::MemoryStore,
            ..
        }
    ));
    assert!(blocked_path.is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_memory_remember_and_recall() {
    use a3s_memory::InMemoryStore;
    let store = Arc::new(InMemoryStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_memory(store);
    let session = agent
        .session_async("/tmp/test-ws-mem-recall", Some(opts))
        .await
        .unwrap();

    let memory = session.memory().unwrap();
    memory
        .remember_success("write a file", &["write".to_string()], "done")
        .await
        .unwrap();

    let results = memory.recall_similar("write", 5).await.unwrap();
    assert!(!results.is_empty());
    let stats = memory.stats().await.unwrap();
    assert_eq!(stats.long_term_count, 1);
}

// ========================================================================
// Tool timeout tests
// ========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_session_tool_timeout_configured() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_tool_timeout(5000)
        .with_llm_api_timeout(30_000);
    let session = agent
        .session_async("/tmp/test-ws-timeout", Some(opts))
        .await
        .unwrap();
    assert!(!session.id().is_empty());
    assert_eq!(session.config.tool_timeout_ms, Some(5_000));
    assert_eq!(session.config.llm_api_timeout_ms, Some(30_000));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_llm_api_timeout_does_not_configure_tool_timeout() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_llm_api_timeout(30_000);
    let session = agent
        .session_async("/tmp/test-ws-api-timeout-only", Some(opts))
        .await
        .unwrap();

    assert_eq!(session.config.llm_api_timeout_ms, Some(30_000));
    assert_eq!(
        session.config.tool_timeout_ms, None,
        "model API timeout must not also constrain tool execution"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_duplicate_tool_call_threshold_configured() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_duplicate_tool_call_threshold(12);
    let session = agent
        .session_async("/tmp/test-ws-duplicate-threshold", Some(opts))
        .await
        .unwrap();

    assert_eq!(session.config.duplicate_tool_call_threshold, 12);
    assert_eq!(
        session.parent_run_context().duplicate_tool_call_threshold,
        Some(12),
        "delegated child runs must inherit the same repeated-tool guard"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_confirmation_timeout_does_not_configure_tool_timeout() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let confirmation = crate::hitl::ConfirmationPolicy::enabled()
        .with_timeout(5_000, crate::hitl::TimeoutAction::Reject);
    let opts = SessionOptions::new().with_confirmation_policy(confirmation);
    let session = agent
        .session_async("/tmp/test-ws-confirmation-timeout-only", Some(opts))
        .await
        .unwrap();

    assert!(session.config.confirmation_manager.is_some());
    assert_eq!(
        session
            .config
            .confirmation_policy
            .as_ref()
            .map(|policy| policy.default_timeout_ms),
        Some(5_000)
    );
    assert_eq!(
        session.config.tool_timeout_ms, None,
        "HITL confirmation waiting must not consume or configure the tool execution timeout budget"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_confirmation_and_tool_timeouts_remain_independent() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let confirmation = crate::hitl::ConfirmationPolicy::enabled()
        .with_timeout(5_000, crate::hitl::TimeoutAction::Reject);
    let opts = SessionOptions::new()
        .with_confirmation_policy(confirmation)
        .with_tool_timeout(300);
    let session = agent
        .session_async(
            "/tmp/test-ws-independent-confirmation-tool-timeouts",
            Some(opts),
        )
        .await
        .unwrap();

    assert_eq!(
        session
            .config
            .confirmation_policy
            .as_ref()
            .map(|policy| policy.default_timeout_ms),
        Some(5_000)
    );
    assert_eq!(
        session.config.tool_timeout_ms,
        Some(300),
        "tool timeout must stay as the explicit execution budget, not the HITL wait budget"
    );
}

// ========================================================================
// Queue fallback tests
// ========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_session_without_queue_builds_ok() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-no-queue", None)
        .await
        .unwrap();
    assert!(!session.id().is_empty());
}

// ========================================================================
// Concurrent history access tests
// ========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_concurrent_history_reads() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .session_async("/tmp/test-ws-concurrent", None)
            .await
            .unwrap(),
    );

    let handles: Vec<_> = (0..10)
        .map(|_| {
            let s = Arc::clone(&session);
            tokio::spawn(async move { s.history().len() })
        })
        .collect();

    for h in handles {
        h.await.unwrap();
    }
}

// ========================================================================
// init_warning tests
// ========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_session_no_init_warning_without_file_memory() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-no-warn", None)
        .await
        .unwrap();
    assert!(session.init_warning().is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_agent_dir_loads_agents_into_live_session() {
    let temp_dir = tempfile::tempdir().unwrap();

    // Write a valid agent file
    std::fs::write(
        temp_dir.path().join("my-agent.yaml"),
        "name: my-dynamic-agent\ndescription: Dynamically registered agent\n",
    )
    .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();

    // The agent must not be known before registration
    assert!(!session.agent_registry.exists("my-dynamic-agent"));

    let count = session.register_agent_dir(temp_dir.path()).unwrap();
    assert_eq!(count, 1);
    assert!(session.agent_registry.exists("my-dynamic-agent"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_agent_dir_empty_dir_returns_zero() {
    let temp_dir = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();
    let count = session.register_agent_dir(temp_dir.path()).unwrap();
    assert_eq!(count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_agent_dir_nonexistent_returns_zero() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();
    let count = session
        .register_agent_dir(std::path::Path::new("/nonexistent/path/abc"))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_worker_agent_loads_worker_into_live_session() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();

    assert!(!session.agent_registry.exists("frontend-cow"));
    let definition = session
        .register_worker_agent(
            crate::subagent::WorkerAgentSpec::implementer(
                "frontend-cow",
                "Disposable frontend implementer",
            )
            .with_max_steps(9),
        )
        .unwrap();

    assert_eq!(definition.max_steps, Some(9));
    assert!(session.agent_registry.exists("frontend-cow"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_register_worker_agents_loads_batch_into_live_session() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();

    let definitions = session
        .register_worker_agents([
            crate::subagent::WorkerAgentSpec::planner("planner-cow", "Plan work"),
            crate::subagent::WorkerAgentSpec::verifier("verify-cow", "Verify work"),
        ])
        .unwrap();

    assert_eq!(definitions.len(), 2);
    assert!(session.agent_registry.exists("planner-cow"));
    assert!(session.agent_registry.exists("verify-cow"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_live_worker_catalog_is_model_visible_and_tracks_registry_changes() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent.session_async(".", None).await.unwrap();

    session
        .register_worker_agent(crate::subagent::WorkerAgentSpec::custom(
            "use",
            "Operate Browser and Office through A3S Use",
        ))
        .unwrap();
    session
        .register_worker_agent(
            crate::subagent::WorkerAgentSpec::custom(
                "hidden-use-helper",
                "Internal application helper",
            )
            .hidden(true),
        )
        .unwrap();

    let definitions = session.tool_definitions();
    let definition = definitions
        .iter()
        .find(|tool| tool.name == "task")
        .expect("unified delegation tool definition");
    assert!(!definitions.iter().any(|tool| tool.name == "parallel_task"));
    let agent_schema =
        &definition.parameters["properties"]["tasks"]["items"]["properties"]["agent"];
    let examples = agent_schema["examples"]
        .as_array()
        .expect("live canonical agent examples");

    assert!(examples.contains(&serde_json::json!("use")));
    assert!(!examples.contains(&serde_json::json!("hidden-use-helper")));
    assert!(agent_schema["description"]
        .as_str()
        .unwrap()
        .contains("use: Operate Browser and Office through A3S Use"));
    assert!(definition
        .description
        .contains("use: Operate Browser and Office through A3S Use"));
    assert!(!definition.description.contains("hidden-use-helper"));

    assert!(session.agent_registry.unregister("use"));
    for definition in session
        .tool_definitions()
        .into_iter()
        .filter(|tool| tool.name == "task")
    {
        assert!(!definition
            .description
            .contains("Operate Browser and Office through A3S Use"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_options_worker_agents_register_for_task_delegation() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_worker_agent(crate::subagent::WorkerAgentSpec::planner(
        "release-planner",
        "Plan releases",
    ));
    let session = agent.session_async(".", Some(opts)).await.unwrap();

    assert!(session.agent_registry.exists("release-planner"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_loads_workspace_a3s_agents() {
    let workspace = tempfile::tempdir().unwrap();
    let agents_dir = workspace.path().join(".a3s").join("agents").join("quality");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(
        agents_dir.join("code-reviewer.md"),
        r#"---
name: code-reviewer
description: Use proactively after code changes to review quality
tools: Read, Grep
---
Review the changed code and return prioritized findings.
"#,
    )
    .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(workspace.path().display().to_string(), None)
        .await
        .unwrap();

    let loaded = session
        .agent_registry
        .get("code-reviewer")
        .expect("workspace .a3s/agents agent should load");
    assert!(loaded
        .permissions
        .allow
        .iter()
        .any(|rule| { rule.matches("read", &serde_json::json!({"file_path": "README.md"})) }));
    assert!(loaded.permissions.allow.iter().any(|rule| {
        rule.matches(
            "search",
            &serde_json::json!({"mode": "grep", "query": "TODO"}),
        )
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_keeps_claude_agents_as_compatibility_source() {
    let workspace = tempfile::tempdir().unwrap();
    let agents_dir = workspace.path().join(".claude").join("agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(
        agents_dir.join("compat-reviewer.md"),
        r#"---
name: compat-reviewer
description: Compatibility agent
tools: Read
---
Review in compatibility mode.
"#,
    )
    .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(workspace.path().display().to_string(), None)
        .await
        .unwrap();

    assert!(session.agent_registry.exists("compat-reviewer"));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_workspace_a3s_agents_override_claude_compat_agents() {
    let workspace = tempfile::tempdir().unwrap();
    let claude_dir = workspace.path().join(".claude").join("agents");
    let a3s_dir = workspace.path().join(".a3s").join("agents");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::create_dir_all(&a3s_dir).unwrap();
    std::fs::write(
        claude_dir.join("same-agent.md"),
        r#"---
name: same-agent
description: Claude compatibility version
tools: Read
---
Compat prompt.
"#,
    )
    .unwrap();
    std::fs::write(
        a3s_dir.join("same-agent.md"),
        r#"---
name: same-agent
description: A3S native version
tools: Read
---
A3S prompt.
"#,
    )
    .unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(workspace.path().display().to_string(), None)
        .await
        .unwrap();

    let loaded = session.agent_registry.get("same-agent").unwrap();
    assert_eq!(loaded.description, "A3S native version");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_for_worker_maps_worker_spec_to_session_options() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_for_worker_async(
            ".",
            crate::subagent::WorkerAgentSpec::reviewer("review-cow", "Review changes")
                .with_max_steps(11),
            None,
        )
        .await
        .unwrap();

    assert_eq!(session.config.max_tool_rounds, 11);
    assert!(session.config.prompt_slots.extra.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_with_mcp_manager_builds_ok() {
    use crate::mcp::manager::McpManager;
    let mcp = Arc::new(McpManager::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_mcp(mcp);
    // No servers connected — should build fine with zero MCP tools registered
    let session = agent
        .session_async("/tmp/test-ws-mcp", Some(opts))
        .await
        .unwrap();
    assert!(!session.id().is_empty());
}

#[test]
fn test_session_command_is_available_from_queue_module() {
    // Compile-time check: SessionCommand remains available from its owning module.
    use crate::queue::SessionCommand;
    let _ = std::marker::PhantomData::<Box<dyn SessionCommand>>;
}

#[tokio::test]
async fn subagent_events_populate_session_tracker() {
    use super::runtime_events::RuntimeEventSink;
    use crate::agent::AgentEvent;
    use crate::subagent_task_tracker::SubagentStatus;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-subagent-tracker", None)
        .await
        .unwrap();

    // Drive a synthetic subagent lifecycle through the session's runtime sink.
    let run = session
        .run_store
        .create_run(session.session_id(), "parent prompt")
        .await;
    let sink = RuntimeEventSink::from_session(&session, &run.id);

    let task_id = "task-test-1".to_string();
    let child_session_id = format!("task-run-{}", task_id);

    sink.observe(&AgentEvent::SubagentStart {
        task_id: task_id.clone(),
        session_id: child_session_id.clone(),
        parent_session_id: session.session_id().to_string(),
        agent: "explore".to_string(),
        description: "demo delegation".to_string(),
        started_ms: 1000,
    })
    .await;

    let snap = session
        .subagent_task(&task_id)
        .await
        .expect("running task should be visible");
    assert_eq!(snap.status, SubagentStatus::Running);
    assert_eq!(snap.parent_session_id, session.session_id());
    assert_eq!(snap.child_session_id, child_session_id);
    assert_eq!(snap.agent, "explore");
    assert!(snap.finished_ms.is_none());

    let pending = session.pending_subagent_tasks().await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].task_id, task_id);

    sink.observe(&AgentEvent::SubagentEnd {
        task_id: task_id.clone(),
        session_id: child_session_id,
        agent: "explore".to_string(),
        output: "found things".to_string(),
        success: true,
        finished_ms: 1500,
    })
    .await;

    let snap = session.subagent_task(&task_id).await.unwrap();
    assert_eq!(snap.status, SubagentStatus::Completed);
    assert_eq!(snap.success, Some(true));
    assert_eq!(snap.output.as_deref(), Some("found things"));
    assert!(snap.finished_ms.is_some());

    assert!(session.pending_subagent_tasks().await.is_empty());
    assert_eq!(session.subagent_tasks().await.len(), 1);
}

#[tokio::test]
async fn subagent_progress_events_accumulate_in_tracker() {
    use super::runtime_events::RuntimeEventSink;
    use crate::agent::AgentEvent;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-subagent-progress", None)
        .await
        .unwrap();

    let run = session
        .run_store
        .create_run(session.session_id(), "parent prompt")
        .await;
    let sink = RuntimeEventSink::from_session(&session, &run.id);

    let task_id = "task-progress".to_string();
    let child_session_id = format!("task-run-{}", task_id);

    sink.observe(&AgentEvent::SubagentStart {
        task_id: task_id.clone(),
        session_id: child_session_id.clone(),
        parent_session_id: session.session_id().to_string(),
        agent: "explore".to_string(),
        description: "demo".to_string(),
        started_ms: 0,
    })
    .await;

    sink.observe(&AgentEvent::SubagentProgress {
        task_id: task_id.clone(),
        session_id: child_session_id.clone(),
        status: "tool_completed".to_string(),
        metadata: serde_json::json!({ "tool": "bash", "exit_code": 0 }),
    })
    .await;

    sink.observe(&AgentEvent::SubagentProgress {
        task_id: task_id.clone(),
        session_id: child_session_id.clone(),
        status: "turn_completed".to_string(),
        metadata: serde_json::json!({ "turn": 1, "total_tokens": 50 }),
    })
    .await;

    let snap = session.subagent_task(&task_id).await.unwrap();
    assert_eq!(snap.progress.len(), 2);
    assert_eq!(snap.progress[0].status, "tool_completed");
    assert_eq!(snap.progress[1].status, "turn_completed");
    assert_eq!(snap.progress[1].metadata["total_tokens"], 50);
}

#[tokio::test]
async fn subagent_tasks_scope_to_parent_session() {
    use super::runtime_events::RuntimeEventSink;
    use crate::agent::AgentEvent;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session_a = agent
        .session_async("/tmp/test-ws-subagent-a", None)
        .await
        .unwrap();
    let session_b = agent
        .session_async("/tmp/test-ws-subagent-b", None)
        .await
        .unwrap();

    let run = session_a
        .run_store
        .create_run(session_a.session_id(), "p")
        .await;
    let sink = RuntimeEventSink::from_session(&session_a, &run.id);

    sink.observe(&AgentEvent::SubagentStart {
        task_id: "task-from-a".to_string(),
        session_id: "task-run-task-from-a".to_string(),
        parent_session_id: session_a.session_id().to_string(),
        agent: "explore".to_string(),
        description: "isolated".to_string(),
        started_ms: 0,
    })
    .await;

    // session A sees the task; session B has its own (empty) tracker.
    assert_eq!(session_a.subagent_tasks().await.len(), 1);
    assert!(session_b.subagent_tasks().await.is_empty());
    assert!(session_b.subagent_task("task-from-a").await.is_none());
}

#[tokio::test]
async fn cancel_subagent_task_marks_snapshot_cancelled() {
    use super::runtime_events::RuntimeEventSink;
    use crate::agent::AgentEvent;
    use crate::subagent_task_tracker::SubagentStatus;
    use tokio_util::sync::CancellationToken;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-subagent-cancel", None)
        .await
        .unwrap();
    let run = session
        .run_store
        .create_run(session.session_id(), "parent")
        .await;
    let sink = RuntimeEventSink::from_session(&session, &run.id);

    let task_id = "task-to-cancel".to_string();
    sink.observe(&AgentEvent::SubagentStart {
        task_id: task_id.clone(),
        session_id: format!("task-run-{}", task_id),
        parent_session_id: session.session_id().to_string(),
        agent: "explore".to_string(),
        description: "long task".to_string(),
        started_ms: 0,
    })
    .await;

    // Simulate what TaskExecutor would do: register a cancellation token
    // for this in-flight task so the public API has something to fire.
    let token = CancellationToken::new();
    session
        .subagent_tasks
        .register_canceller(&task_id, token.clone())
        .await;

    assert!(session.cancel_subagent_task(&task_id).await);
    assert!(token.is_cancelled());

    let snap = session.subagent_task(&task_id).await.unwrap();
    assert_eq!(snap.status, SubagentStatus::Cancelled);

    // A late SubagentEnd from the cancelled child must not downgrade.
    sink.observe(&AgentEvent::SubagentEnd {
        task_id: task_id.clone(),
        session_id: format!("task-run-{}", task_id),
        agent: "explore".to_string(),
        output: "Task cancelled by caller".to_string(),
        success: false,
        finished_ms: 0,
    })
    .await;
    let snap = session.subagent_task(&task_id).await.unwrap();
    assert_eq!(snap.status, SubagentStatus::Cancelled);

    // Cancelling again or against an unknown id is a no-op.
    assert!(!session.cancel_subagent_task(&task_id).await);
    assert!(!session.cancel_subagent_task("task-unknown").await);
}

/// Regression: `agent_executor()` must install the same `ChildRunContext` the
/// model-driven `task` path uses, so orchestrated/scripted steps inherit the
/// session's governance instead of running under weaker ambient authority.
/// Before the fix the executor was built without `.with_parent_context(..)`.
#[tokio::test]
async fn test_agent_executor_inherits_parent_run_context() {
    use crate::security::DefaultSecurityProvider;
    use crate::skills::SkillRegistry;

    let agent = Agent::from_config(test_config()).await.unwrap();

    let security: Arc<dyn crate::security::SecurityProvider> =
        Arc::new(DefaultSecurityProvider::new());
    let skills = Arc::new(SkillRegistry::new());
    let permission_policy = crate::permissions::PermissionPolicy::new().allow("web_fetch(*)");
    let opts = SessionOptions::new()
        .with_security_provider(Arc::clone(&security))
        .with_skill_registry(Arc::clone(&skills))
        .with_llm_api_timeout(45_000)
        .with_duplicate_tool_call_threshold(9)
        .with_confirmation_policy(crate::hitl::ConfirmationPolicy::enabled())
        .with_permission_policy(permission_policy);

    let session = agent
        .session_async("/tmp/test-workspace", Some(opts))
        .await
        .unwrap();
    let runtime_budget: Arc<dyn crate::budget::BudgetGuard> =
        Arc::new(DenyingBudgetGuard::default());
    session
        .set_budget_guard(Some(Arc::clone(&runtime_budget)))
        .unwrap();
    session
        .add_skill(Arc::new(crate::skills::Skill {
            name: "live-use-capability".to_string(),
            description: "Live Use capability".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: crate::skills::SkillKind::Instruction,
            content: "Use the attached capability.".to_string(),
            tags: Vec::new(),
            version: None,
        }))
        .unwrap();
    let ctx = session.parent_run_context();

    assert!(
        ctx.security_provider.is_some(),
        "security provider must propagate to delegated/orchestrated child runs"
    );
    assert!(
        ctx.skill_registry.is_some(),
        "skill registry (skill restrictions) must propagate to child runs"
    );
    let inherited_skills = ctx
        .skill_registry
        .as_ref()
        .expect("inherited effective skill registry");
    assert!(Arc::ptr_eq(
        inherited_skills,
        &session.close_handle.skill_registry
    ));
    assert!(inherited_skills.get("live-use-capability").is_some());
    assert!(
        ctx.permission_checker.is_some(),
        "permission checker must propagate to child runs"
    );
    assert!(
        ctx.permission_policy.is_some(),
        "serializable permission policy must propagate to child runs"
    );
    assert!(
        ctx.confirmation_manager.is_some(),
        "confirmation manager built from policy must propagate to child runs"
    );
    assert!(
        ctx.workspace_services.is_some(),
        "workspace services must propagate so child tools share the workspace"
    );
    assert_eq!(ctx.llm_api_timeout_ms, Some(45_000));
    assert_eq!(ctx.duplicate_tool_call_threshold, Some(9));
    let expected_hook: Arc<dyn crate::hooks::HookExecutor> = session.hook_engine.clone();
    assert!(Arc::ptr_eq(
        ctx.hook_engine.as_ref().expect("inherited hook executor"),
        &expected_hook
    ));
    assert!(Arc::ptr_eq(
        ctx.budget_guard.as_ref().expect("inherited runtime budget"),
        &runtime_budget
    ));
}

/// A session-bound executor may outlive the `AgentSession` value that created
/// it. Closing the session must therefore remain an admission boundary for
/// orchestrated work, not just for `send()`/`stream()` calls.
#[tokio::test]
async fn test_agent_executor_created_before_close_rejects_new_steps() {
    use crate::orchestration::AgentStepSpec;

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-agent-executor-close-boundary".into(),
            Arc::new(StaticStreamingClient::new("must not run after close")),
            &SessionOptions::new(),
        )
        .unwrap();
    let executor = session.agent_executor();
    let (event_tx, mut event_rx) = tokio::sync::broadcast::channel(8);

    session.close().await;

    let outcome = executor
        .execute_step(
            AgentStepSpec::new(
                "after-close",
                "general",
                "close admission regression",
                "This child must never start.",
            ),
            Some(event_tx),
        )
        .await;

    assert!(!outcome.success);
    assert!(
        outcome.output.to_ascii_lowercase().contains("cancel"),
        "closed-session failure should explain the cancellation: {}",
        outcome.output
    );
    assert!(
        matches!(
            event_rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
                | Err(tokio::sync::broadcast::error::TryRecvError::Closed)
        ),
        "a rejected child must not emit SubagentStart"
    );
}

#[tokio::test]
async fn runtime_budget_guard_refreshes_the_registered_task_tool() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_worker_agent(crate::subagent::WorkerAgentSpec::planner(
        "budgeted-child",
        "Exercise delegated budget inheritance",
    ));
    let session = agent
        .build_session(
            "/tmp/test-runtime-budget-delegation".into(),
            Arc::new(StaticStreamingClient::new("must not reach the provider")),
            &opts,
        )
        .unwrap();
    let runtime_budget = Arc::new(DenyingBudgetGuard::default());
    session
        .set_budget_guard(Some(
            Arc::clone(&runtime_budget) as Arc<dyn crate::budget::BudgetGuard>
        ))
        .unwrap();

    let result = session
        .tool(
            "task",
            serde_json::json!({
                "agent": "budgeted-child",
                "description": "budget check",
                "prompt": "Return a short answer."
            }),
        )
        .await
        .unwrap();

    assert_ne!(result.exit_code, 0, "delegated run must be denied");
    assert_eq!(
        runtime_budget
            .checks
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the runtime-installed guard must govern the child provider attempt"
    );
    assert_eq!(
        runtime_budget
            .llm_records
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a denied child call must not record provider usage"
    );
}

#[tokio::test]
async fn runtime_budget_guard_refreshes_the_registered_skill_tool() {
    use crate::skills::{Skill, SkillKind, SkillRegistry};

    let skills = Arc::new(SkillRegistry::new());
    skills.register_unchecked(Arc::new(Skill {
        name: "budgeted-skill".to_string(),
        description: "Exercise runtime budget inheritance".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Return a short answer.".to_string(),
        tags: Vec::new(),
        version: None,
    }));

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-runtime-budget-skill".into(),
            Arc::new(StaticStreamingClient::new("must not reach the provider")),
            &SessionOptions::new().with_skill_registry(skills),
        )
        .unwrap();
    let runtime_budget = Arc::new(DenyingBudgetGuard::default());
    session
        .set_budget_guard(Some(
            Arc::clone(&runtime_budget) as Arc<dyn crate::budget::BudgetGuard>
        ))
        .unwrap();

    let result = session
        .tool("Skill", serde_json::json!({"skill_name": "budgeted-skill"}))
        .await
        .unwrap();

    assert_ne!(result.exit_code, 0, "skill child run must be denied");
    assert_eq!(
        runtime_budget
            .checks
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the runtime-installed guard must govern the skill provider attempt"
    );
    assert_eq!(
        runtime_budget
            .llm_records
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test]
async fn delegated_child_run_publishes_events_to_the_parent_hook_executor() {
    let hook = Arc::new(RecordingRuntimeHook::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new()
        .with_hook_executor(Arc::clone(&hook) as Arc<dyn crate::hooks::HookExecutor>)
        .with_worker_agent(crate::subagent::WorkerAgentSpec::planner(
            "hooked-child",
            "Exercise delegated hook inheritance",
        ));
    let session = agent
        .build_session(
            "/tmp/test-delegated-hook-inheritance".into(),
            Arc::new(StaticStreamingClient::new("child answer")),
            &opts,
        )
        .unwrap();

    let result = session
        .tool(
            "task",
            serde_json::json!({
                "agent": "hooked-child",
                "description": "hook check",
                "prompt": "Return a short answer."
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        result.exit_code, 0,
        "delegated run failed: {}",
        result.output
    );

    let events = hook.hook_events.lock().unwrap();
    assert!(events.iter().any(|event| {
        event.session_id().starts_with("task-run-task-")
            && matches!(event, crate::hooks::HookEvent::GenerateStart(_))
    }));
    assert!(events.iter().any(|event| {
        event.session_id().starts_with("task-run-task-")
            && matches!(event, crate::hooks::HookEvent::GenerateEnd(_))
    }));
}

#[tokio::test]
async fn test_registered_parallel_task_inherits_final_confirmation_manager() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "hello from parent context").unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "read-1",
            "read",
            serde_json::json!({"file_path": "note.txt"}),
        ),
        scripted_text_response("child read complete"),
        scripted_text_response("control child complete"),
    ]));
    let worker = crate::subagent::WorkerAgentSpec::custom(
        "needs-parent-hitl",
        "Uses parent HITL for Ask decisions",
    )
    .with_confirmation(crate::subagent::ConfirmationInheritance::InheritParent)
    .with_max_steps(3);
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_worker_agent(worker)
        .with_max_parallel_tasks(1)
        .with_confirmation_policy(crate::hitl::ConfirmationPolicy::enabled());
    let session = agent
        .session_async(dir.path().to_string_lossy().to_string(), Some(opts))
        .await
        .unwrap();

    let (_rx, join) = session.tool_with_events(
        "parallel_task",
        serde_json::json!({
            "tasks": [
                {
                    "agent": "needs-parent-hitl",
                    "description": "Read a note",
                    "prompt": "Read note.txt",
                    "max_steps": 3
                },
                {
                    "agent": "needs-parent-hitl",
                    "description": "Independent control",
                    "prompt": "Return a short control result.",
                    "max_steps": 1
                }
            ]
        }),
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut approved = false;
    while !join.is_finished() && std::time::Instant::now() < deadline {
        let pending = session.pending_confirmations().await;
        if let Some(request) = pending.first() {
            assert_eq!(request.tool_name, "read");
            assert!(session
                .confirm_tool_use(&request.tool_id, true, Some("test approval".to_string()))
                .await
                .unwrap());
            approved = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    assert!(
        approved,
        "child run should surface a parent HITL confirmation"
    );
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), join)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        result.exit_code, 0,
        "parallel_task output: {}",
        result.output
    );
    assert!(!result.output.contains("requires confirmation but no HITL"));
    assert!(!result.output.contains("Permission denied"));
}

#[tokio::test]
async fn test_dynamic_workflow_parallel_explore_can_use_readonly_web_tools() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "fetch-1",
            "web_fetch",
            serde_json::json!({"url": "not-a-url"}),
        ),
        scripted_text_response("explore web fetch completed"),
        scripted_text_response("independent web review completed"),
    ]));
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_max_parallel_tasks(1)
        .with_manual_delegation_enabled(true);
    let session = agent
        .session_async(dir.path().to_string_lossy().to_string(), Some(opts))
        .await
        .unwrap();
    session.register_dynamic_workflow_runtime().unwrap();

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const fanout = inputs.step_outputs.web_research;
    if (fanout) {
      return { type: "complete", output: { fanout } };
    }
    return {
      type: "schedule_step",
      step_id: "web_research",
      step_name: "parallel_task",
      input: {
        tasks: [
          {
            agent: "explore",
            description: "Fetch web evidence",
            prompt: "Use web_fetch on the requested URL and summarize the result.",
            max_steps: 3,
          },
          {
            agent: "explore",
            description: "Review web evidence",
            prompt: "Return a short independent review.",
            max_steps: 1,
          },
        ],
      },
      retry: { max_attempts: 1, delay_ms: 0 },
    };
  }

  return { error: "parallel_task should run as a host step" };
}
"#;

    let (_rx, join) = session.tool_with_events(
        "dynamic_workflow",
        serde_json::json!({
            "source": source,
            "run_id": "test-dynamic-workflow-explore-web",
            "allowed_tools": [],
        }),
    );

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), join)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(
        result.exit_code, 0,
        "dynamic workflow output: {}",
        result.output
    );
    assert!(
        result.output.contains("explore web fetch completed"),
        "{}",
        result.output
    );
    assert!(
        !result.output.contains("Permission denied"),
        "web_fetch must be permitted for read-only explore research: {}",
        result.output
    );
}

#[tokio::test]
async fn test_dynamic_workflow_parallel_deep_research_inherits_parent_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let workspace_fs: Arc<dyn crate::workspace::WorkspaceFileSystem> =
        Arc::new(TestWorkspaceFs::default());
    let runner = Arc::new(TestWorkspaceRunner::default());
    let runner_backend: Arc<dyn crate::workspace::WorkspaceCommandRunner> = runner.clone();
    let services = crate::workspace::WorkspaceServices::builder(
        crate::workspace::WorkspaceRef::new(
            "deep-research-permission-inheritance",
            dir.path().to_string_lossy(),
        ),
        workspace_fs,
    )
    .command_runner(runner_backend)
    .build();
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "bash-1",
            "bash",
            serde_json::json!({"command": "echo inherited-dynamic-workflow-deep-research"}),
        ),
        scripted_text_response("deep-research child bash completed"),
        scripted_text_response("deep-research control child completed"),
    ]));
    let policy = crate::permissions::PermissionPolicy::new().allow("bash(*)");
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_permission_policy(policy)
        .with_workspace_backend(services)
        .with_max_parallel_tasks(1)
        .with_manual_delegation_enabled(true);
    let session = agent
        .session_async(dir.path().to_string_lossy().to_string(), Some(opts))
        .await
        .unwrap();
    session.register_dynamic_workflow_runtime().unwrap();

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const fanout = inputs.step_outputs.deep_research;
    if (fanout) {
      return { type: "complete", output: { fanout } };
    }
    return {
      type: "schedule_step",
      step_id: "deep_research",
      step_name: "parallel_task",
      input: {
        tasks: [
          {
            agent: "deep-research",
            description: "Use inherited parent tool policy",
            prompt: "Run the harmless bash command requested by this regression test.",
            max_steps: 3,
          },
          {
            agent: "deep-research",
            description: "Verify inherited policy independently",
            prompt: "Return a short control result.",
            max_steps: 1,
          },
        ],
      },
      retry: { max_attempts: 1, delay_ms: 0 },
    };
  }

  return { error: "parallel_task should run as a host step" };
}
"#;

    let (_rx, join) = session.tool_with_events(
        "dynamic_workflow",
        serde_json::json!({
            "source": source,
            "run_id": "test-dynamic-workflow-deep-research-inherits-permissions",
            "allowed_tools": [],
        }),
    );

    let result = tokio::time::timeout(std::time::Duration::from_secs(15), join)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(
        result.exit_code, 0,
        "dynamic workflow output: {}",
        result.output
    );
    assert!(
        result.output.contains("deep-research child bash completed"),
        "{}",
        result.output
    );
    assert!(
        !result.output.contains("Permission denied"),
        "deep-research must inherit the parent permission checker: {}",
        result.output
    );
    assert!(
        !result.output.contains("requires confirmation but no HITL"),
        "deep-research must inherit the parent confirmation context: {}",
        result.output
    );
    assert_eq!(
        runner.commands.read().unwrap().as_slice(),
        ["echo inherited-dynamic-workflow-deep-research"],
        "the child must execute through the parent workspace runner"
    );
}

#[tokio::test]
async fn test_dynamic_workflow_parallel_deep_research_inherits_parent_write_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("child-evidence.txt");
    let agent = Agent::from_config(test_config()).await.unwrap();
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "write-1",
            "write",
            serde_json::json!({
                "file_path": "child-evidence.txt",
                "content": "deep-research child inherited write permission\n"
            }),
        ),
        scripted_text_response("deep-research child write completed"),
        scripted_text_response("deep-research write control completed"),
    ]));
    let policy = crate::permissions::PermissionPolicy::new().allow("write(*)");
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_permission_policy(policy)
        .with_max_parallel_tasks(1)
        .with_manual_delegation_enabled(true);
    let session = agent
        .session_async(dir.path().to_string_lossy().to_string(), Some(opts))
        .await
        .unwrap();
    session.register_dynamic_workflow_runtime().unwrap();

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const fanout = inputs.step_outputs.deep_research;
    if (fanout) {
      return { type: "complete", output: { fanout } };
    }
    return {
      type: "schedule_step",
      step_id: "deep_research",
      step_name: "parallel_task",
      input: {
        tasks: [
          {
            agent: "deep-research",
            description: "Use inherited parent write policy",
            prompt: "Write the requested child evidence file.",
            max_steps: 3,
          },
          {
            agent: "deep-research",
            description: "Verify inherited write policy independently",
            prompt: "Return a short control result.",
            max_steps: 1,
          },
        ],
      },
      retry: { max_attempts: 1, delay_ms: 0 },
    };
  }

  return { error: "parallel_task should run as a host step" };
}
"#;

    let (_rx, join) = session.tool_with_events(
        "dynamic_workflow",
        serde_json::json!({
            "source": source,
            "run_id": "test-dynamic-workflow-deep-research-inherits-write-permissions",
            "allowed_tools": [],
        }),
    );

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), join)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    assert_eq!(
        result.exit_code, 0,
        "dynamic workflow output: {}",
        result.output
    );
    assert!(
        result
            .output
            .contains("deep-research child write completed"),
        "{}",
        result.output
    );
    assert!(
        !result.output.contains("Permission denied"),
        "deep-research must inherit parent write permissions: {}",
        result.output
    );
    assert_eq!(
        std::fs::read_to_string(target).unwrap(),
        "deep-research child inherited write permission\n"
    );
}

/// `AgentSession::workflow()` must pre-wire a shared budget ledger and a stable,
/// session-derived root id (so phase checkpoints resume across runs).
#[tokio::test]
async fn test_session_workflow_is_prewired_with_budget_and_stable_root_id() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-workspace", None)
        .await
        .unwrap();

    let wf = session.workflow();
    // An uncapped workflow still owns a ledger (for snapshots / aggregation).
    let snap = wf
        .budget_snapshot()
        .expect("workflow is pre-wired with a shared budget ledger");
    assert_eq!(snap.limit_tokens, None);
    assert_eq!(snap.consumed_tokens, 0);
    assert!(
        wf.root_id().contains(session.id()),
        "root id is session-derived so phase checkpoints are stable across runs"
    );

    // A capped workflow records its hard ceiling.
    let capped = session.workflow_with_token_budget(Some(50_000));
    assert_eq!(capped.budget_snapshot().unwrap().limit_tokens, Some(50_000));
}

/// End-to-end: `session.workflow().agent(spec)` actually spawns a real child
/// agent loop through the wired executor and returns its output. Uses a static
/// mock LLM so the built-in `explore` agent finishes with that text.
#[tokio::test]
async fn test_session_workflow_runs_a_real_child_agent_step() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new();
    let session = agent
        .build_session(
            "/tmp/test-workflow-e2e".into(),
            Arc::new(StaticStreamingClient::new("explored answer")),
            &opts,
        )
        .unwrap();

    let wf = session.workflow();
    let outcome = wf
        .agent(crate::orchestration::AgentStepSpec::new(
            "t1",
            "explore",
            "explore",
            "find the auth code",
        ))
        .await;

    assert!(outcome.success, "child step failed: {}", outcome.output);
    assert_eq!(outcome.agent, "explore");
    assert!(
        outcome.output.contains("explored answer"),
        "child agent returned the mock LLM output; got: {}",
        outcome.output
    );

    // The shared ledger recorded the child's token usage (proves the workflow
    // budget was installed as the child's budget guard).
    assert!(
        wf.budget_snapshot().unwrap().consumed_tokens > 0,
        "child LLM usage fed the shared workflow budget"
    );
}

#[tokio::test]
async fn test_session_workflow_scopes_explicit_host_tools_to_its_children() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tool: Arc<dyn crate::tools::Tool> = Arc::new(CountingScopedWorkflowTool {
        calls: Arc::clone(&calls),
    });
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "scoped-evidence-1",
            "scoped_workflow_evidence",
            serde_json::json!({}),
        ),
        scripted_text_response("scoped workflow evidence reviewed"),
    ]));
    let mut parent_permissions =
        crate::permissions::PermissionPolicy::new().allow("scoped_workflow_evidence(*)");
    parent_permissions.default_decision = crate::permissions::PermissionDecision::Deny;
    let mut worker_permissions =
        crate::permissions::PermissionPolicy::new().allow("scoped_workflow_evidence(*)");
    worker_permissions.default_decision = crate::permissions::PermissionDecision::Deny;
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_permission_policy(parent_permissions)
        .with_worker_agent(
            crate::subagent::WorkerAgentSpec::custom(
                "scoped-evidence-reviewer",
                "Review only explicitly scoped host evidence.",
            )
            .with_permissions(worker_permissions)
            .with_max_steps(2),
        );
    let session = agent
        .session_async("/tmp/test-workflow-scoped-host-tools", Some(opts))
        .await
        .unwrap();

    let parent_before = session
        .tool("scoped_workflow_evidence", serde_json::json!({}))
        .await
        .unwrap();
    assert_ne!(
        parent_before.exit_code, 0,
        "a workflow-scoped tool must not be registered on its parent session"
    );

    let workflow = session.workflow_with_token_budget_and_tools(Some(1_024), vec![tool]);
    let outcome = workflow
        .agent(
            crate::orchestration::AgentStepSpec::new(
                "scoped-evidence-step",
                "scoped-evidence-reviewer",
                "Review scoped evidence",
                "Call the admitted evidence tool exactly once, then report completion.",
            )
            .with_max_steps(2),
        )
        .await;

    assert!(outcome.success, "child step failed: {}", outcome.output);
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the explicit workflow child must execute the scoped tool once"
    );
    let parent_after = session
        .tool("scoped_workflow_evidence", serde_json::json!({}))
        .await
        .unwrap();
    assert_ne!(
        parent_after.exit_code, 0,
        "a workflow-scoped tool must not escape after child execution"
    );
}

#[tokio::test]
async fn test_session_workflow_parent_permission_can_deny_scoped_host_tool() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tool: Arc<dyn crate::tools::Tool> = Arc::new(CountingScopedWorkflowTool {
        calls: Arc::clone(&calls),
    });
    let client = Arc::new(ScriptedStreamingClient::new(vec![
        scripted_tool_call_response(
            "scoped-evidence-denied-1",
            "scoped_workflow_evidence",
            serde_json::json!({}),
        ),
        scripted_text_response("permission boundary observed"),
    ]));
    let mut parent_permissions = crate::permissions::PermissionPolicy::new();
    parent_permissions.default_decision = crate::permissions::PermissionDecision::Deny;
    let mut worker_permissions =
        crate::permissions::PermissionPolicy::new().allow("scoped_workflow_evidence(*)");
    worker_permissions.default_decision = crate::permissions::PermissionDecision::Deny;
    let opts = SessionOptions::new()
        .with_llm_client(client)
        .with_permission_policy(parent_permissions)
        .with_worker_agent(
            crate::subagent::WorkerAgentSpec::custom(
                "scoped-evidence-reviewer-denied",
                "Review only explicitly scoped host evidence.",
            )
            .with_permissions(worker_permissions)
            .with_max_steps(2),
        );
    let session = agent
        .session_async(
            "/tmp/test-workflow-scoped-host-tool-parent-deny",
            Some(opts),
        )
        .await
        .unwrap();

    let workflow = session.workflow_with_token_budget_and_tools(Some(1_024), vec![tool]);
    let outcome = workflow
        .agent(
            crate::orchestration::AgentStepSpec::new(
                "scoped-evidence-denied-step",
                "scoped-evidence-reviewer-denied",
                "Review scoped evidence",
                "Attempt the admitted evidence tool exactly once, then report completion.",
            )
            .with_max_steps(2),
        )
        .await;

    assert!(
        outcome.success,
        "the child should observe the governed denial and finish: {}",
        outcome.output
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a child allow rule must not override its parent deny boundary"
    );
}

#[tokio::test]
async fn test_session_workflow_inherits_runtime_budget_guard() {
    let runtime_budget = Arc::new(DenyingBudgetGuard::default());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .build_session(
            "/tmp/test-workflow-runtime-budget".into(),
            Arc::new(StaticStreamingClient::new("must not reach the provider")),
            &SessionOptions::new(),
        )
        .unwrap();
    session
        .set_budget_guard(Some(
            Arc::clone(&runtime_budget) as Arc<dyn crate::budget::BudgetGuard>
        ))
        .unwrap();

    let outcome = session
        .workflow()
        .agent(crate::orchestration::AgentStepSpec::new(
            "runtime-budget-step",
            "explore",
            "budget check",
            "Return a short answer.",
        ))
        .await;

    assert!(!outcome.success, "runtime budget must deny workflow child");
    assert_eq!(
        runtime_budget
            .checks
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        runtime_budget
            .llm_records
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[tokio::test(flavor = "current_thread")]
async fn async_session_builder_initializes_all_async_resources_on_current_thread_runtime() {
    use base64::Engine as _;

    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let memory_dir = root.path().join("memory");
    let sessions_dir = root.path().join("sessions");
    let mcp = Arc::new(crate::mcp::manager::McpManager::new());
    let options = SessionOptions::new()
        .with_session_id("async-current-thread")
        .with_queue_config(SessionQueueConfig::default())
        .with_file_memory(&memory_dir)
        .with_file_session_store(&sessions_dir)
        .with_mcp(mcp);

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_builder(workspace.display().to_string())
        .options(options)
        .build()
        .await
        .unwrap();

    assert!(session.has_queue());
    assert!(session.memory().is_some());
    session.save().await.unwrap();
    assert!(memory_dir.is_dir());
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("async-current-thread");
    assert!(sessions_dir
        .join("v1")
        .join("sessions")
        .join(format!("id_{key}.json"))
        .is_file());
}

#[tokio::test(flavor = "current_thread")]
async fn async_session_builder_returns_typed_memory_initialization_error() {
    let root = tempfile::tempdir().unwrap();
    let blocked = root.path().join("not-a-directory");
    std::fs::write(&blocked, "file blocks directory creation").unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let error = agent
        .session_builder(root.path().display().to_string())
        .options(SessionOptions::new().with_file_memory(blocked.join("memory")))
        .build()
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::SessionInitialization {
            resource: crate::error::SessionBuildResource::MemoryStore,
            ..
        }
    ));
    assert!(error
        .to_string()
        .replace('\\', "/")
        .contains("not-a-directory/memory"));
}

#[tokio::test(flavor = "current_thread")]
async fn async_session_builder_returns_typed_trajectory_initialization_error() {
    let root = tempfile::tempdir().unwrap();
    let blocked = root.path().join("not-a-directory");
    std::fs::write(&blocked, "file blocks trajectory parent creation").unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let error = agent
        .session_builder(root.path().display().to_string())
        .options(SessionOptions::new().with_rl_trajectory(
            crate::rl_trajectory::RlTrajectoryConfig::new(blocked.join("trajectory.jsonl")),
        ))
        .build()
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::SessionInitialization {
            resource: crate::error::SessionBuildResource::RlTrajectory,
            ..
        }
    ));
    assert!(error.to_string().contains("not-a-directory"));
}

#[tokio::test(flavor = "current_thread")]
async fn sync_session_compatibility_rejects_async_resource_specs_without_panicking() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();

    let error = agent
        .session(root.path().display().to_string(), None)
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::AsyncSessionBuildRequired {
            resource: crate::error::SessionBuildResource::MemoryStore,
        }
    ));

    let memory_dir = root.path().join("memory");
    let error = agent
        .session(
            root.path().display().to_string(),
            Some(SessionOptions::new().with_file_memory(memory_dir.clone())),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::AsyncSessionBuildRequired {
            resource: crate::error::SessionBuildResource::MemoryStore,
        }
    ));
    assert!(!memory_dir.exists());

    let trajectory_path = root.path().join("trajectory.jsonl");
    let error = agent
        .session(
            root.path().display().to_string(),
            Some(SessionOptions::new().with_rl_trajectory(
                crate::rl_trajectory::RlTrajectoryConfig::new(trajectory_path.clone()),
            )),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::AsyncSessionBuildRequired {
            resource: crate::error::SessionBuildResource::RlTrajectory,
        }
    ));
    assert!(!trajectory_path.exists());
}

#[tokio::test(flavor = "current_thread")]
async fn sync_session_compatibility_accepts_preinitialized_resources() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let memory = Arc::new(a3s_memory::InMemoryStore::new());

    let session = agent
        .session(
            root.path().display().to_string(),
            Some(SessionOptions::new().with_memory(memory)),
        )
        .unwrap();

    assert!(session.memory().is_some());
    assert!(!session.has_queue());
}

#[tokio::test(flavor = "current_thread")]
async fn async_session_keeps_inherited_mcp_sources_separate_from_live_extensions() {
    use crate::mcp::{McpManager, McpServerConfig, McpTransportConfig};

    fn server(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: McpTransportConfig::Stdio {
                command: "unused".to_string(),
                args: Vec::new(),
            },
            enabled: false,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 60,
        }
    }

    let global = Arc::new(McpManager::new());
    global.register_server(server("global-source")).await;
    let configured = Arc::new(McpManager::new());
    configured
        .register_server(server("configured-source"))
        .await;

    let mut agent = Agent::from_config(test_config()).await.unwrap();
    agent.global_mcp = Some(Arc::clone(&global));
    let session = agent
        .session_async(
            "/tmp/test-mcp-source-isolation",
            Some(SessionOptions::new().with_mcp(Arc::clone(&configured))),
        )
        .await
        .unwrap();

    assert!(!Arc::ptr_eq(&session.mcp_manager, &global));
    assert!(!Arc::ptr_eq(&session.mcp_manager, &configured));
    assert_eq!(session.inherited_mcp_managers.len(), 2);
    assert!(Arc::ptr_eq(&session.inherited_mcp_managers[0], &global));
    assert!(Arc::ptr_eq(&session.inherited_mcp_managers[1], &configured));
    assert_eq!(session.mcp_managers.len(), 3);
    assert!(Arc::ptr_eq(
        session.mcp_managers.last().unwrap(),
        &session.mcp_manager
    ));

    let status = session.mcp_status().await;
    assert!(status.contains_key("global-source"));
    assert!(status.contains_key("configured-source"));
    assert!(!global.get_status().await.contains_key("configured-source"));
    assert!(!configured.get_status().await.contains_key("global-source"));
    assert!(session.mcp_manager.get_status().await.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn async_sessions_never_share_their_live_mcp_manager() {
    let inherited = Arc::new(crate::mcp::McpManager::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let first = agent
        .session_async(
            "/tmp/test-private-mcp-a",
            Some(SessionOptions::new().with_mcp(Arc::clone(&inherited))),
        )
        .await
        .unwrap();
    let second = agent
        .session_async(
            "/tmp/test-private-mcp-b",
            Some(SessionOptions::new().with_mcp(Arc::clone(&inherited))),
        )
        .await
        .unwrap();

    assert!(!Arc::ptr_eq(&first.mcp_manager, &second.mcp_manager));
    assert!(!Arc::ptr_eq(&first.mcp_manager, &inherited));
    assert!(!Arc::ptr_eq(&second.mcp_manager, &inherited));
}

#[tokio::test(flavor = "current_thread")]
async fn sync_session_uses_cached_global_mcp_without_blocking_runtime() {
    let global = Arc::new(crate::mcp::McpManager::new());
    let mut agent = Agent::from_config(test_config()).await.unwrap();
    agent.global_mcp = Some(Arc::clone(&global));

    let session = agent
        .session(
            "/tmp/test-sync-global-mcp",
            Some(SessionOptions::new().with_memory(Arc::new(a3s_memory::InMemoryStore::new()))),
        )
        .unwrap();

    assert_eq!(session.inherited_mcp_managers.len(), 1);
    assert!(Arc::ptr_eq(&session.inherited_mcp_managers[0], &global));
    assert!(!Arc::ptr_eq(&session.mcp_manager, &global));
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_global_mcp_source_is_deduplicated() {
    let global = Arc::new(crate::mcp::McpManager::new());
    let mut agent = Agent::from_config(test_config()).await.unwrap();
    agent.global_mcp = Some(Arc::clone(&global));

    let session = agent
        .session_async(
            "/tmp/test-deduplicated-global-mcp",
            Some(SessionOptions::new().with_mcp(Arc::clone(&global))),
        )
        .await
        .unwrap();

    assert_eq!(session.inherited_mcp_managers.len(), 1);
    assert_eq!(session.mcp_managers.len(), 2);
    assert!(Arc::ptr_eq(&session.inherited_mcp_managers[0], &global));
}

#[tokio::test(flavor = "current_thread")]
async fn live_mcp_add_remove_is_session_local_and_restores_inherited_precedence() {
    use crate::mcp::{McpManager, McpServerConfig, McpTransportConfig};

    fn server(name: &str, enabled: bool) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: McpTransportConfig::Stdio {
                command: "unused".to_string(),
                args: Vec::new(),
            },
            enabled,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 60,
        }
    }

    let global = Arc::new(McpManager::new());
    global.register_server(server("shared", false)).await;
    let configured = Arc::new(McpManager::new());
    configured.register_server(server("shared", true)).await;

    let mut agent = Agent::from_config(test_config()).await.unwrap();
    agent.global_mcp = Some(Arc::clone(&global));
    let session = agent
        .session_async(
            "/tmp/test-live-mcp-isolation",
            Some(SessionOptions::new().with_mcp(Arc::clone(&configured))),
        )
        .await
        .unwrap();

    let inherited_status = session.mcp_status().await;
    assert!(
        inherited_status["shared"].enabled,
        "the later configured source must override the global status"
    );

    let error = session
        .add_mcp_server(server("shared", false))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("disabled"));
    let rolled_back_status = session.mcp_status().await;
    assert!(
        rolled_back_status["shared"].enabled,
        "a failed session-local add must reveal the inherited source"
    );
    assert!(
        !session
            .mcp_manager
            .get_status()
            .await
            .contains_key("shared"),
        "connect failure must roll back local config and error state"
    );
    assert!(
        !global.get_status().await["shared"].enabled,
        "live add must not mutate the global manager"
    );
    assert!(
        configured.get_status().await["shared"].enabled,
        "live add must not mutate the configured inherited manager"
    );

    session.remove_mcp_server("shared").await.unwrap();
    let restored_status = session.mcp_status().await;
    assert!(
        restored_status["shared"].enabled,
        "removing the local shadow must reveal the configured source again"
    );
    assert!(!session
        .mcp_manager
        .get_status()
        .await
        .contains_key("shared"));
    assert!(global.get_status().await.contains_key("shared"));
    assert!(configured.get_status().await.contains_key("shared"));
}

#[tokio::test(flavor = "current_thread")]
async fn close_serializes_with_live_mcp_mutation_and_rejects_late_add_remove() {
    use crate::mcp::{McpServerConfig, McpTransportConfig};

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .session_async("/tmp/test-live-mcp-close-race", None)
            .await
            .unwrap(),
    );
    let local_tool_name = "mcp__close-owned__local";
    let shadowed_tool_name = "mcp__close-owned__shared";
    let shadowed_tool: Arc<dyn crate::tools::Tool> =
        Arc::new(NamedSessionTool(shadowed_tool_name.to_string()));
    session
        .register_dynamic_tool(Arc::clone(&shadowed_tool))
        .unwrap();
    session
        .close_handle
        .mcp_tool_ownership
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .install(
            "close-owned",
            &session.tool_executor,
            vec![
                Arc::new(NamedSessionTool(local_tool_name.to_string())),
                Arc::new(NamedSessionTool(shadowed_tool_name.to_string())),
            ],
        );
    assert!(session
        .tool_names()
        .iter()
        .any(|name| name == local_tool_name));
    let mutation = session.close_handle.extension_mutation.lock().await;

    let closing_session = Arc::clone(&session);
    let close_task = tokio::spawn(async move {
        closing_session.close().await;
    });
    while !session.is_closed() {
        tokio::task::yield_now().await;
    }

    let adding_session = Arc::clone(&session);
    let add_task = tokio::spawn(async move {
        adding_session
            .add_mcp_server(McpServerConfig {
                name: "late".to_string(),
                transport: McpTransportConfig::Stdio {
                    command: "unused".to_string(),
                    args: Vec::new(),
                },
                enabled: false,
                env: HashMap::new(),
                oauth: None,
                tool_timeout_secs: 60,
            })
            .await
    });

    drop(mutation);
    tokio::time::timeout(std::time::Duration::from_secs(2), close_task)
        .await
        .expect("close must finish after the admitted mutation releases")
        .unwrap();
    let add_error = add_task.await.unwrap().unwrap_err();
    assert!(matches!(
        add_error,
        crate::error::CodeError::SessionClosed { .. }
    ));
    assert!(session.mcp_manager.get_status().await.is_empty());
    assert!(session.mcp_manager.list_connected().await.is_empty());
    assert!(
        !session
            .tool_names()
            .iter()
            .any(|name| name == local_tool_name),
        "close must unwind wrappers owned by session-local MCP servers"
    );
    let restored_shadow = session
        .tool_executor
        .registry()
        .get(shadowed_tool_name)
        .unwrap();
    assert!(Arc::ptr_eq(&restored_shadow, &shadowed_tool));

    let remove_error = session.remove_mcp_server("late").await.unwrap_err();
    assert!(matches!(
        remove_error,
        crate::error::CodeError::SessionClosed { .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn live_mcp_remove_cleanup_failure_still_commits_registry_removal() {
    use crate::mcp::{McpClient, McpServerConfig, McpTransportConfig};

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-live-mcp-remove-cleanup", None)
        .await
        .unwrap();
    let server_name = "failing-cleanup";
    let local_tool_name = "mcp__failing-cleanup__local";
    session
        .mcp_manager
        .register_server(McpServerConfig {
            name: server_name.to_string(),
            transport: McpTransportConfig::Stdio {
                command: "unused".to_string(),
                args: Vec::new(),
            },
            enabled: true,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 60,
        })
        .await;
    session
        .mcp_manager
        .insert_client_for_test(
            server_name,
            Arc::new(McpClient::new(
                server_name.to_string(),
                Arc::new(FailingCloseSessionTransport),
            )),
        )
        .await;
    session
        .close_handle
        .mcp_tool_ownership
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .install(
            server_name,
            &session.tool_executor,
            vec![Arc::new(NamedSessionTool(local_tool_name.to_string()))],
        );

    let error = session.remove_mcp_server(server_name).await.unwrap_err();
    assert!(error.to_string().contains("transport cleanup failed"));
    assert!(!session.mcp_manager.contains_server(server_name).await);
    assert!(session.mcp_manager.get_client(server_name).await.is_none());
    assert!(!session
        .tool_names()
        .iter()
        .any(|name| name == local_tool_name));
    session.remove_mcp_server(server_name).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn blank_session_id_returns_typed_configuration_error() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let error = agent
        .session_async(
            "/tmp/test-blank-session-id",
            Some(SessionOptions::new().with_session_id("  \t")),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::SessionConfiguration {
            field: "session_id",
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn cognitive_package_binding_is_persisted_and_retained_by_run_events() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (cognitive_context, provider) = test_cognitive_context(TestCognitiveProviderMode::Valid);
    let expected = cognitive_context.binding().clone();
    let opts = SessionOptions::new()
        .with_session_id("cognitive-session")
        .with_session_store(store.clone())
        .with_llm_client(Arc::new(StaticStreamingClient::new("grounded response")))
        .with_planning(false)
        .with_cognitive_context(cognitive_context);
    let session = agent
        .session_async("/tmp/test-cognitive-session", Some(opts))
        .await
        .unwrap();

    assert_eq!(session.cognitive_package_binding(), Some(&expected));
    session
        .send("What is the retry policy?", None)
        .await
        .unwrap();
    session.save().await.unwrap();

    let requests = provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].session_id, "cognitive-session");
    assert_eq!(requests[0].binding, expected);

    let snapshot = store
        .load_snapshot("cognitive-session")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        snapshot.session.cognitive_package_binding.as_ref(),
        Some(&expected)
    );
    let bound_events = snapshot
        .run_records
        .iter()
        .flat_map(|record| &record.events)
        .filter_map(|record| match &record.event {
            AgentEvent::CognitiveContextBound { binding } => Some(binding),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(bound_events, vec![&expected]);
    assert!(!snapshot
        .run_records
        .iter()
        .flat_map(|record| &record.events)
        .any(|record| matches!(record.event, AgentEvent::MemoryRecalled { .. })));
    snapshot.validate_for_session("cognitive-session").unwrap();

    let mut tampered = snapshot.clone();
    let event_binding = tampered
        .run_records
        .iter_mut()
        .flat_map(|record| &mut record.events)
        .find_map(|record| match &mut record.event {
            AgentEvent::CognitiveContextBound { binding } => Some(binding),
            _ => None,
        })
        .unwrap();
    event_binding.limits.max_results -= 1;
    assert!(tampered.validate_for_session("cognitive-session").is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn cognitive_provider_failure_and_generation_drift_fail_closed_without_fallback() {
    for (session_id, mode) in [
        (
            "cognitive-provider-failure",
            TestCognitiveProviderMode::Fail,
        ),
        (
            "cognitive-provider-drift",
            TestCognitiveProviderMode::DriftGeneration,
        ),
    ] {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let (cognitive_context, _) = test_cognitive_context(mode);
        let opts = SessionOptions::new()
            .with_session_id(session_id)
            .with_llm_client(Arc::new(StaticStreamingClient::new(
                "this response must never be reached",
            )))
            .with_planning(false)
            .with_cognitive_context(cognitive_context);
        let session = agent
            .session_async(format!("/tmp/{session_id}"), Some(opts))
            .await
            .unwrap();

        let error = session
            .send("Use the package evidence", None)
            .await
            .expect_err("required cognitive context must not be omitted");
        assert!(error.to_string().contains("failed closed"), "{error:#}");
        let records = session.run_store.records().await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].snapshot.status, crate::run::RunStatus::Failed);
        assert!(records[0]
            .events
            .iter()
            .any(|record| matches!(record.event, AgentEvent::CognitiveContextBound { .. })));
        assert!(!records[0].events.iter().any(|record| matches!(
            record.event,
            AgentEvent::MemoryRecalled { .. } | AgentEvent::End { .. }
        )));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cognitive_session_rejects_general_rag_or_graph_provider_fallback() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (cognitive_context, _) = test_cognitive_context(TestCognitiveProviderMode::Valid);
    let error = agent
        .session_async(
            "/tmp/test-cognitive-no-generic-fallback",
            Some(
                SessionOptions::new()
                    .with_session_id("cognitive-no-generic-fallback")
                    .with_llm_client(Arc::new(StaticStreamingClient::new("unused")))
                    .with_cognitive_context(cognitive_context)
                    .with_context_provider(Arc::new(CapturingContextProvider::default())),
            ),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::SessionConfiguration {
            field: "cognitive_context",
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn cognitive_session_resume_requires_the_same_host_injected_binding() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (cognitive_context, _) = test_cognitive_context(TestCognitiveProviderMode::Valid);
    let expected = cognitive_context.binding().clone();
    let session = agent
        .session_async(
            "/tmp/test-cognitive-resume",
            Some(
                SessionOptions::new()
                    .with_session_id("cognitive-resume")
                    .with_session_store(store.clone())
                    .with_llm_client(Arc::new(StaticStreamingClient::new("unused")))
                    .with_cognitive_context(cognitive_context),
            ),
        )
        .await
        .unwrap();
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    let missing = agent
        .resume_session_async(
            "cognitive-resume",
            SessionOptions::new()
                .with_session_store(store.clone())
                .with_llm_client(Arc::new(StaticStreamingClient::new("unused"))),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        missing,
        crate::error::CodeError::SessionConfiguration {
            field: "cognitive_context",
            ..
        }
    ));

    let mut different_binding = expected.clone();
    different_binding.limits.max_results -= 1;
    different_binding.validate().unwrap();
    let drift_provider: Arc<dyn crate::cognitive_context::CognitiveContextProvider> =
        Arc::new(TestCognitiveProvider::new(TestCognitiveProviderMode::Valid));
    let drift_context =
        crate::cognitive_context::CognitiveContextSession::new(different_binding, drift_provider)
            .unwrap();
    let drift = agent
        .resume_session_async(
            "cognitive-resume",
            SessionOptions::new()
                .with_session_store(store.clone())
                .with_llm_client(Arc::new(StaticStreamingClient::new("unused")))
                .with_cognitive_context(drift_context),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        drift,
        crate::error::CodeError::SessionConfiguration {
            field: "cognitive_context",
            ..
        }
    ));

    let (exact_context, _) = test_cognitive_context(TestCognitiveProviderMode::Valid);
    let resumed = agent
        .resume_session_async(
            "cognitive-resume",
            SessionOptions::new()
                .with_session_store(store)
                .with_llm_client(Arc::new(StaticStreamingClient::new("unused")))
                .with_cognitive_context(exact_context),
        )
        .await
        .unwrap();
    assert_eq!(resumed.cognitive_package_binding(), Some(&expected));
}
