use super::*;
use crate::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use crate::tools::{register_generate_object, ToolExecutor, ToolOutput};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[test]
fn registered_dynamic_workflow_does_not_retain_its_registry() {
    let registry = Arc::new(ToolRegistry::new(std::path::PathBuf::from(
        "dynamic-workflow-cycle-test",
    )));
    let lifetime = Arc::downgrade(&registry);
    register_dynamic_workflow(&registry);

    drop(registry);

    assert!(lifetime.upgrade().is_none());
}

struct DelayedObjectClient {
    delay: Duration,
}

#[async_trait]
impl LlmClient for DelayedObjectClient {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        tokio::time::sleep(self.delay).await;
        Ok(LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: r#"{"ok":true}"#.to_string(),
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
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by this test")
    }
}

#[derive(Clone)]
struct ForkingDelayedObjectClient {
    delay: Duration,
    bound_session: Option<String>,
    active: Arc<AtomicUsize>,
    maximum_active: Arc<AtomicUsize>,
    observed_sessions: Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl LlmClient for ForkingDelayedObjectClient {
    fn fork_for_session(&self, session_id: &str) -> Option<Arc<dyn LlmClient>> {
        Some(Arc::new(Self {
            bound_session: Some(session_id.to_string()),
            ..self.clone()
        }))
    }

    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum_active.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.observed_sessions
            .lock()
            .unwrap()
            .push(self.bound_session.clone().unwrap_or_default());
        Ok(LlmResponse {
            message: Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: r#"{"ok":true}"#.to_string(),
                }],
                reasoning_content: None,
            },
            usage: TokenUsage::default(),
            stop_reason: Some("end_turn".to_string()),
            token_logprobs: Vec::new(),
            meta: None,
        })
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("streaming is not used by this test")
    }
}

struct FakeParallelTaskTool;

#[async_trait]
impl Tool for FakeParallelTaskTool {
    fn name(&self) -> &str {
        PARALLEL_TASK_TOOL
    }

    fn description(&self) -> &str {
        "Fake parallel task tool for DynamicWorkflowRuntime tests."
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let count = args
            .get("tasks")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        Ok(ToolOutput::success(format!("parallel:{count}"))
            .with_metadata(json!({ "task_count": count })))
    }
}

struct FakeTaskTool;

#[async_trait]
impl Tool for FakeTaskTool {
    fn name(&self) -> &str {
        TASK_TOOL
    }

    fn description(&self) -> &str {
        "Fake unified task tool for DynamicWorkflowRuntime tests."
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let count = args
            .get("tasks")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        Ok(ToolOutput::success(format!("task:{count}"))
            .with_metadata(json!({ "task_count": count })))
    }
}

struct FakeRuntimeTool;

#[async_trait]
impl Tool for FakeRuntimeTool {
    fn name(&self) -> &str {
        "runtime"
    }

    fn description(&self) -> &str {
        "Fake OS runtime tool for DynamicWorkflowRuntime tests."
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let tasks = args
            .get("tasks")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        Ok(ToolOutput::success(format!("runtime:{tasks}"))
            .with_metadata(json!({ "runtime_tasks": tasks })))
    }
}

struct FailingRuntimeTool;

#[async_trait]
impl Tool for FailingRuntimeTool {
    fn name(&self) -> &str {
        "runtime"
    }

    fn description(&self) -> &str {
        "Failing OS runtime tool for DynamicWorkflowRuntime tests."
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, _args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::error("runtime unavailable"))
    }
}

struct RetryOnceRuntimeTool {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for RetryOnceRuntimeTool {
    fn name(&self) -> &str {
        "runtime"
    }

    fn description(&self) -> &str {
        "Fails once so DynamicWorkflowRuntime can exercise a persisted retry."
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, _args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(if call == 0 {
            ToolOutput::error("transient runtime failure")
        } else {
            ToolOutput::success("runtime recovered")
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generate_object_step_waits_for_admission_before_program_timeout_starts() {
    const PROGRAM_TIMEOUT: Duration = Duration::from_millis(500);
    const QUEUE_WAIT: Duration = Duration::from_millis(700);
    const GENERATION_DELAY: Duration = Duration::from_millis(100);

    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    register_generate_object(
        executor.registry(),
        Arc::new(DelayedObjectClient {
            delay: GENERATION_DELAY,
        }),
    );

    let admission = crate::llm::ModelGenerationAdmission::default();
    let holder = admission
        .acquire(&CancellationToken::new())
        .await
        .expect("hold the only active model-generation slot");
    let context = executor
        .registry()
        .context()
        .with_model_generation_admission(admission);
    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind !== "step" || inputs.step_name !== "generate_object") {
    throw new Error("unexpected invocation");
  }
  return await ctx.tool("generate_object", {
    schema: {
      type: "object",
      additionalProperties: false,
      properties: { ok: { type: "boolean" } },
      required: ["ok"],
    },
    schema_name: "admission_result",
    prompt: "Return an object whose ok field is true.",
    mode: "prompt",
    max_repair_attempts: 0,
    timeout_ms: 2000,
  });
}
"#;
    let runtime = DynamicWorkflowRuntime::new(Arc::clone(executor.registry()), context, source)
        .with_allowed_tools([GENERATE_OBJECT_TOOL.to_string()])
        .with_limits(DynamicWorkflowScriptLimits {
            timeout_ms: Some(PROGRAM_TIMEOUT.as_millis() as u64),
            max_tool_calls: Some(1),
            max_output_bytes: None,
            max_concurrent_generations: None,
        });
    let run = tokio::spawn(async move {
        runtime
            .run_step(StepInvocation::new(
                "model-admission-before-program-timeout",
                "generate",
                GENERATE_OBJECT_TOOL,
                json!({}),
                Vec::new(),
            ))
            .await
    });

    tokio::time::sleep(QUEUE_WAIT).await;
    assert!(
        !run.is_finished(),
        "the Program active timeout must not include model-admission queue wait"
    );
    drop(holder);

    let output = tokio::time::timeout(Duration::from_secs(2), run)
        .await
        .expect("step should receive its full Program timeout after admission")
        .expect("step task should join")
        .expect("step should complete");
    assert_eq!(output["exitCode"], 0);
    assert!(
        output["metadata"]["generation_admission"]["queue_wait_ms"]
            .as_u64()
            .is_some_and(|wait_ms| wait_ms >= 600),
        "{output}"
    );
    assert_eq!(
        output["metadata"]["generation_admission"]["active_timeout_ms"],
        2_000
    );
    assert!(
        output["output"]
            .as_str()
            .is_some_and(|value| value.contains(r#""ok":true"#)),
        "{output}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_generation_fanout_uses_bounded_independent_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let active = Arc::new(AtomicUsize::new(0));
    let maximum_active = Arc::new(AtomicUsize::new(0));
    let observed_sessions = Arc::new(std::sync::Mutex::new(Vec::new()));
    let client = Arc::new(ForkingDelayedObjectClient {
        delay: Duration::from_millis(150),
        bound_session: None,
        active: Arc::clone(&active),
        maximum_active: Arc::clone(&maximum_active),
        observed_sessions: Arc::clone(&observed_sessions),
    });
    register_generate_object(executor.registry(), client.clone());
    let context = executor
        .registry()
        .context()
        .with_llm_client(client as Arc<dyn LlmClient>);
    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind !== "step" || inputs.step_name !== "generate_object") {
    throw new Error("unexpected invocation");
  }
  return await ctx.tool("generate_object", {
    schema: {
      type: "object",
      additionalProperties: false,
      properties: { ok: { type: "boolean" } },
      required: ["ok"],
    },
    schema_name: "parallel_admission_result",
    prompt: "Return an object whose ok field is true.",
    mode: "prompt",
    max_repair_attempts: 0,
    timeout_ms: 2000,
  });
}

"#;
    let runtime = Arc::new(
        DynamicWorkflowRuntime::new(Arc::clone(executor.registry()), context, source)
            .with_allowed_tools([GENERATE_OBJECT_TOOL.to_string()])
            .with_limits(DynamicWorkflowScriptLimits {
                timeout_ms: Some(2_000),
                max_tool_calls: Some(1),
                max_output_bytes: None,
                max_concurrent_generations: Some(2),
            }),
    );
    let invocation = |step_id: &str| {
        StepInvocation::new(
            "bounded-generation-fanout",
            step_id,
            GENERATE_OBJECT_TOOL,
            json!({}),
            Vec::new(),
        )
    };
    let (first, second) = tokio::join!(
        runtime.run_step(invocation("first")),
        runtime.run_step(invocation("second")),
    );
    assert!(first.is_ok(), "{first:?}");
    assert!(second.is_ok(), "{second:?}");
    assert_eq!(maximum_active.load(Ordering::SeqCst), 2);
    let sessions = observed_sessions.lock().unwrap().clone();
    assert_eq!(sessions.len(), 2);
    assert!(sessions.iter().any(|session| session.ends_with(":first")));
    assert!(sessions.iter().any(|session| session.ends_with(":second")));
}

#[tokio::test]
async fn completed_step_recovery_is_bound_to_exact_run_query_and_step() {
    let dir = tempfile::tempdir().unwrap();
    let store_root = dynamic_workflow_store_path(dir.path());
    tokio::fs::create_dir_all(&store_root).await.unwrap();
    let store = LocalFileEventStore::new(&store_root);
    let run_id = "checkpoint-recovery";
    store
        .append(
            run_id,
            FlowEvent::RunCreated {
                spec: WorkflowSpec::rust_embedded("test", "v1", "ptc", "run"),
                input: json!({ "query": "exact inquiry" }),
            },
        )
        .await
        .unwrap();
    store.append(run_id, FlowEvent::RunStarted).await.unwrap();
    store
        .append(
            run_id,
            FlowEvent::StepCompleted {
                step_id: "checkpoint_initial_retrieval".to_string(),
                output: json!({ "mode": "inquiry_collection", "evidence": [1] }),
            },
        )
        .await
        .unwrap();

    let recovered = recover_dynamic_workflow_step_output(
        dir.path(),
        run_id,
        "exact inquiry",
        "checkpoint_initial_retrieval",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(recovered["mode"], "inquiry_collection");
    assert!(recover_dynamic_workflow_step_output(
        dir.path(),
        run_id,
        "different inquiry",
        "checkpoint_initial_retrieval",
    )
    .await
    .unwrap()
    .is_none());
    assert!(recover_dynamic_workflow_step_output(
        dir.path(),
        run_id,
        "exact inquiry",
        "different_step",
    )
    .await
    .unwrap()
    .is_none());
}

#[tokio::test]
async fn dynamic_workflow_optionally_projects_committed_flow_history_into_graph() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let graph = Arc::new(Mutex::new(crate::GraphRuntime::new()));
    let observer = FlowGraphObserver::new(Arc::clone(&graph));
    let tool = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_graph_observer(observer.clone());
    let result = tool
        .execute(
            &json!({
                "source": "async function run(ctx, inputs) { return { type: 'complete', output: { ok: true } }; }",
                "run_id": "graph-projected-workflow"
            }),
            &executor.registry().context(),
        )
        .await
        .unwrap();
    assert!(result.success, "{}", result.content);
    assert!(observer.last_error().await.is_none());
    let graph = graph.lock().await;
    let run = graph
        .graph()
        .object(&crate::flow_run_object_id("graph-projected-workflow"))
        .unwrap();
    assert_eq!(run.data["status"], "completed");
    assert_eq!(run.data["last_sequence"], 3);
}

#[tokio::test]
async fn dynamic_workflow_tool_runs_ptc_step_through_a3s_flow() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("fixture.txt"), "hello from fixture")
        .await
        .unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    register_dynamic_workflow(executor.registry());

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
const read = inputs.step_outputs.read_fixture;
if (read) {
  return { type: "complete", output: { text: read.output } };
}
return {
  type: "schedule_step",
  step_id: "read_fixture",
  step_name: "read_fixture",
  input: { path: inputs.input.path },
  retry: { max_attempts: 1, delay_ms: 0 },
};
  }

  if (inputs.kind === "step" && inputs.step_name === "read_fixture") {
return await ctx.read(inputs.input.path);
  }

  return { error: "unknown invocation" };
}
"#;

    let result = executor
        .execute(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": source,
                "input": { "path": "fixture.txt" },
                "run_id": "test-dynamic-workflow",
                "allowed_tools": ["read"],
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(
        result.output.contains("hello from fixture"),
        "{}",
        result.output
    );
    let metadata = result.metadata.unwrap();
    assert_eq!(
        metadata["dynamic_workflow"]["run_id"],
        "test-dynamic-workflow"
    );
    assert_eq!(metadata["dynamic_workflow"]["status"], "Completed");
    assert_eq!(
        metadata["dynamic_workflow"]["snapshot"]["steps"]["read_fixture"]["status"],
        "completed"
    );
    assert!(
        tokio::fs::try_exists(
            dynamic_workflow_store_path(dir.path()).join("test-dynamic-workflow.jsonl")
        )
        .await
        .unwrap(),
        "dynamic workflow history should be stored under .a3s/workflow"
    );
    assert!(
        !tokio::fs::try_exists(dir.path().join(".a3s/flow"))
            .await
            .unwrap(),
        "dynamic workflows must not recreate the legacy .a3s/flow directory"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn dynamic_workflow_refuses_symlinked_project_store() {
    use std::os::unix::fs::symlink;

    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    tokio::fs::create_dir_all(workspace.path().join(".a3s"))
        .await
        .unwrap();
    symlink(outside.path(), workspace.path().join(".a3s/workflow")).unwrap();
    let context = ToolContext::new(workspace.path().to_path_buf());

    let error = match flow_store_for_context(&context, Some("symlink-test")).await {
        Ok(_) => panic!("a symlinked dynamic workflow store must be rejected"),
        Err(error) => error,
    };

    assert!(error
        .to_string()
        .contains("symlinked dynamic workflow directory"));
    let mut entries = tokio::fs::read_dir(outside.path()).await.unwrap();
    assert!(entries.next_entry().await.unwrap().is_none());
}

#[tokio::test]
async fn dynamic_workflow_emits_agent_progress_events() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(dir.path().join("fixture.txt"), "hello from fixture")
        .await
        .unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    register_dynamic_workflow(executor.registry());
    let (tx, mut rx) = broadcast::channel(64);
    let ctx = ToolContext::new(dir.path().to_path_buf())
        .with_session_id("progress-session")
        .with_agent_event_tx(tx);

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
const read = inputs.step_outputs.read_fixture;
if (read) {
  return { type: "complete", output: { text: read.output } };
}
return {
  type: "schedule_step",
  step_id: "read_fixture",
  step_name: "read_fixture",
  input: { path: inputs.input.path, description: "Read fixture" },
  retry: { max_attempts: 1, delay_ms: 0 },
};
  }

  if (inputs.kind === "step" && inputs.step_name === "read_fixture") {
return await ctx.read(inputs.input.path);
  }

  return { error: "unknown invocation" };
}
"#;

    let result = executor
        .execute_with_context(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": source,
                "input": { "path": "fixture.txt" },
                "run_id": "test-dynamic-workflow-progress",
                "allowed_tools": ["read"],
            }),
            &ctx,
        )
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0, "{}", result.output);
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::PlanningStart { .. })),
        "{events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::TaskUpdated { tasks, .. }
                if tasks.iter().any(|task| task.id == "read_fixture")
        )),
        "{events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::StepStart { step_id, .. } if step_id == "read_fixture"
        )),
        "{events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AgentEvent::StepEnd { step_id, status, .. }
                if step_id == "read_fixture" && *status == TaskStatus::Completed
        )),
        "{events:?}"
    );
}

#[tokio::test]
async fn dynamic_workflow_step_can_call_host_task_without_ptc_task_fanout() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    executor.register_dynamic_tool(Arc::new(FakeTaskTool));
    register_dynamic_workflow(executor.registry());

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
const fanout = inputs.step_outputs.fanout;
if (fanout) {
  return { type: "complete", output: { fanout } };
}
return {
  type: "schedule_step",
  step_id: "fanout",
  step_name: "task",
  input: {
    tasks: [
      { agent: "explore", description: "alpha", prompt: "research alpha" },
      { agent: "explore", description: "beta", prompt: "research beta" },
    ],
  },
};
  }

  return { error: "ptc step handler should not run for task fan-out" };
}
"#;

    let result = executor
        .execute(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": source,
                "run_id": "test-dynamic-workflow-task-step",
                "allowed_tools": [],
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("task:2"), "{}", result.output);
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["dynamic_workflow"]["status"], "Completed");
    let step = &metadata["dynamic_workflow"]["snapshot"]["steps"]["fanout"];
    assert_eq!(step["status"], "completed");
    assert_eq!(step["output"]["tool"], TASK_TOOL);
    assert_eq!(step["output"]["metadata"]["task_count"], 2);
}

#[tokio::test]
async fn dynamic_workflow_legacy_parallel_task_step_remains_readable() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    executor.register_dynamic_tool(Arc::new(FakeParallelTaskTool));
    register_dynamic_workflow(executor.registry());

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
const fanout = inputs.step_outputs.fanout;
if (fanout) {
  return { type: "complete", output: { fanout } };
}
return {
  type: "schedule_step",
  step_id: "fanout",
  step_name: "parallel_task",
  input: {
    tasks: [
      { agent: "explore", description: "alpha", prompt: "research alpha" },
      { agent: "explore", description: "beta", prompt: "research beta" },
    ],
  },
};
  }

  return { error: "ptc step handler should not run for parallel_task" };
}
"#;

    let result = executor
        .execute(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": source,
                "run_id": "test-dynamic-workflow-parallel-step",
                "allowed_tools": [],
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("parallel:2"), "{}", result.output);
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["dynamic_workflow"]["status"], "Completed");
    let step = &metadata["dynamic_workflow"]["snapshot"]["steps"]["fanout"];
    assert_eq!(step["status"], "completed");
    assert_eq!(step["output"]["tool"], PARALLEL_TASK_TOOL);
    assert_eq!(step["output"]["metadata"]["task_count"], 2);
}

#[tokio::test]
async fn dynamic_workflow_ptc_step_can_call_login_registered_runtime_tool_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    executor.register_dynamic_tool(Arc::new(FakeRuntimeTool));
    register_dynamic_workflow(executor.registry());

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
const runtime = inputs.step_outputs.runtime_fanout;
if (runtime) {
  return { type: "complete", output: { runtime } };
}
return {
  type: "schedule_step",
  step_id: "runtime_fanout",
  step_name: "runtime_fanout",
  input: {
    worker: "research-worker",
    tasks: ["alpha", "beta", "gamma"],
  },
};
  }

  if (inputs.kind === "step" && inputs.step_name === "runtime_fanout") {
return await ctx.tool("runtime", inputs.input);
  }

  return { error: "unknown invocation" };
}
"#;

    let result = executor
        .execute(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": source,
                "run_id": "test-dynamic-workflow-runtime-step",
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("runtime:3"), "{}", result.output);
    let metadata = result.metadata.unwrap();
    let step = &metadata["dynamic_workflow"]["snapshot"]["steps"]["runtime_fanout"];
    assert_eq!(step["status"], "completed");
    assert_eq!(step["output"]["name"], "runtime");
    assert_eq!(step["output"]["metadata"]["runtime_tasks"], 3);
}

#[tokio::test]
async fn dynamic_workflow_ptc_step_can_call_legacy_ctx_tools_runtime_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    executor.register_dynamic_tool(Arc::new(FakeRuntimeTool));
    register_dynamic_workflow(executor.registry());

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
const runtime = inputs.step_outputs.runtime_fanout;
if (runtime) {
  return { type: "complete", output: { runtime } };
}
return {
  type: "schedule_step",
  step_id: "runtime_fanout",
  step_name: "runtime_fanout",
  input: {
    worker: "research-worker",
    tasks: ["alpha", "beta"],
  },
};
  }

  if (inputs.kind === "step" && inputs.step_name === "runtime_fanout") {
return await ctx.tools.runtime(inputs.input);
  }

  return { error: "unknown invocation" };
}
"#;

    let result = executor
        .execute(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": source,
                "run_id": "test-dynamic-workflow-runtime-tools-proxy",
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("runtime:2"), "{}", result.output);
    let metadata = result.metadata.unwrap();
    let step = &metadata["dynamic_workflow"]["snapshot"]["steps"]["runtime_fanout"];
    assert_eq!(step["status"], "completed");
    assert_eq!(step["output"]["name"], "runtime");
    assert_eq!(step["output"]["metadata"]["runtime_tasks"], 2);
}

#[tokio::test]
async fn dynamic_workflow_tool_returns_error_when_runtime_step_fails() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    executor.register_dynamic_tool(Arc::new(FailingRuntimeTool));
    register_dynamic_workflow(executor.registry());

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
const runtime = inputs.step_outputs.runtime_fanout;
if (runtime) {
  return { type: "complete", output: { runtime } };
}
return {
  type: "schedule_step",
  step_id: "runtime_fanout",
  step_name: "runtime_fanout",
  input: { worker: "research-worker", tasks: ["alpha"] },
};
  }

  if (inputs.kind === "step" && inputs.step_name === "runtime_fanout") {
const result = await ctx.tool("runtime", inputs.input);
if (result.exitCode !== 0) {
  throw new Error(result.output || "runtime failed");
}
return result;
  }

  return { error: "unknown invocation" };
}
"#;

    let result = executor
        .execute(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": source,
                "run_id": "test-dynamic-workflow-runtime-step-fails",
            }),
        )
        .await
        .unwrap();

    assert_ne!(result.exit_code, 0, "{}", result.output);
    assert!(
        result.output.contains("runtime unavailable"),
        "{}",
        result.output
    );
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["dynamic_workflow"]["status"], "Failed");
    let step = &metadata["dynamic_workflow"]["snapshot"]["steps"]["runtime_fanout"];
    assert_eq!(step["status"], "failed");
}

#[tokio::test]
async fn dynamic_workflow_step_failure_can_continue_workflow_with_error_payload() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    executor.register_dynamic_tool(Arc::new(FailingRuntimeTool));
    register_dynamic_workflow(executor.registry());

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
const failure = inputs.step_failures.runtime_fanout;
if (failure) {
  return { type: "complete", output: { recovered: true, error: failure.error } };
}
return {
  type: "schedule_step",
  step_id: "runtime_fanout",
  step_name: "runtime_fanout",
  input: { worker: "research-worker" },
  retry: { max_attempts: 1, delay_ms: 0, on_exhausted: "continue_workflow" },
};
  }

  if (inputs.kind === "step" && inputs.step_name === "runtime_fanout") {
const result = await ctx.tool("runtime", inputs.input);
if (result.exitCode !== 0) {
  throw new Error(result.output || "runtime failed");
}
return result;
  }

  return { error: "unknown invocation" };
}
"#;

    let result = executor
        .execute(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": source,
                "run_id": "test-dynamic-workflow-continue-after-step-failure",
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(
        result.output.contains("runtime unavailable"),
        "{}",
        result.output
    );
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["dynamic_workflow"]["status"], "Completed");
    let step = &metadata["dynamic_workflow"]["snapshot"]["steps"]["runtime_fanout"];
    assert_eq!(step["status"], "failed");
    assert!(step["error"]
        .as_str()
        .is_some_and(|error| error.contains("runtime unavailable")));
}

#[tokio::test]
async fn dynamic_workflow_drives_short_event_sourced_retries_to_completion() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let calls = Arc::new(AtomicUsize::new(0));
    executor.register_dynamic_tool(Arc::new(RetryOnceRuntimeTool {
        calls: Arc::clone(&calls),
    }));
    register_dynamic_workflow(executor.registry());

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const result = inputs.step_outputs.retry_once;
    if (result) {
      return { type: "complete", output: { recovered: result.output } };
    }
    return {
      type: "schedule_step",
      step_id: "retry_once",
      step_name: "retry_once",
      input: {},
      retry: { max_attempts: 2, delay_ms: 10 },
    };
  }
  if (inputs.kind === "step" && inputs.step_name === "retry_once") {
    const result = await ctx.tool("runtime", {});
    if (result.exitCode !== 0) {
      throw new Error(result.output || "runtime failed");
    }
    return result;
  }
  return { error: "unknown invocation" };
}
"#;

    let result = executor
        .execute(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": source,
                "run_id": "test-dynamic-workflow-inline-retry",
            }),
        )
        .await
        .unwrap();

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(
        result.output.contains("runtime recovered"),
        "{}",
        result.output
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["dynamic_workflow"]["status"], "Completed");
    assert_eq!(
        metadata["dynamic_workflow"]["snapshot"]["steps"]["retry_once"]["attempt"],
        2
    );
}

#[tokio::test]
async fn dynamic_workflow_tool_returns_error_when_run_is_suspended() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    register_dynamic_workflow(executor.registry());

    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
return {
  type: "wait_until",
  wait_id: "external-research-still-running",
  resume_at: "2099-01-01T00:00:00Z",
};
  }

  return { error: "unknown invocation" };
}
"#;

    let result = executor
        .execute(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": source,
                "run_id": "test-dynamic-workflow-suspended-is-error",
            }),
        )
        .await
        .unwrap();

    assert_ne!(result.exit_code, 0, "{}", result.output);
    assert!(
        result
            .output
            .contains("dynamic_workflow ended without a terminal result: Suspended"),
        "{}",
        result.output
    );
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["dynamic_workflow"]["status"], "Suspended");
    assert_eq!(
        metadata["dynamic_workflow"]["snapshot"]["waits"]["external-research-still-running"]
            ["status"],
        "waiting"
    );
}

#[test]
fn default_allowed_tools_exclude_recursive_program_and_dynamic_workflow_tools() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    register_dynamic_workflow(executor.registry());

    let tools = default_allowed_tools(executor.registry());

    assert!(!tools.contains(&PROGRAM_TOOL.to_string()));
    assert!(!tools.contains(&DYNAMIC_WORKFLOW_TOOL.to_string()));
    assert!(!tools.contains(&PARALLEL_TASK_TOOL.to_string()));
    assert!(tools.contains(&"read".to_string()));
}
