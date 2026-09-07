use super::*;
use crate::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use crate::tools::{register_generate_object, ToolExecutor, ToolOutput};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{mpsc, Notify};
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

#[test]
fn event_store_registration_does_not_retain_its_registry() {
    let registry = Arc::new(ToolRegistry::new(std::path::PathBuf::from(
        "dynamic-workflow-store-cycle-test",
    )));
    let lifetime = Arc::downgrade(&registry);
    register_dynamic_workflow_with_event_store(&registry, Arc::new(InMemoryEventStore::new()));

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

struct BlockingTaskTool {
    started: Arc<AtomicUsize>,
    release: Arc<Notify>,
}

struct BlockingProbeTool {
    started: Arc<AtomicUsize>,
    release: Arc<Notify>,
}

#[async_trait]
impl Tool for BlockingProbeTool {
    fn name(&self) -> &str {
        "flow_probe"
    }

    fn description(&self) -> &str {
        "Blocking probe for global Flow-step admission tests."
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, _args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        self.started.fetch_add(1, Ordering::SeqCst);
        self.release.notified().await;
        Ok(ToolOutput::success("released"))
    }
}

#[async_trait]
impl Tool for BlockingTaskTool {
    fn name(&self) -> &str {
        TASK_TOOL
    }

    fn description(&self) -> &str {
        "Blocking task tool for dynamic workflow admission tests."
    }

    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }

    async fn execute(&self, _args: &Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        self.started.fetch_add(1, Ordering::SeqCst);
        self.release.notified().await;
        Ok(ToolOutput::success("released"))
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

#[derive(Default)]
struct LeaseLosingWorkflowLedger;

#[async_trait]
impl FlowDecisionLedger for LeaseLosingWorkflowLedger {
    async fn claim(
        &self,
        _decision_id: &str,
        _request_hash: &str,
        _owner_id: &str,
        _now_ms: u64,
        _lease_ms: u64,
    ) -> anyhow::Result<FlowDecisionClaimOutcome> {
        Ok(FlowDecisionClaimOutcome::Claimed { attempt: 1 })
    }

    async fn renew(
        &self,
        _decision_id: &str,
        _request_hash: &str,
        _owner_id: &str,
        _now_ms: u64,
        _lease_ms: u64,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn complete(
        &self,
        _decision_id: &str,
        _request_hash: &str,
        _owner_id: &str,
        _completed_at_ms: u64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn release(
        &self,
        _decision_id: &str,
        _request_hash: &str,
        _owner_id: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }
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
            max_concurrent_steps: None,
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
                max_concurrent_steps: None,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dynamic_workflow_step_admission_is_bounded_and_cancellable() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let started = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    executor.register_dynamic_tool(Arc::new(BlockingTaskTool {
        started: Arc::clone(&started),
        release: Arc::clone(&release),
    }));
    let cancellation = CancellationToken::new();
    let context = executor
        .registry()
        .context()
        .with_cancellation(cancellation.clone());
    let runtime = Arc::new(
        DynamicWorkflowRuntime::new(
            Arc::clone(executor.registry()),
            context,
            "unused for host task steps",
        )
        .with_limits(DynamicWorkflowScriptLimits {
            timeout_ms: None,
            max_tool_calls: None,
            max_output_bytes: None,
            max_concurrent_generations: None,
            max_concurrent_steps: Some(1),
        }),
    );

    let first_runtime = Arc::clone(&runtime);
    let first = tokio::spawn(async move {
        first_runtime
            .run_step(StepInvocation::new(
                "admission-run",
                "first",
                TASK_TOOL,
                json!({}),
                Vec::new(),
            ))
            .await
    });
    for _ in 0..1_000 {
        if started.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    if started.load(Ordering::SeqCst) != 1 {
        let result = first.await.unwrap();
        panic!("first step did not enter blocking tool: {result:?}");
    }

    let second_runtime = Arc::clone(&runtime);
    let second = tokio::spawn(async move {
        second_runtime
            .run_step(StepInvocation::new(
                "admission-run",
                "second",
                TASK_TOOL,
                json!({}),
                Vec::new(),
            ))
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(started.load(Ordering::SeqCst), 1);
    let stats = runtime.admission_stats();
    assert_eq!(stats.active_steps, 1);
    assert_eq!(stats.admitted_steps, 1);
    assert_eq!(stats.peak_active_steps, 1);

    cancellation.cancel();
    let second_result = tokio::time::timeout(Duration::from_secs(1), second)
        .await
        .expect("queued step should observe cancellation")
        .expect("queued step task should join")
        .expect_err("queued step must not execute after cancellation");
    assert!(second_result.to_string().contains("admission cancelled"));

    release.notify_waiters();
    first.await.unwrap().unwrap();
    let stats = runtime.admission_stats();
    assert_eq!(stats.active_steps, 0);
    assert_eq!(stats.admitted_steps, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dynamic_workflow_can_layer_global_scheduler_admission_without_step_deadlock() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let started = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    executor.register_dynamic_tool(Arc::new(BlockingProbeTool {
        started: Arc::clone(&started),
        release: Arc::clone(&release),
    }));
    let scheduler = Arc::new(
        crate::task_scheduler::TaskScheduler::new(crate::task_scheduler::TaskSchedulerConfig {
            max_active: 1,
            aging_interval_ms: 60_000,
        })
        .unwrap(),
    );
    let runtime = Arc::new(
        DynamicWorkflowRuntime::new(
            Arc::clone(executor.registry()),
            executor.registry().context(),
            r#"async function run(ctx, inputs) {
                if (inputs.kind !== "step") throw new Error("unexpected invocation");
                return await ctx.tool("flow_probe", {});
            }"#,
        )
        .with_allowed_tools(["flow_probe".to_string()])
        .with_task_scheduler(Arc::clone(&scheduler), true)
        .with_task_scheduler_quota(
            crate::task_scheduler::TaskSchedulerQuota::for_scope("dynamic:global-admission-run", 1)
                .unwrap(),
        ),
    );

    let first_runtime = Arc::clone(&runtime);
    let first = tokio::spawn(async move {
        first_runtime
            .run_step(StepInvocation::new(
                "global-admission-run",
                "first",
                "flow_probe",
                json!({}),
                Vec::new(),
            ))
            .await
    });
    for _ in 0..1_000 {
        if started.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(started.load(Ordering::SeqCst), 1);

    let second_runtime = Arc::clone(&runtime);
    let second = tokio::spawn(async move {
        second_runtime
            .run_step(StepInvocation::new(
                "global-admission-run",
                "second",
                "flow_probe",
                json!({}),
                Vec::new(),
            ))
            .await
    });
    for _ in 0..1_000 {
        if scheduler.stats().await.unwrap().pending == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(scheduler.stats().await.unwrap().pending, 1);
    assert_eq!(started.load(Ordering::SeqCst), 1);

    release.notify_one();
    first.await.unwrap().unwrap();
    for _ in 0..1_000 {
        if started.load(Ordering::SeqCst) == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(started.load(Ordering::SeqCst), 2);
    release.notify_one();
    second.await.unwrap().unwrap();
    assert_eq!(runtime.admission_stats().peak_active_steps, 2);
    assert_eq!(scheduler.stats().await.unwrap().active, 0);
    let quota = runtime
        .scheduler_quota_snapshot()
        .await
        .unwrap()
        .expect("runtime quota is configured");
    assert_eq!(quota.max_active, 1);
    assert_eq!(quota.active, 0);
    assert_eq!(quota.pending, 0);
    scheduler.shutdown().await;
}

#[test]
fn dynamic_flow_step_identity_is_digest_only_and_input_bound() {
    let first =
        dynamic_workflow_step_identity("run-1", "step-1", "read", &json!({ "path": "README.md" }))
            .unwrap();
    let same =
        dynamic_workflow_step_identity("run-1", "step-1", "read", &json!({ "path": "README.md" }))
            .unwrap();
    let changed =
        dynamic_workflow_step_identity("run-1", "step-1", "read", &json!({ "path": "src/lib.rs" }))
            .unwrap();
    assert_eq!(first, same);
    assert_ne!(first, changed);
    assert!(!serde_json::to_string(&first).unwrap().contains("README.md"));
}

#[test]
fn dynamic_flow_history_projection_reconstructs_resumed_plan() {
    let run_id = "projection-run";
    let envelope = |sequence, event| {
        FlowEventEnvelope::new(run_id, sequence, uuid::Uuid::new_v4(), Utc::now(), event)
    };
    let history = vec![
        envelope(
            1,
            FlowEvent::RunCreated {
                spec: WorkflowSpec::rust_embedded("test", "v1", "ptc", "run"),
                input: json!({}),
            },
        ),
        envelope(
            2,
            FlowEvent::StepCreated {
                step_id: "first".to_string(),
                step_name: "read".to_string(),
                input: json!({"description": "Read source"}),
                retry: Default::default(),
            },
        ),
        envelope(
            3,
            FlowEvent::StepCompleted {
                step_id: "first".to_string(),
                output: json!({"ok": true}),
            },
        ),
        envelope(
            4,
            FlowEvent::StepCreated {
                step_id: "second".to_string(),
                step_name: "write".to_string(),
                input: json!({"title": "Write result"}),
                retry: Default::default(),
            },
        ),
        envelope(
            5,
            FlowEvent::StepStarted {
                step_id: "second".to_string(),
                attempt: 1,
            },
        ),
    ];
    let plan = dynamic_workflow_execution_plan(&history);
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[0].status, TaskStatus::Completed);
    assert_eq!(plan.steps[1].status, TaskStatus::InProgress);
    assert_eq!(plan.steps[0].content, "Read source");
    assert_eq!(plan.steps[1].content, "Write result");
    assert_eq!(plan.required_tools, vec!["read", "write"]);
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
    assert_eq!(
        metadata["dynamic_workflow"]["plan"]["steps"][0]["id"],
        "read_fixture"
    );
    assert_eq!(
        metadata["dynamic_workflow"]["plan"]["steps"][0]["status"],
        "completed"
    );
    assert_eq!(
        metadata["dynamic_workflow"]["plan_identity"]["domain"],
        crate::execution_identity::EXECUTION_PLAN_IDENTITY_DOMAIN_V1
    );
    assert_eq!(
        metadata["dynamic_workflow"]["runtime_build_id"],
        DYNAMIC_WORKFLOW_RUNTIME_BUILD_ID
    );
    assert_eq!(
        metadata["dynamic_workflow"]["continuation_identity"]["domain"],
        DYNAMIC_WORKFLOW_CONTINUATION_IDENTITY_DOMAIN_V1
    );
    assert_eq!(
        metadata["dynamic_workflow"]["admission"]["peakActiveSteps"],
        1
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dynamic_workflow_worker_lease_serializes_same_run_and_replays_completion() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let started = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    executor.register_dynamic_tool(Arc::new(BlockingTaskTool {
        started: Arc::clone(&started),
        release: Arc::clone(&release),
    }));
    let ledger: Arc<dyn FlowDecisionLedger> = Arc::new(MemoryFlowDecisionLedger::new());
    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    if (inputs.step_outputs.work) {
      return { type: "complete", output: { ok: true } };
    }
    return {
      type: "schedule_step",
      step_id: "work",
      step_name: "task",
      input: {},
      retry: { max_attempts: 1, delay_ms: 0 },
    };
  }
  return await ctx.tool("task", inputs.input);
}
"#;
    let args = json!({
        "source": source,
        "run_id": "serialized-worker-run",
    });
    let first_tool = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_continuation_lease_ledger(Arc::clone(&ledger), 1_000);
    let second_tool = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_continuation_lease_ledger(Arc::clone(&ledger), 1_000);
    let first_context = executor.registry().context();
    let first_args = args.clone();
    let first_task =
        tokio::spawn(async move { first_tool.execute(&first_args, &first_context).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while started.load(Ordering::SeqCst) != 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("first worker should enter its side-effecting step");

    let second = second_tool
        .execute(
            &json!({
                "source": source,
                "run_id": "serialized-worker-run",
            }),
            &executor.registry().context(),
        )
        .await
        .unwrap();
    assert!(
        !second.success,
        "a live lease must reject a competing worker"
    );
    assert!(
        second.content.contains("worker lease is busy"),
        "{}",
        second.content
    );
    assert_eq!(started.load(Ordering::SeqCst), 1);

    release.notify_waiters();
    let first = tokio::time::timeout(Duration::from_secs(2), first_task)
        .await
        .expect("first worker should settle")
        .expect("first worker task should join")
        .unwrap();
    assert!(first.success, "{}", first.content);
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(
        first.metadata.as_ref().expect("first metadata")["dynamic_workflow"]["worker_lease"]
            ["state"],
        "completed"
    );

    let replay = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_continuation_lease_ledger(ledger, 1_000)
        .execute(&args, &executor.registry().context())
        .await
        .unwrap();
    assert!(replay.success, "{}", replay.content);
    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(
        replay.metadata.as_ref().expect("replay metadata")["dynamic_workflow"]["worker_lease"]
            ["state"],
        "already_completed"
    );
}

#[tokio::test]
async fn dynamic_workflow_generates_safe_run_id_and_keeps_lease_sidecar_digest_only() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let source =
        "async function run(ctx, inputs) { return { type: 'complete', output: { ok: true } }; }";
    let result = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .execute(
            &json!({ "source": source, "input": { "secret": "do-not-persist" } }),
            &executor.registry().context(),
        )
        .await
        .unwrap();
    assert!(result.success, "{}", result.content);
    let metadata = result.metadata.expect("generated-run metadata");
    let run_id = metadata["dynamic_workflow"]["run_id"]
        .as_str()
        .expect("generated run id");
    assert!(safe_workflow_run_id(run_id));
    assert_eq!(
        metadata["dynamic_workflow"]["worker_lease"]["state"],
        "completed"
    );
    let lease_path = dynamic_workflow_store_path(dir.path())
        .join(DYNAMIC_WORKFLOW_LEASE_RELATIVE_PATH)
        .join("flow-decisions.json");
    let lease_contents = tokio::fs::read_to_string(lease_path).await.unwrap();
    assert!(!lease_contents.contains("do-not-persist"));
    assert!(!lease_contents.contains(source));
}

#[tokio::test]
async fn dynamic_workflow_rejects_unsafe_run_id_before_store_access() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let result = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .execute(
            &json!({
                "source": "async function run(ctx, inputs) { return { type: 'complete', output: {} }; }",
                "run_id": "../outside",
            }),
            &executor.registry().context(),
        )
        .await
        .unwrap();
    assert!(!result.success);
    assert!(result.content.contains("run_id must contain only"));
    assert!(!tokio::fs::try_exists(dir.path().join("outside.jsonl"))
        .await
        .unwrap());
}

#[tokio::test]
async fn dynamic_workflow_lost_lease_is_fenced_before_runtime_admission() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let tool = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_continuation_lease_ledger(Arc::new(LeaseLosingWorkflowLedger), 1_000);
    let result = tool
        .execute(
            &json!({
                "source": "async function run(ctx, inputs) { return { type: 'complete', output: { should_not_run: true } }; }",
                "run_id": "lost-lease-before-admission",
            }),
            &executor.registry().context(),
        )
        .await
        .unwrap();
    assert!(!result.success);
    assert!(
        result.content.contains("lease is no longer owned"),
        "{}",
        result.content
    );

    let store = LocalFileEventStore::new(dynamic_workflow_store_path(dir.path()));
    let history = store.list("lost-lease-before-admission").await.unwrap();
    assert!(history
        .iter()
        .all(|envelope| !is_terminal_workflow_event(&envelope.event)));
    assert!(history
        .iter()
        .all(|envelope| !matches!(&envelope.event, FlowEvent::StepCreated { .. })));
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

    // Replaying a terminal run must use the durable completion and never
    // execute the side-effecting step a third time.
    let replay = executor
        .execute(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": source,
                "run_id": "test-dynamic-workflow-inline-retry",
            }),
        )
        .await
        .unwrap();
    assert_eq!(replay.exit_code, 0, "{}", replay.output);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        replay.metadata.as_ref().expect("replay metadata")["dynamic_workflow"]
            ["continuation_identity"]["domain"],
        DYNAMIC_WORKFLOW_CONTINUATION_IDENTITY_DOMAIN_V1
    );

    // A changed source is a new immutable generation, not a permission to
    // rerun the old step under the same durable run id.
    let changed_source = format!("{source}\n// changed generation");
    let changed = executor
        .execute(
            DYNAMIC_WORKFLOW_TOOL,
            &json!({
                "source": changed_source,
                "run_id": "test-dynamic-workflow-inline-retry",
            }),
        )
        .await
        .unwrap();
    assert_ne!(changed.exit_code, 0, "changed source must be rejected");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dynamic_workflow_parent_cancellation_releases_claim_for_retrying_worker() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let calls = Arc::new(AtomicUsize::new(0));
    executor.register_dynamic_tool(Arc::new(RetryOnceRuntimeTool {
        calls: Arc::clone(&calls),
    }));
    let ledger: Arc<dyn FlowDecisionLedger> = Arc::new(MemoryFlowDecisionLedger::new());
    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const result = inputs.step_outputs.retry_once;
    if (result) return { type: "complete", output: { recovered: result.output } };
    return {
      type: "schedule_step",
      step_id: "retry_once",
      step_name: "retry_once",
      input: {},
      retry: { max_attempts: 2, delay_ms: 200 },
    };
  }
  const result = await ctx.tool("runtime", {});
  if (result.exitCode !== 0) throw new Error(result.output || "runtime failed");
  return result;
}
"#;
    let cancellation = CancellationToken::new();
    let first_context = executor
        .registry()
        .context()
        .with_cancellation(cancellation.clone());
    let first_tool = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_continuation_lease_ledger(Arc::clone(&ledger), 1_000);
    let first_args = json!({
        "source": source,
        "run_id": "cancel-and-retry-run",
    });
    let first_task =
        tokio::spawn(async move { first_tool.execute(&first_args, &first_context).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while calls.load(Ordering::SeqCst) != 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("first attempt should be recorded before cancellation");
    cancellation.cancel();
    let first = tokio::time::timeout(Duration::from_secs(2), first_task)
        .await
        .expect("cancelled worker should settle")
        .expect("cancelled worker task should join")
        .unwrap();
    assert!(
        !first.success,
        "parent cancellation must not report success"
    );

    // The retry boundary remains in Flow history, but the old worker no longer
    // owns the claim. A fresh worker can take it over without replaying the
    // already-recorded first attempt as a second side effect.
    tokio::time::sleep(Duration::from_millis(250)).await;
    let second = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_continuation_lease_ledger(ledger, 1_000)
        .execute(
            &json!({
                "source": source,
                "run_id": "cancel-and-retry-run",
            }),
            &executor.registry().context(),
        )
        .await
        .unwrap();
    assert!(second.success, "{}", second.content);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        second.metadata.as_ref().expect("retry metadata")["dynamic_workflow"]["worker_lease"]
            ["state"],
        "completed"
    );
}

#[test]
fn dynamic_workflow_continuation_identity_binds_persisted_facts_not_progress() {
    let run_id = "continuation-identity";
    let source =
        "async function run(ctx, inputs) { return { type: 'complete', output: { ok: true } }; }";
    let input = json!({"secret_path": "scientific-notes.txt"});
    let runtime_build = RuntimeBuildId::new("code-generation-a").unwrap();
    let envelope = |sequence, event| {
        FlowEventEnvelope::new(run_id, sequence, uuid::Uuid::new_v4(), Utc::now(), event)
    };
    let spec = WorkflowSpec::rust_embedded(
        "a3s-code.dynamic-workflow",
        source_hash(source),
        "ptc",
        "run",
    )
    .with_runtime_build(runtime_build.clone());
    let created = envelope(
        1,
        FlowEvent::RunCreated {
            spec,
            input: input.clone(),
        },
    );
    let step = envelope(
        2,
        FlowEvent::StepCreated {
            step_id: "read".to_string(),
            step_name: "read".to_string(),
            input: json!({"path": "scientific-notes.txt"}),
            retry: Default::default(),
        },
    );
    let before = vec![
        created.clone(),
        step.clone(),
        envelope(
            3,
            FlowEvent::StepStarted {
                step_id: "read".to_string(),
                attempt: 1,
            },
        ),
    ];
    let after = vec![
        created,
        step.clone(),
        envelope(
            3,
            FlowEvent::StepStarted {
                step_id: "read".to_string(),
                attempt: 1,
            },
        ),
        envelope(
            4,
            FlowEvent::StepCompleted {
                step_id: "read".to_string(),
                output: json!({"content": "secret"}),
            },
        ),
        envelope(
            5,
            FlowEvent::RunCompleted {
                output: json!({"ok": true}),
            },
        ),
    ];
    let first = dynamic_workflow_continuation_identity(
        run_id,
        source,
        &input,
        runtime_build.as_str(),
        &before,
    )
    .unwrap();
    let replay = dynamic_workflow_continuation_identity(
        run_id,
        source,
        &input,
        runtime_build.as_str(),
        &after,
    )
    .unwrap();
    assert_eq!(first, replay);
    assert!(!serde_json::to_string(&replay)
        .unwrap()
        .contains("scientific-notes.txt"));

    let mut conflicting = after.clone();
    conflicting.push(envelope(
        6,
        FlowEvent::StepCreated {
            step_id: "read".to_string(),
            step_name: "write".to_string(),
            input: json!({"path": "scientific-notes.txt"}),
            retry: Default::default(),
        },
    ));
    assert!(dynamic_workflow_continuation_identity(
        run_id,
        source,
        &input,
        runtime_build.as_str(),
        &conflicting,
    )
    .is_err());

    let mut conflicting_retry = before.clone();
    conflicting_retry.push(envelope(
        4,
        FlowEvent::StepCreated {
            step_id: "read".to_string(),
            step_name: "read".to_string(),
            input: json!({"path": "scientific-notes.txt"}),
            retry: a3s_flow::RetryPolicy::fixed(2, Duration::from_millis(1)),
        },
    ));
    assert!(dynamic_workflow_continuation_identity(
        run_id,
        source,
        &input,
        runtime_build.as_str(),
        &conflicting_retry,
    )
    .is_err());

    let mut malformed_sequence = before.clone();
    malformed_sequence[2].sequence = 4;
    assert!(dynamic_workflow_continuation_identity(
        run_id,
        source,
        &input,
        runtime_build.as_str(),
        &malformed_sequence,
    )
    .is_err());
}

#[test]
fn dynamic_workflow_claim_identity_is_stable_across_history_progress() {
    let run_id = "claim-identity";
    let source =
        "async function run(ctx, inputs) { return { type: 'complete', output: { ok: true } }; }";
    let input = json!({ "query": "identity-bound" });
    let runtime_build = RuntimeBuildId::new("code-generation-a").unwrap();
    let empty =
        dynamic_workflow_claim_identity(run_id, source, &input, runtime_build.as_str(), &[])
            .unwrap();
    let spec = WorkflowSpec::rust_embedded(
        "a3s-code.dynamic-workflow",
        source_hash(source),
        "ptc",
        "run",
    )
    .with_runtime_build(runtime_build.clone());
    let progressed = vec![
        FlowEventEnvelope::new(
            run_id,
            1,
            uuid::Uuid::new_v4(),
            Utc::now(),
            FlowEvent::RunCreated {
                spec,
                input: input.clone(),
            },
        ),
        FlowEventEnvelope::new(
            run_id,
            2,
            uuid::Uuid::new_v4(),
            Utc::now(),
            FlowEvent::RunStarted,
        ),
    ];
    let resumed = dynamic_workflow_claim_identity(
        run_id,
        source,
        &input,
        runtime_build.as_str(),
        &progressed,
    )
    .unwrap();
    assert_eq!(empty, resumed);
    assert_eq!(empty.domain, DYNAMIC_WORKFLOW_CLAIM_IDENTITY_DOMAIN_V1);
    assert!(!serde_json::to_string(&empty)
        .unwrap()
        .contains("identity-bound"));
}

#[tokio::test]
async fn dynamic_workflow_rejects_a_stale_runtime_generation_before_replay() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    return { type: "wait_until", wait_id: "operator", resume_at: "2099-01-01T00:00:00Z" };
  }
  return { error: "unexpected invocation" };
}
"#;
    let v1 = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_runtime_build_compatibility(RuntimeBuildCompatibility::new(
            RuntimeBuildId::new("code-generation-a").unwrap(),
        ));
    let args = json!({
        "source": source,
        "run_id": "stale-generation-run",
    });
    let first = v1
        .execute(&args, &executor.registry().context())
        .await
        .unwrap();
    assert!(!first.success);

    let v2 = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_runtime_build_compatibility(RuntimeBuildCompatibility::new(
            RuntimeBuildId::new("code-generation-b").unwrap(),
        ));
    let replay = v2
        .execute(&args, &executor.registry().context())
        .await
        .unwrap();
    assert!(!replay.success);
    assert!(
        replay.content.contains("conflicts with existing run")
            || replay.content.contains("runtime build")
    );
}

#[tokio::test]
async fn dynamic_workflow_keeps_legacy_unpinned_terminal_runs_readable() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    return { type: "complete", output: { legacy: true } };
  }
  return { error: "unexpected invocation" };
}
"#;
    let context = executor.registry().context();
    let runtime = Arc::new(DynamicWorkflowRuntime::new(
        Arc::clone(executor.registry()),
        context.clone(),
        source,
    ));
    let store = Arc::new(LocalFileEventStore::new(dynamic_workflow_store_path(
        dir.path(),
    )));
    let legacy_engine = FlowEngine::new(store, runtime);
    let spec = WorkflowSpec::rust_embedded(
        "a3s-code.dynamic-workflow",
        source_hash(source),
        "ptc",
        "run",
    );
    legacy_engine
        .start_with_id("legacy-unpinned-run", spec, json!({}))
        .await
        .unwrap();

    let replay = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .execute(
            &json!({
                "source": source,
                "run_id": "legacy-unpinned-run",
            }),
            &context,
        )
        .await
        .unwrap();
    assert!(replay.success, "{}", replay.content);
    let metadata = replay.metadata.expect("legacy replay metadata");
    assert!(metadata["dynamic_workflow"]["runtime_build_id"].is_null());
    assert_eq!(
        metadata["dynamic_workflow"]["continuation_identity"]["domain"],
        DYNAMIC_WORKFLOW_CONTINUATION_IDENTITY_DOMAIN_V1
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

#[tokio::test]
async fn dynamic_workflow_control_projects_and_settles_durable_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let ledger: Arc<dyn FlowDecisionLedger> = Arc::new(MemoryFlowDecisionLedger::new());
    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") {
    const cancellationRequested = inputs.history.some((entry) =>
      entry.event && entry.event.type === "run_cancellation_requested"
    );
    if (cancellationRequested) return { type: "cancel" };
    return {
      type: "wait_until",
      wait_id: "operator",
      resume_at: "2099-01-01T00:00:00Z",
    };
  }
  return { type: "fail", error: "unexpected invocation" };
}
"#;
    let input = json!({ "secret": "must-not-enter-projection" });
    let control = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_continuation_lease_ledger(Arc::clone(&ledger), 1_000)
        .control(
            "control-cancellation-run",
            source,
            input.clone(),
            &executor.registry().context(),
        )
        .unwrap();

    let suspended = control.drive().await.unwrap();
    assert_eq!(suspended.status, WorkflowRunStatus::Suspended);
    assert!(!suspended.cancellation_requested);
    assert!(matches!(
        suspended.worker_lease,
        FlowDecisionClaimState::Pending { .. }
    ));
    assert!(!suspended.worker_lease.is_live_at(dynamic_workflow_now_ms()));
    let encoded = serde_json::to_string(&suspended).unwrap();
    assert!(!encoded.contains("must-not-enter-projection"));
    assert!(!encoded.contains(source));

    let cancelled = control
        .request_cancellation(Some("operator stop".to_string()))
        .await
        .unwrap();
    assert_eq!(cancelled.status, WorkflowRunStatus::Cancelled);
    assert!(cancelled.cancellation_requested);
    assert!(matches!(
        cancelled.worker_lease,
        FlowDecisionClaimState::Completed { .. }
    ));
    let health = control.health();
    assert_eq!(health.claim_attempts, 2);
    assert_eq!(health.claims, 2);
    assert_eq!(health.takeovers, 1);
    assert_eq!(health.already_completed, 0);
    assert_eq!(health.busy, 0);
    assert_eq!(health.releases, 1);
    assert_eq!(health.completions, 1);
    assert_eq!(health.in_flight, 0);

    let inspected = control.inspect().await.unwrap();
    assert_eq!(inspected, cancelled);
    let history = control.history().await.unwrap();
    assert!(history
        .iter()
        .any(|event| matches!(event.event, FlowEvent::RunCancellationRequested { .. })));
    assert!(history
        .iter()
        .any(|event| matches!(event.event, FlowEvent::RunCancelled { .. })));
    let _ = input;
}

#[tokio::test]
async fn dynamic_workflow_control_diagnostics_combine_local_claims_and_scheduler() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let scheduler = Arc::new(
        TaskScheduler::new(crate::task_scheduler::TaskSchedulerConfig {
            max_active: 1,
            aging_interval_ms: 10,
        })
        .unwrap(),
    );
    let ledger: Arc<dyn FlowDecisionLedger> = Arc::new(MemoryFlowDecisionLedger::new());
    let source = r#"
async function run(ctx, inputs) {
  if (inputs.kind === "workflow") return { type: "complete", output: { ok: true } };
  return { type: "fail", error: "unexpected invocation" };
}
"#;
    let control = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_task_scheduler(Arc::clone(&scheduler), true)
        .with_continuation_lease_ledger(Arc::clone(&ledger), 1_000)
        .control(
            "control-diagnostics-run",
            source,
            json!({}),
            &executor.registry().context(),
        )
        .unwrap();

    let initial_diagnostics = control.diagnostics().await.unwrap();
    assert_eq!(initial_diagnostics.workflow.claim_attempts, 0);
    assert_eq!(initial_diagnostics.scheduler_quota.unwrap().active, 0);

    let completed = control.drive().await.unwrap();
    assert_eq!(completed.status, WorkflowRunStatus::Completed);
    let diagnostics = control.diagnostics().await.unwrap();
    assert_eq!(diagnostics.workflow.claim_attempts, 1);
    assert_eq!(diagnostics.workflow.claims, 1);
    assert_eq!(diagnostics.workflow.completions, 1);
    assert_eq!(diagnostics.workflow.in_flight, 0);
    let scheduler_health = diagnostics.scheduler.expect("scheduler is configured");
    assert_eq!(scheduler_health.max_active, 1);
    assert_eq!(scheduler_health.active, 0);
    assert_eq!(scheduler_health.pending, 0);
    let scheduler_quota = diagnostics
        .scheduler_quota
        .expect("global workflow quota is projected");
    assert_eq!(scheduler_quota.active, 0);
    assert_eq!(scheduler_quota.pending, 0);
    assert!(!scheduler_quota.blocked);
    scheduler.shutdown().await;
}

#[tokio::test]
async fn dynamic_workflow_control_reuses_a_host_owned_event_store() {
    let dir = tempfile::tempdir().unwrap();
    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let store: Arc<dyn FlowEventStore> = Arc::new(InMemoryEventStore::new());
    let source =
        "async function run(ctx, inputs) { return { type: 'complete', output: { hosted: true } }; }";
    let tool = DynamicWorkflowTool::new(Arc::clone(executor.registry()))
        .with_flow_event_store(Arc::clone(&store));
    let args = json!({
        "source": source,
        "run_id": "host-owned-store-run",
    });
    let result = tool
        .execute(&args, &executor.registry().context())
        .await
        .unwrap();
    assert!(result.success, "{}", result.content);
    let control = tool
        .control(
            "host-owned-store-run",
            source,
            json!({}),
            &executor.registry().context(),
        )
        .unwrap();
    let snapshot = control.inspect().await.unwrap();
    assert_eq!(snapshot.status, WorkflowRunStatus::Completed);
    assert_eq!(snapshot.last_sequence, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_process_flow_store_serializes_independent_writers() {
    let directory = tempfile::tempdir().unwrap();
    let first = Arc::new(CrossProcessFlowEventStore::new(directory.path()));
    let second = Arc::new(CrossProcessFlowEventStore::new(directory.path()));
    let spec = WorkflowSpec::rust_embedded(
        "a3s-code.dynamic-workflow",
        source_hash("cross-process-store"),
        "ptc",
        "run",
    )
    .with_runtime_build(
        RuntimeBuildId::new(DYNAMIC_WORKFLOW_RUNTIME_BUILD_ID.to_string()).unwrap(),
    );
    first
        .append(
            "cross-process-store",
            FlowEvent::RunCreated {
                spec,
                input: json!({}),
            },
        )
        .await
        .unwrap();

    let (left, right) = tokio::join!(
        first.append_if_sequence("cross-process-store", 1, FlowEvent::RunStarted,),
        second.append_if_sequence("cross-process-store", 1, FlowEvent::RunStarted,),
    );
    assert_eq!(left.is_ok() as u8 + right.is_ok() as u8, 1);
    let conflict = if left.is_err() { left } else { right };
    assert!(matches!(
        conflict,
        Err(a3s_flow::FlowError::EventConflict {
            expected_sequence: 1,
            actual_sequence: 2,
            ..
        })
    ));
    let history = second.list("cross-process-store").await.unwrap();
    assert_eq!(history.len(), 2);
    assert!(matches!(history[1].event, FlowEvent::RunStarted));
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
