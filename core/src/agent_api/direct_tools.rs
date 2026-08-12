//! Direct host tool access.
//!
//! These helpers expose the same tool executor used by the agent loop for host
//! control-plane calls. Keeping argument shaping and result projection here
//! prevents the public session facade from duplicating tool-specific knowledge.

use super::{agent_loop_runtime::build_agent_loop, AgentSession, ReadFileOptions, ToolCallResult};
use crate::agent::{AgentEvent, AgentLoop};
use crate::error::Result;
use crate::llm::ToolDefinition;
use crate::tools::{ToolArtifact, ToolContext, ToolExecutor, ToolInvocation};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(test)]
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

mod event_stream;
#[cfg(test)]
mod governed_tests;

pub(super) struct DirectToolRuntime {
    tool_executor: Arc<ToolExecutor>,
    tool_context: ToolContext,
    agent_loop: AgentLoop,
    session_id: String,
    session_cancel: tokio_util::sync::CancellationToken,
    closed: Arc<AtomicBool>,
    task_scheduler: Option<Arc<crate::task_scheduler::TaskScheduler>>,
    task_priority: crate::task_scheduler::TaskPriority,
    security_provider: Option<Arc<dyn crate::security::SecurityProvider>>,
}

impl DirectToolRuntime {
    pub(super) fn from_session(session: &AgentSession) -> Self {
        Self {
            tool_executor: Arc::clone(&session.tool_executor),
            tool_context: session.tool_context.clone(),
            agent_loop: build_agent_loop(session),
            session_id: session.session_id.clone(),
            session_cancel: session.session_cancel.clone(),
            closed: Arc::clone(&session.closed),
            task_scheduler: Some(Arc::clone(&session.task_scheduler)),
            task_priority: session.task_priority,
            security_provider: session.config.security_provider.clone(),
        }
    }

    pub(super) fn definitions(&self) -> Vec<ToolDefinition> {
        self.tool_executor.definitions()
    }

    pub(super) fn names(&self) -> Vec<String> {
        self.tool_executor
            .definitions()
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    }

    pub(super) fn artifact(&self, artifact_uri: &str) -> Option<ToolArtifact> {
        self.tool_executor.get_artifact(artifact_uri)
    }

    pub(super) async fn read_file(&self, path: &str, options: ReadFileOptions) -> Result<String> {
        let mut args = serde_json::json!({ "file_path": path });
        if let serde_json::Value::Object(ref mut map) = args {
            if let Some(offset) = options.offset {
                map.insert("offset".to_string(), serde_json::json!(offset));
            }
            if let Some(limit) = options.limit {
                map.insert("limit".to_string(), serde_json::json!(limit));
            }
        }
        Ok(self.call("read", args).await?.output)
    }

    pub(super) async fn write_file(&self, path: &str, content: &str) -> Result<ToolCallResult> {
        let args = serde_json::json!({ "file_path": path, "content": content });
        self.call("write", args).await
    }

    pub(super) async fn ls(&self, path: Option<&str>) -> Result<ToolCallResult> {
        let args = match path {
            Some(path) => serde_json::json!({ "path": path }),
            None => serde_json::json!({}),
        };
        self.call("ls", args).await
    }

    pub(super) async fn edit_file(
        &self,
        path: &str,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> Result<ToolCallResult> {
        let args = serde_json::json!({
            "file_path": path,
            "old_string": old_string,
            "new_string": new_string,
            "replace_all": replace_all,
        });
        self.call("edit", args).await
    }

    pub(super) async fn patch_file(&self, path: &str, diff: &str) -> Result<ToolCallResult> {
        let args = serde_json::json!({ "file_path": path, "diff": diff });
        self.call("patch", args).await
    }

    pub(super) async fn bash(&self, command: &str) -> Result<String> {
        let args = serde_json::json!({ "command": command });
        Ok(self.call("bash", args).await?.output)
    }

    pub(super) async fn glob(&self, pattern: &str) -> Result<Vec<String>> {
        let args = serde_json::json!({ "mode": "glob", "query": pattern });
        let result = self.call("search", args).await?;
        Ok(parse_glob_output(&result.output))
    }

    pub(super) async fn grep(&self, pattern: &str) -> Result<String> {
        let args = serde_json::json!({ "mode": "grep", "query": pattern });
        let result = self.call("search", args).await?;
        Ok(result.output)
    }

    pub(super) async fn call(&self, name: &str, args: serde_json::Value) -> Result<ToolCallResult> {
        self.call_invocation(ToolInvocation::host_direct(
            format!("host-{name}-{}", uuid::Uuid::new_v4()),
            name,
            args,
        ))
        .await
    }

    pub(super) async fn call_governed(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<ToolCallResult> {
        self.call_invocation(ToolInvocation::host_governed(
            format!("host-governed-{name}-{}", uuid::Uuid::new_v4()),
            name,
            args,
        ))
        .await
    }

    async fn call_invocation(&self, invocation: ToolInvocation) -> Result<ToolCallResult> {
        self.ensure_open()?;
        let cancel = self.session_cancel.child_token();
        let _task_lease = acquire_task_admission(
            self.task_scheduler.as_deref(),
            self.task_priority,
            &self.session_id,
            &invocation.name,
            &cancel,
            &self.closed,
        )
        .await?;
        self.ensure_open()?;
        let result = self
            .agent_loop
            .invoke_host_tool(
                invocation,
                &self.session_id,
                &None,
                &cancel,
                &self.tool_context,
            )
            .await;
        Ok(project_tool_result(result))
    }

    fn ensure_open(&self) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(crate::error::CodeError::SessionClosed {
                session_id: self.session_id.clone(),
            });
        }
        Ok(())
    }
}

async fn acquire_task_admission(
    scheduler: Option<&crate::task_scheduler::TaskScheduler>,
    priority: crate::task_scheduler::TaskPriority,
    session_id: &str,
    operation: &str,
    cancellation: &tokio_util::sync::CancellationToken,
    closed: &AtomicBool,
) -> Result<Option<crate::task_scheduler::TaskLease>> {
    let Some(scheduler) = scheduler else {
        return Ok(None);
    };
    scheduler
        .acquire(
            priority,
            format!("{session_id}:tool:{operation}"),
            cancellation,
        )
        .await
        .map(Some)
        .map_err(|error| match error {
            crate::task_scheduler::TaskSchedulerError::Cancelled
                if closed.load(Ordering::Acquire) =>
            {
                crate::error::CodeError::SessionClosed {
                    session_id: session_id.to_string(),
                }
            }
            crate::task_scheduler::TaskSchedulerError::Cancelled => {
                crate::error::CodeError::TaskAdmissionCancelled {
                    session_id: session_id.to_string(),
                }
            }
            crate::task_scheduler::TaskSchedulerError::Closed => {
                crate::error::CodeError::TaskSchedulerClosed
            }
            crate::task_scheduler::TaskSchedulerError::InvalidConfig(message) => {
                crate::error::CodeError::Config(message)
            }
        })
}

fn project_tool_result(result: crate::tools::ToolResult) -> ToolCallResult {
    ToolCallResult {
        name: result.name,
        output: result.output,
        exit_code: result.exit_code,
        metadata: result.metadata,
        error_kind: result.error_kind,
    }
}

fn parse_glob_output(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentConfig;
    use crate::budget::{BudgetDecision, BudgetGuard};
    use crate::hooks::{HookEvent, HookExecutor, HookResult};
    use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
    use crate::permissions::{PermissionChecker, PermissionDecision};
    use crate::queue::SessionQueueConfig;
    use crate::security::SecurityProvider;
    use crate::session_lane_queue::SessionLaneQueue;
    use crate::subagent::{AgentRegistry, WorkerAgentSpec};
    use crate::tools::{register_task, Tool, ToolErrorKind, ToolOutput, ToolStreamEvent};
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    struct ContextProbeTool;
    struct StreamingProbeTool;
    struct SensitiveStreamingProbeTool;
    struct DirectToolTestLlm;
    struct BlockingBackgroundLlm {
        started: Arc<Notify>,
        release: Arc<Notify>,
    }
    struct DenyAllTools;
    struct DenyToolBudget;
    struct RedactingSecurity;

    #[derive(Debug, Default)]
    struct BlockingPreToolHook {
        calls: AtomicUsize,
    }

    impl PermissionChecker for DenyAllTools {
        fn check(&self, _tool_name: &str, _args: &serde_json::Value) -> PermissionDecision {
            PermissionDecision::Deny
        }
    }

    #[async_trait]
    impl BudgetGuard for DenyToolBudget {
        async fn check_before_tool(&self, _session_id: &str, _tool_name: &str) -> BudgetDecision {
            BudgetDecision::Deny {
                resource: "tool_calls".to_string(),
                reason: "direct tool budget exhausted".to_string(),
            }
        }
    }

    impl SecurityProvider for RedactingSecurity {
        fn sanitize_output(&self, _text: &str) -> String {
            "[sanitized]".to_string()
        }
    }

    #[async_trait]
    impl HookExecutor for BlockingPreToolHook {
        async fn fire(&self, event: &HookEvent) -> HookResult {
            if matches!(event, HookEvent::PreToolUse(_)) {
                self.calls.fetch_add(1, Ordering::SeqCst);
                HookResult::Block("blocked direct tool in test".to_string())
            } else {
                HookResult::Continue(None)
            }
        }
    }

    #[async_trait]
    impl LlmClient for DirectToolTestLlm {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<LlmResponse> {
            anyhow::bail!("direct tool tests do not call the model")
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
            anyhow::bail!("direct tool tests do not call the model")
        }
    }

    impl BlockingBackgroundLlm {
        fn response() -> LlmResponse {
            LlmResponse {
                message: Message::assistant("background complete"),
                usage: Default::default(),
                stop_reason: Some("end_turn".to_string()),
                token_logprobs: Vec::new(),
                meta: None,
            }
        }
    }

    #[async_trait]
    impl LlmClient for BlockingBackgroundLlm {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<LlmResponse> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(Self::response())
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
            let (tx, rx) = mpsc::channel(1);
            let started = Arc::clone(&self.started);
            let release = Arc::clone(&self.release);
            tokio::spawn(async move {
                started.notify_one();
                release.notified().await;
                tx.send(StreamEvent::Done(Self::response())).await.ok();
            });
            Ok(rx)
        }
    }

    fn direct_runtime(
        tool_executor: Arc<ToolExecutor>,
        tool_context: ToolContext,
    ) -> DirectToolRuntime {
        direct_runtime_with_config(tool_executor, tool_context, AgentConfig::default())
    }

    fn direct_runtime_with_config(
        tool_executor: Arc<ToolExecutor>,
        tool_context: ToolContext,
        config: AgentConfig,
    ) -> DirectToolRuntime {
        let session_id = tool_context
            .session_id
            .clone()
            .unwrap_or_else(|| "test-session".to_string());
        let security_provider = config.security_provider.clone();
        let agent_loop = AgentLoop::new(
            Arc::new(DirectToolTestLlm),
            Arc::clone(&tool_executor),
            tool_context.clone(),
            config,
        );
        DirectToolRuntime {
            tool_executor,
            tool_context,
            agent_loop,
            session_id,
            session_cancel: tokio_util::sync::CancellationToken::new(),
            closed: Arc::new(AtomicBool::new(false)),
            task_scheduler: None,
            task_priority: crate::task_scheduler::TaskPriority::Interactive,
            security_provider,
        }
    }

    struct CountingTool {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Tool for CountingTool {
        fn name(&self) -> &str {
            "counting"
        }

        fn description(&self) -> &str {
            "records one side effect"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::success("counted"))
        }
    }

    struct BlockingTool;

    #[async_trait]
    impl Tool for BlockingTool {
        fn name(&self) -> &str {
            "blocking"
        }

        fn description(&self) -> &str {
            "waits forever unless the invocation gateway cancels it"
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }

    #[async_trait]
    impl Tool for ContextProbeTool {
        fn name(&self) -> &str {
            "context_probe"
        }

        fn description(&self) -> &str {
            "Reports direct tool context for tests."
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            Ok(ToolOutput::success(
                ctx.session_id.as_deref().unwrap_or("missing-session"),
            ))
        }
    }

    #[async_trait]
    impl Tool for StreamingProbeTool {
        fn name(&self) -> &str {
            "streaming_probe"
        }

        fn description(&self) -> &str {
            "Streams direct tool progress for tests."
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            if let Some(tx) = &ctx.event_tx {
                let _ = tx
                    .send(ToolStreamEvent::OutputDelta(
                        "submitted 2 tasks\n".to_string(),
                    ))
                    .await;
                let _ = tx
                    .send(ToolStreamEvent::OutputDelta("2/2 done\n".to_string()))
                    .await;
            }
            Ok(ToolOutput::success("complete"))
        }
    }

    #[async_trait]
    impl Tool for SensitiveStreamingProbeTool {
        fn name(&self) -> &str {
            "sensitive_streaming_probe"
        }

        fn description(&self) -> &str {
            "Streams sensitive direct tool progress for tests."
        }

        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }

        async fn execute(
            &self,
            _args: &serde_json::Value,
            ctx: &ToolContext,
        ) -> Result<ToolOutput> {
            if let Some(tx) = &ctx.event_tx {
                tx.send(ToolStreamEvent::OutputDelta("progress user@".to_string()))
                    .await
                    .ok();
                tx.send(ToolStreamEvent::OutputDelta("example.com".to_string()))
                    .await
                    .ok();
            }
            Ok(ToolOutput::success("result user@example.com"))
        }
    }

    #[test]
    fn parse_glob_output_ignores_empty_lines() {
        assert_eq!(
            parse_glob_output("src/lib.rs\n\nsrc/main.rs\n"),
            vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]
        );
    }

    #[tokio::test]
    async fn direct_tool_call_uses_session_tool_context() {
        let dir = tempfile::tempdir().unwrap();
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(ContextProbeTool));
        let runtime = direct_runtime(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-123"),
        );

        let output = runtime
            .call("context_probe", serde_json::json!({}))
            .await
            .unwrap();

        assert_eq!(output.output, "session-123");
    }

    #[tokio::test]
    async fn direct_read_file_passes_offset_and_limit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "one\ntwo\nthree\nfour\n").unwrap();
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        let runtime = direct_runtime(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-123"),
        );

        let output = runtime
            .read_file(
                "notes.txt",
                ReadFileOptions {
                    offset: Some(1),
                    limit: Some(2),
                },
            )
            .await
            .unwrap();

        assert!(output.contains("two"));
        assert!(output.contains("three"));
        assert!(!output.contains("one"));
        assert!(!output.contains("four"));
    }

    #[tokio::test]
    async fn direct_tool_call_with_events_forwards_tool_stream_deltas() {
        let dir = tempfile::tempdir().unwrap();
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(StreamingProbeTool));
        let runtime = direct_runtime(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-123"),
        );

        let (mut rx, join) = runtime
            .spawn_call_with_agent_events("streaming_probe".to_string(), serde_json::json!({}));
        let output = join.await.unwrap().unwrap();
        assert_eq!(output.output, "complete");

        let mut call_id = None;
        let mut saw_preparation = false;
        let mut execution_args = None;
        let mut completed_args = None;
        let mut deltas = Vec::new();
        while let Ok(Some(event)) =
            tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await
        {
            match event {
                AgentEvent::ToolStart { id, name } => {
                    saw_preparation = true;
                    assert_eq!(name, "streaming_probe");
                    call_id = Some(id);
                }
                AgentEvent::ToolExecutionStart { id, name, args } => {
                    assert_eq!(name, "streaming_probe");
                    assert!(call_id.replace(id).is_none());
                    execution_args = Some(args);
                }
                AgentEvent::ToolOutputDelta { id, name, delta } => {
                    assert_eq!(name, "streaming_probe");
                    assert_eq!(Some(id.as_str()), call_id.as_deref());
                    deltas.push(delta);
                }
                AgentEvent::ToolEnd { id, name, args, .. } => {
                    assert_eq!(name, "streaming_probe");
                    assert_eq!(Some(id.as_str()), call_id.as_deref());
                    completed_args = args;
                }
                _ => {}
            }
        }
        assert!(call_id.is_some());
        assert!(
            !saw_preparation,
            "host-direct calls do not have model-side preparation"
        );
        assert_eq!(execution_args, Some(serde_json::json!({})));
        assert_eq!(completed_args, Some(serde_json::json!({})));
        assert_eq!(deltas.join(""), "submitted 2 tasks\n2/2 done\n");
    }

    #[tokio::test]
    async fn direct_tool_events_are_sanitized_and_end_after_all_deltas() {
        let dir = tempfile::tempdir().unwrap();
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(SensitiveStreamingProbeTool));
        let runtime = direct_runtime_with_config(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-123"),
            AgentConfig {
                security_provider: Some(Arc::new(crate::security::DefaultSecurityProvider::new())),
                ..AgentConfig::default()
            },
        );

        let (mut rx, join) = runtime.spawn_call_with_agent_events(
            "sensitive_streaming_probe".to_string(),
            serde_json::json!({"contact": "user@example.com"}),
        );
        let output = join.await.unwrap().unwrap();
        assert!(!output.output.contains("user@example.com"));

        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        assert!(matches!(events.last(), Some(AgentEvent::ToolEnd { .. })));
        assert!(events
            .iter()
            .any(|event| matches!(event, AgentEvent::ToolOutputDelta { .. })));
        let streamed_output = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolOutputDelta { delta, .. } => Some(delta.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert!(streamed_output.contains("REDACTED:EMAIL"));
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            assert!(
                !json.contains("user@example.com"),
                "unsanitized event: {json}"
            );
        }
    }

    #[tokio::test]
    async fn direct_background_task_does_not_hold_event_bridge_open() {
        let dir = tempfile::tempdir().unwrap();
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let child_llm = Arc::new(BlockingBackgroundLlm {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        });
        let registry = AgentRegistry::new();
        registry.register(
            WorkerAgentSpec::custom("worker", "Blocking background worker")
                .with_prompt("Wait for the test gate, then finish.")
                .with_max_steps(1)
                .into_agent_definition(),
        );
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        register_task(
            tool_executor.registry(),
            child_llm,
            Arc::new(registry),
            dir.path().to_string_lossy().to_string(),
        );
        let runtime = direct_runtime(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-background"),
        );

        let child_started = started.notified();
        let (mut rx, join) = runtime.spawn_call_with_agent_events(
            "task".to_string(),
            serde_json::json!({
                "agent": "worker",
                "description": "block in background",
                "prompt": "wait for release",
                "background": true
            }),
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), child_started)
            .await
            .expect("background child should enter its blocked model call");

        let output = tokio::time::timeout(std::time::Duration::from_secs(1), join)
            .await
            .expect("direct tool lifecycle must not wait for a detached background child")
            .unwrap()
            .unwrap();
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        release.notify_one();

        assert!(output.output.contains("Task started in background"));
        assert!(events
            .iter()
            .any(|event| matches!(event, AgentEvent::SubagentStart { .. })));
        assert!(!events
            .iter()
            .any(|event| matches!(event, AgentEvent::SubagentEnd { .. })));
        assert!(matches!(events.last(), Some(AgentEvent::ToolEnd { .. })));
    }

    #[tokio::test]
    async fn host_direct_policy_bypasses_model_permission_for_nested_batch_calls() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(CountingTool {
            calls: Arc::clone(&calls),
        }));
        let runtime = direct_runtime_with_config(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-host"),
            AgentConfig {
                permission_checker: Some(Arc::new(DenyAllTools)),
                ..AgentConfig::default()
            },
        );

        let result = runtime
            .call(
                "batch",
                serde_json::json!({
                    "invocations": [{"tool": "counting", "args": {}}]
                }),
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0, "{}", result.output);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn host_direct_policy_propagates_to_nested_program_calls() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(CountingTool {
            calls: Arc::clone(&calls),
        }));
        let runtime = direct_runtime_with_config(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-host-program"),
            AgentConfig {
                permission_checker: Some(Arc::new(DenyAllTools)),
                ..AgentConfig::default()
            },
        );

        let result = runtime
            .call(
                "program",
                serde_json::json!({
                    "type": "script",
                    "source": "async function run(ctx) { return await ctx.tool('counting', {}); }",
                    "allowed_tools": ["counting"]
                }),
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0, "{}", result.output);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn host_direct_nested_calls_do_not_deadlock_a_single_slot_tool_queue() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(CountingTool {
            calls: Arc::clone(&calls),
        }));

        let queue_config = SessionQueueConfig {
            execute_max_concurrency: 1,
            ..SessionQueueConfig::default()
        };
        let (queue_events, _) = broadcast::channel(16);
        let queue = Arc::new(
            SessionLaneQueue::new("session-queue", queue_config, queue_events)
                .await
                .unwrap(),
        );
        queue.start().await.unwrap();

        let tool_context =
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-queue");
        let agent_loop = AgentLoop::new(
            Arc::new(DirectToolTestLlm),
            Arc::clone(&tool_executor),
            tool_context.clone(),
            AgentConfig {
                tool_timeout_ms: Some(100),
                ..AgentConfig::default()
            },
        )
        .with_queue(queue);
        let runtime = DirectToolRuntime {
            tool_executor,
            tool_context,
            agent_loop,
            session_id: "session-queue".to_string(),
            session_cancel: tokio_util::sync::CancellationToken::new(),
            closed: Arc::new(AtomicBool::new(false)),
            task_scheduler: None,
            task_priority: crate::task_scheduler::TaskPriority::Interactive,
            security_provider: None,
        };

        let result = runtime
            .call(
                "batch",
                serde_json::json!({
                    "invocations": [{"tool": "counting", "args": {}}]
                }),
            )
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0, "{}", result.output);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn host_direct_calls_still_obey_budget_before_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(CountingTool {
            calls: Arc::clone(&calls),
        }));
        let runtime = direct_runtime_with_config(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-budget"),
            AgentConfig {
                budget_guard: Some(Arc::new(DenyToolBudget)),
                ..AgentConfig::default()
            },
        );

        let result = runtime
            .call("counting", serde_json::json!({}))
            .await
            .unwrap();

        assert_ne!(result.exit_code, 0);
        assert!(result.output.contains("Budget exhausted"));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn host_direct_calls_still_obey_pre_tool_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let hooks = Arc::new(BlockingPreToolHook::default());
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(CountingTool {
            calls: Arc::clone(&calls),
        }));
        let runtime = direct_runtime_with_config(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-hook"),
            AgentConfig {
                hook_engine: Some(hooks.clone()),
                ..AgentConfig::default()
            },
        );

        let result = runtime
            .call("counting", serde_json::json!({}))
            .await
            .unwrap();

        assert_ne!(result.exit_code, 0);
        assert!(result.output.contains("blocked direct tool in test"));
        assert!(matches!(
            result.error_kind,
            Some(ToolErrorKind::HookDenied {
                reason,
                retryable: false,
                retry_after_ms: None,
            }) if reason == "blocked direct tool in test"
        ));
        assert_eq!(hooks.calls.load(Ordering::SeqCst), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn host_direct_hook_denial_sanitizes_structured_reason() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let hooks = Arc::new(BlockingPreToolHook::default());
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(CountingTool {
            calls: Arc::clone(&calls),
        }));
        let runtime = direct_runtime_with_config(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-hook-sanitize"),
            AgentConfig {
                hook_engine: Some(hooks),
                security_provider: Some(Arc::new(RedactingSecurity)),
                ..AgentConfig::default()
            },
        );

        let result = runtime
            .call("counting", serde_json::json!({}))
            .await
            .unwrap();

        assert_eq!(result.output, "[sanitized]");
        assert!(matches!(
            result.error_kind,
            Some(ToolErrorKind::HookDenied {
                reason,
                retryable: false,
                retry_after_ms: None,
            }) if reason == "[sanitized]"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn host_direct_calls_still_sanitize_tool_output() {
        let dir = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(CountingTool {
            calls: Arc::clone(&calls),
        }));
        let runtime = direct_runtime_with_config(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-sanitize"),
            AgentConfig {
                security_provider: Some(Arc::new(RedactingSecurity)),
                ..AgentConfig::default()
            },
        );

        let result = runtime
            .call("counting", serde_json::json!({}))
            .await
            .unwrap();

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.output, "[sanitized]");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn host_direct_calls_still_obey_tool_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(BlockingTool));
        let runtime = direct_runtime_with_config(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-timeout"),
            AgentConfig {
                tool_timeout_ms: Some(10),
                ..AgentConfig::default()
            },
        );

        let result = runtime
            .call("blocking", serde_json::json!({}))
            .await
            .unwrap();

        assert_ne!(result.exit_code, 0);
        assert!(result.output.contains("timed out after 10ms"));
    }

    #[tokio::test]
    async fn session_cancellation_interrupts_host_direct_tool_execution() {
        let dir = tempfile::tempdir().unwrap();
        let tool_executor = Arc::new(ToolExecutor::new(dir.path().to_string_lossy().to_string()));
        tool_executor.register_dynamic_tool(Arc::new(BlockingTool));
        let runtime = Arc::new(direct_runtime(
            tool_executor,
            ToolContext::new(dir.path().to_path_buf()).with_session_id("session-cancel"),
        ));

        let caller = Arc::clone(&runtime);
        let task =
            tokio::spawn(async move { caller.call("blocking", serde_json::json!({})).await });
        tokio::task::yield_now().await;
        runtime.session_cancel.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("host direct invocation must stop after session cancellation")
            .unwrap()
            .unwrap();
        assert_ne!(result.exit_code, 0);
        assert!(result.output.contains("cancelled"));
    }
}
