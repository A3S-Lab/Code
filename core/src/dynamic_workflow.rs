//! A3S Flow-backed dynamic workflow runtime.
//!
//! `DynamicWorkflowRuntime` lets hosts run a sandboxed PTC script as an A3S
//! Flow runtime. Flow owns durable replay and step lifecycle; A3S Code's
//! existing `program` tool remains the sandbox and tool-call boundary.

use crate::llm::{ModelGenerationAdmission, ModelGenerationConcurrency};
use crate::tools::{
    registry_tool_invoker, Tool, ToolContext, ToolInvoker, ToolOutput, ToolRegistry, ToolResult,
};
use crate::{
    agent::AgentEvent,
    flow_graph::FlowGraphObserver,
    planning::{Complexity, ExecutionPlan, Task, TaskStatus},
};
use a3s_flow::{
    FanoutFlowEventObserver, FlowEngine, FlowEvent, FlowEventEnvelope, FlowEventObserver,
    FlowEventStore, FlowRuntime, InMemoryEventStore, LocalFileEventStore, RuntimeCommand,
    StepInvocation, StepStatus, WorkflowInvocation, WorkflowRunSnapshot, WorkflowRunStatus,
    WorkflowSpec,
};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{broadcast, Mutex};

const DYNAMIC_WORKFLOW_TOOL: &str = "dynamic_workflow";
const GENERATE_OBJECT_TOOL: &str = "generate_object";
const PROGRAM_TOOL: &str = "program";
const TASK_TOOL: &str = "task";
const PARALLEL_TASK_TOOL: &str = "parallel_task";
const MAX_INLINE_RETRY_RESUMES: usize = 8;
const MAX_INLINE_RETRY_DELAY: Duration = Duration::from_secs(5);

/// Project-relative directory used for durable dynamic workflow history.
pub const DYNAMIC_WORKFLOW_STORE_RELATIVE_PATH: &str = ".a3s/workflow";

/// Resolve the durable dynamic workflow history directory for a local workspace.
pub fn dynamic_workflow_store_path(workspace_root: impl AsRef<Path>) -> PathBuf {
    workspace_root
        .as_ref()
        .join(DYNAMIC_WORKFLOW_STORE_RELATIVE_PATH)
}

/// Recover one completed step output from the exact durable workflow run.
///
/// Recovery is bound to the requested run ID and the original input query. It
/// never acts as a cross-run query cache and never promotes an incomplete step.
pub async fn recover_dynamic_workflow_step_output(
    workspace_root: impl AsRef<Path>,
    run_id: &str,
    expected_query: &str,
    step_id: &str,
) -> Result<Option<Value>> {
    if !safe_workflow_run_id(run_id) || expected_query.is_empty() || step_id.is_empty() {
        return Ok(None);
    }
    let workspace_root = workspace_root.as_ref();
    let store_root = dynamic_workflow_store_path(workspace_root);
    let log_path = store_root.join(format!("{run_id}.jsonl"));
    match tokio::fs::symlink_metadata(&log_path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Ok(None)
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("inspect dynamic workflow history {}", log_path.display())
            })
        }
    }
    validate_dynamic_workflow_directory(&workspace_root.join(".a3s"), ".a3s").await?;
    validate_dynamic_workflow_directory(&store_root, ".a3s/workflow").await?;
    validate_dynamic_workflow_log(&log_path).await?;

    let events = LocalFileEventStore::new(store_root).list(run_id).await?;
    let input_matches = events.iter().any(|envelope| {
        matches!(
            &envelope.event,
            FlowEvent::RunCreated { input, .. }
                if input.get("query").and_then(Value::as_str) == Some(expected_query)
        )
    });
    if !input_matches {
        return Ok(None);
    }
    Ok(events
        .iter()
        .rev()
        .find_map(|envelope| match &envelope.event {
            FlowEvent::StepCompleted {
                step_id: completed_step_id,
                output,
            } if completed_step_id == step_id => Some(output.clone()),
            _ => None,
        }))
}

/// Limits forwarded to the underlying PTC `program` tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DynamicWorkflowScriptLimits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tool_calls: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_bytes: Option<usize>,
    /// Maximum independently session-bound model generations active at once.
    /// This orchestration limit is not forwarded to the PTC program sandbox.
    #[serde(default, skip_serializing)]
    pub max_concurrent_generations: Option<usize>,
}

/// Runs A3S Flow workflow and step invocations through a sandboxed PTC script.
#[derive(Clone)]
pub struct DynamicWorkflowRuntime {
    invoker: Arc<dyn ToolInvoker>,
    context: ToolContext,
    source: Arc<str>,
    allowed_tools: Vec<String>,
    limits: DynamicWorkflowScriptLimits,
    parallel_generation_admission: Option<ModelGenerationAdmission>,
}

impl DynamicWorkflowRuntime {
    pub fn new(
        registry: Arc<ToolRegistry>,
        context: ToolContext,
        source: impl Into<String>,
    ) -> Self {
        let allowed_tools = default_allowed_tools(&registry);
        // Session/agent callers install the governed gateway in ToolContext.
        // The raw registry adapter is retained only for explicit low-level
        // callers that construct this public runtime outside an AgentSession.
        let invoker = context
            .tool_invoker()
            .unwrap_or_else(|| registry_tool_invoker(registry));
        Self {
            invoker,
            context,
            source: Arc::from(source.into()),
            allowed_tools,
            limits: DynamicWorkflowScriptLimits::default(),
            parallel_generation_admission: None,
        }
    }

    pub fn with_allowed_tools(mut self, allowed_tools: impl IntoIterator<Item = String>) -> Self {
        self.allowed_tools = sanitize_allowed_tools(allowed_tools);
        self
    }

    pub fn with_limits(mut self, limits: DynamicWorkflowScriptLimits) -> Self {
        let generation_concurrency = limits.max_concurrent_generations.unwrap_or(1).clamp(1, 4);
        self.parallel_generation_admission = NonZeroUsize::new(generation_concurrency)
            .filter(|maximum| maximum.get() > 1)
            .map(|maximum| {
                ModelGenerationAdmission::new(ModelGenerationConcurrency::bounded(maximum))
            });
        self.limits = limits;
        self
    }

    async fn run_script(
        &self,
        payload: Value,
        context: &ToolContext,
    ) -> a3s_flow::Result<ToolResult> {
        let mut args = json!({
            "type": "script",
            "language": "javascript",
            "source": self.source.as_ref(),
            "inputs": payload,
            "allowed_tools": self.allowed_tools,
        });
        if let Some(object) = args.as_object_mut() {
            if let Ok(Value::Object(limits)) = serde_json::to_value(&self.limits) {
                if !limits.is_empty() {
                    object.insert("limits".to_string(), Value::Object(limits));
                }
            }
        }

        let result = self
            .invoker
            .invoke(context.nested_tool_invocation(PROGRAM_TOOL, args), context)
            .await;
        if result.exit_code != 0 {
            return Err(a3s_flow::FlowError::Runtime(result.output));
        }
        Ok(result)
    }

    async fn context_for_step(
        &self,
        run_id: &str,
        step_id: &str,
        step_name: &str,
    ) -> a3s_flow::Result<ToolContext> {
        if step_name != GENERATE_OBJECT_TOOL {
            return Ok(self.context.clone());
        }
        if let (Some(admission), Some(client)) = (
            self.parallel_generation_admission.as_ref(),
            self.context.llm_client(),
        ) {
            let fork_id = format!("{run_id}:{step_id}");
            if let Some(forked_client) = client.fork_for_session(&fork_id) {
                let permit = admission
                    .acquire(&self.context.cancellation_token())
                    .await
                    .map_err(|error| {
                        a3s_flow::FlowError::Runtime(format!(
                            "parallel model-generation admission failed before workflow step: {error}"
                        ))
                    })?;
                return self
                    .context
                    .clone()
                    .with_llm_client(forked_client)
                    .with_model_generation_permit(admission.clone(), Arc::new(permit))
                    .map_err(|error| {
                        a3s_flow::FlowError::Runtime(format!(
                            "bind parallel model-generation admission to workflow step: {error}"
                        ))
                    });
            }
        }
        let Some(admission) = self.context.model_generation_admission() else {
            return Ok(self.context.clone());
        };
        let permit = admission
            .acquire(&self.context.cancellation_token())
            .await
            .map_err(|error| {
                a3s_flow::FlowError::Runtime(format!(
                    "model-generation admission failed before workflow step: {error}"
                ))
            })?;
        self.context
            .clone()
            .with_model_generation_permit(admission, Arc::new(permit))
            .map_err(|error| {
                a3s_flow::FlowError::Runtime(format!(
                    "bind model-generation admission to workflow step: {error}"
                ))
            })
    }

    async fn run_tool_step(&self, tool_name: &str, args: Value) -> a3s_flow::Result<Value> {
        let result = self
            .invoker
            .invoke(
                self.context
                    .nested_tool_invocation(tool_name.to_string(), args),
                &self.context,
            )
            .await;
        if result.exit_code != 0 {
            return Err(a3s_flow::FlowError::Runtime(result.output));
        }
        Ok(json!({
            "tool": result.name,
            "output": result.output,
            "exit_code": result.exit_code,
            "metadata": result.metadata,
        }))
    }
}

#[async_trait]
impl FlowRuntime for DynamicWorkflowRuntime {
    async fn run_workflow(
        &self,
        invocation: WorkflowInvocation,
    ) -> a3s_flow::Result<RuntimeCommand> {
        let payload = invocation_payload("workflow", &invocation.run_id, &invocation.history)
            .with("input", invocation.input);
        let result = self.run_script(payload.into_value(), &self.context).await?;
        serde_json::from_value(script_result(&result)?).map_err(a3s_flow::FlowError::from)
    }

    async fn run_step(&self, invocation: StepInvocation) -> a3s_flow::Result<Value> {
        if matches!(
            invocation.step_name.as_str(),
            TASK_TOOL | PARALLEL_TASK_TOOL
        ) {
            return self
                .run_tool_step(&invocation.step_name, invocation.input)
                .await;
        }

        let context = self
            .context_for_step(
                &invocation.run_id,
                &invocation.step_id,
                &invocation.step_name,
            )
            .await?;
        let payload = invocation_payload("step", &invocation.run_id, &invocation.history)
            .with("step_id", invocation.step_id)
            .with("step_name", invocation.step_name)
            .with("input", invocation.input);
        let result = self.run_script(payload.into_value(), &context).await?;
        script_result(&result)
    }
}

struct WorkflowProgressState {
    tasks: Vec<Task>,
}

impl WorkflowProgressState {
    fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    fn upsert_step(
        &mut self,
        step_id: &str,
        step_name: &str,
        input: Option<&Value>,
        status: TaskStatus,
    ) {
        let content = workflow_step_description(step_id, step_name, input);
        if let Some(task) = self.tasks.iter_mut().find(|task| task.id == step_id) {
            task.content = content;
            task.status = status;
            task.tool = Some(step_name.to_string());
        } else {
            self.tasks
                .push(Task::new(step_id.to_string(), content).with_tool(step_name));
            if let Some(task) = self.tasks.last_mut() {
                task.status = status;
            }
        }
    }

    fn mark_status(&mut self, step_id: &str, status: TaskStatus) {
        if let Some(task) = self.tasks.iter_mut().find(|task| task.id == step_id) {
            task.status = status;
        }
    }

    fn step_position(&self, step_id: &str) -> (usize, usize) {
        let total = self.tasks.len().max(1);
        let number = self
            .tasks
            .iter()
            .position(|task| task.id == step_id)
            .map(|idx| idx + 1)
            .unwrap_or(total);
        (number, total)
    }

    fn step_description(&self, step_id: &str) -> String {
        self.tasks
            .iter()
            .find(|task| task.id == step_id)
            .map(|task| task.content.clone())
            .unwrap_or_else(|| step_id.to_string())
    }
}

struct AgentEventFlowObserver {
    tx: broadcast::Sender<AgentEvent>,
    session_id: String,
    state: Mutex<WorkflowProgressState>,
}

impl AgentEventFlowObserver {
    fn new(tx: broadcast::Sender<AgentEvent>, session_id: String) -> Self {
        Self {
            tx,
            session_id,
            state: Mutex::new(WorkflowProgressState::new()),
        }
    }

    fn emit_task_update(&self, tasks: &[Task]) {
        let _ = self.tx.send(AgentEvent::TaskUpdated {
            session_id: self.session_id.clone(),
            tasks: tasks.to_vec(),
        });
    }
}

#[async_trait]
impl FlowEventObserver for AgentEventFlowObserver {
    async fn observe(&self, envelope: FlowEventEnvelope) {
        match envelope.event {
            FlowEvent::RunStarted => {
                let _ = self.tx.send(AgentEvent::PlanningStart {
                    prompt: "dynamic_workflow".to_string(),
                });
            }
            FlowEvent::StepCreated {
                step_id,
                step_name,
                input,
                ..
            } => {
                let mut state = self.state.lock().await;
                state.upsert_step(&step_id, &step_name, Some(&input), TaskStatus::Pending);
                self.emit_task_update(&state.tasks);
                let mut plan = ExecutionPlan::new("dynamic workflow", Complexity::Medium);
                for task in state.tasks.iter().cloned() {
                    plan.add_step(task);
                }
                let _ = self.tx.send(AgentEvent::PlanningEnd {
                    estimated_steps: plan.steps.len(),
                    plan,
                });
            }
            FlowEvent::StepStarted { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::InProgress);
                self.emit_task_update(&state.tasks);
                let (step_number, total_steps) = state.step_position(&step_id);
                let _ = self.tx.send(AgentEvent::StepStart {
                    description: state.step_description(&step_id),
                    step_id,
                    step_number,
                    total_steps,
                });
            }
            FlowEvent::StepCompleted { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::Completed);
                self.emit_task_update(&state.tasks);
                let (step_number, total_steps) = state.step_position(&step_id);
                let _ = self.tx.send(AgentEvent::StepEnd {
                    step_id,
                    status: TaskStatus::Completed,
                    step_number,
                    total_steps,
                });
            }
            FlowEvent::StepRetrying { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::InProgress);
                self.emit_task_update(&state.tasks);
            }
            FlowEvent::StepFailed { step_id, .. } => {
                let mut state = self.state.lock().await;
                state.mark_status(&step_id, TaskStatus::Failed);
                self.emit_task_update(&state.tasks);
                let (step_number, total_steps) = state.step_position(&step_id);
                let _ = self.tx.send(AgentEvent::StepEnd {
                    step_id,
                    status: TaskStatus::Failed,
                    step_number,
                    total_steps,
                });
            }
            FlowEvent::RunFailed { .. } => {
                let mut state = self.state.lock().await;
                for task in &mut state.tasks {
                    if task.status.is_active() {
                        task.status = TaskStatus::Failed;
                    }
                }
                self.emit_task_update(&state.tasks);
            }
            FlowEvent::RunCancelled { .. } => {
                let mut state = self.state.lock().await;
                for task in &mut state.tasks {
                    if task.status.is_active() {
                        task.status = TaskStatus::Cancelled;
                    }
                }
                self.emit_task_update(&state.tasks);
            }
            _ => {}
        }
    }
}

fn workflow_step_description(step_id: &str, step_name: &str, input: Option<&Value>) -> String {
    if matches!(step_name, TASK_TOOL | PARALLEL_TASK_TOOL) {
        let count = input
            .and_then(|value| value.get("tasks"))
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        if count > 0 {
            return format!("Fan out {count} parallel subagent task(s)");
        }
    }

    input
        .and_then(|value| value.get("description").or_else(|| value.get("title")))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            if step_name == step_id {
                step_id.to_string()
            } else {
                format!("{step_name}: {step_id}")
            }
        })
}

/// Model-visible tool that executes a dynamic workflow through A3S Flow.
pub struct DynamicWorkflowTool {
    registry: DynamicWorkflowRegistry,
    graph_observer: Option<FlowGraphObserver>,
}

enum DynamicWorkflowRegistry {
    Standalone(Arc<ToolRegistry>),
    RegistryBound(Weak<ToolRegistry>),
}

impl DynamicWorkflowRegistry {
    fn resolve(&self) -> Option<Arc<ToolRegistry>> {
        match self {
            Self::Standalone(registry) => Some(Arc::clone(registry)),
            Self::RegistryBound(registry) => registry.upgrade(),
        }
    }
}

impl DynamicWorkflowTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry: DynamicWorkflowRegistry::Standalone(registry),
            graph_observer: None,
        }
    }

    fn new_registry_bound(registry: Arc<ToolRegistry>) -> Self {
        Self {
            registry: DynamicWorkflowRegistry::RegistryBound(Arc::downgrade(&registry)),
            graph_observer: None,
        }
    }

    /// Project committed Flow events into an optional reactive state graph.
    /// A3S Flow remains the workflow execution source of truth.
    pub fn with_graph_observer(mut self, observer: FlowGraphObserver) -> Self {
        self.graph_observer = Some(observer);
        self
    }
}

#[async_trait]
impl Tool for DynamicWorkflowTool {
    fn name(&self) -> &str {
        DYNAMIC_WORKFLOW_TOOL
    }

    fn description(&self) -> &str {
        "Run a local dynamic workflow with A3S Flow. The workflow source is a sandboxed JavaScript PTC script that may call allowed ctx tools; A3S Flow records workflow and step history."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "source": {
                    "type": "string",
                    "description": "JavaScript PTC source defining async function run(ctx, inputs). For inputs.kind='workflow', return a Flow command: {type:'complete', output}, {type:'fail', error}, {type:'schedule_step', step_id, step_name, input, retry?}, or {type:'schedule_steps', steps:[...]}. For inputs.kind='step', return the step JSON output. A scheduled step with step_name='task' bypasses QuickJS and calls the host task tool directly with input as its arguments; legacy parallel_task steps remain readable."
                },
                "input": {
                    "type": "object",
                    "description": "Initial workflow input."
                },
                "run_id": {
                    "type": "string",
                    "description": "Optional durable run id. Reusing it with the same source and input is idempotent."
                },
                "allowed_tools": {
                    "type": "array",
                    "description": "Tool names the workflow script may call through ctx. Defaults to all registered tools except program, dynamic_workflow, and the legacy parallel_task alias. Direct task fan-out is blocked inside QuickJS; schedule a host task step instead. Login-registered tools such as runtime are allowed when present.",
                    "items": { "type": "string" }
                },
                "limits": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "timeoutMs": { "type": "integer", "minimum": 1 },
                        "maxToolCalls": { "type": "integer", "minimum": 1 },
                        "maxOutputBytes": { "type": "integer", "minimum": 1 },
                        "maxConcurrentGenerations": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 4,
                            "description": "Optional bounded fan-out for independently session-bound generate_object steps. Providers without session forking remain single-flight."
                        }
                    }
                }
            },
            "required": ["source"]
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(registry) = self.registry.resolve() else {
            return Ok(ToolOutput::error("Tool registry is closed"));
        };
        let Some(source) = args.get("source").and_then(Value::as_str) else {
            return Ok(ToolOutput::error("dynamic_workflow requires source"));
        };
        let input = args.get("input").cloned().unwrap_or_else(|| json!({}));
        let allowed_tools = args
            .get("allowed_tools")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| default_allowed_tools(&registry));
        let limits = args
            .get("limits")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();

        let runtime = Arc::new(
            DynamicWorkflowRuntime::new(registry, ctx.clone(), source)
                .with_allowed_tools(allowed_tools)
                .with_limits(limits),
        );
        let requested_run_id = args.get("run_id").and_then(Value::as_str);
        let store = match flow_store_for_context(ctx, requested_run_id).await {
            Ok(store) => store,
            Err(error) => return Ok(ToolOutput::error(error.to_string())),
        };
        let mut observers: Vec<Arc<dyn FlowEventObserver>> = Vec::new();
        if let Some(tx) = ctx.agent_event_tx.clone() {
            observers.push(Arc::new(AgentEventFlowObserver::new(
                tx,
                ctx.session_id.clone().unwrap_or_default(),
            )));
        }
        if let Some(observer) = &self.graph_observer {
            observers.push(Arc::new(observer.clone()));
        }
        let engine = if observers.is_empty() {
            FlowEngine::new(store, runtime)
        } else {
            FlowEngine::builder(runtime)
                .with_store(store)
                .with_observer(Arc::new(FanoutFlowEventObserver::from_observers(observers)))
                .build()
        };
        let source_hash = source_hash(source);
        let spec = WorkflowSpec::rust_embedded(
            "a3s-code.dynamic-workflow",
            source_hash.as_str(),
            "ptc",
            "run",
        );

        let run_id = match requested_run_id {
            Some(run_id) => match engine.start_with_id(run_id, spec, input).await {
                Ok(run_id) => run_id,
                Err(err) => return Ok(ToolOutput::error(err.to_string())),
            },
            None => match engine.start(spec, input).await {
                Ok(run_id) => run_id,
                Err(err) => return Ok(ToolOutput::error(err.to_string())),
            },
        };

        let snapshot = match drive_inline_retries(&engine, &run_id, ctx).await {
            Ok(snapshot) => snapshot,
            Err(err) => return Ok(ToolOutput::error(err.to_string())),
        };
        let history = match engine.history(&run_id).await {
            Ok(history) => history,
            Err(err) => return Ok(ToolOutput::error(err.to_string())),
        };

        let output = match &snapshot.output {
            Some(output) => {
                serde_json::to_string_pretty(output).unwrap_or_else(|_| output.to_string())
            }
            None => snapshot
                .error
                .clone()
                .unwrap_or_else(|| format!("workflow status: {:?}", snapshot.status)),
        };

        let status = snapshot.status;
        let metadata = json!({
            "dynamic_workflow": {
                "run_id": run_id,
                "status": format!("{:?}", snapshot.status),
                "last_sequence": snapshot.last_sequence,
                "source_hash": source_hash,
                "snapshot": snapshot,
                "history": history,
            }
        });
        let output = match status {
            WorkflowRunStatus::Completed => ToolOutput::success(output),
            WorkflowRunStatus::Failed | WorkflowRunStatus::Cancelled => ToolOutput::error(output),
            _ => ToolOutput::error(format!(
                "dynamic_workflow ended without a terminal result: {status:?}; {output}"
            )),
        };

        Ok(output.with_metadata(metadata))
    }
}

/// Drive short, persisted step retries inside the originating tool call.
///
/// A3S Flow deliberately suspends at a delayed retry boundary. Interactive
/// waits and hooks must remain suspended for an external host, but a bounded
/// retry delay is ordinary fault recovery: returning it as a terminal tool
/// error forces every caller to reimplement the scheduler and previously made
/// DeepResearch abandon its event-sourced run. Retry attempts and their delay
/// remain authoritative in the Flow journal; this helper only waits for the
/// due time and asks the engine to replay the same run.
async fn drive_inline_retries(
    engine: &FlowEngine,
    run_id: &str,
    ctx: &ToolContext,
) -> Result<WorkflowRunSnapshot> {
    for _ in 0..MAX_INLINE_RETRY_RESUMES {
        let snapshot = engine.snapshot(run_id).await?;
        if snapshot.status.is_terminal() {
            return Ok(snapshot);
        }
        let Some(retry_after) = snapshot
            .steps
            .values()
            .filter(|step| step.status == StepStatus::Pending)
            .filter_map(|step| step.retry_after)
            .min()
        else {
            return Ok(snapshot);
        };
        let delay = retry_after
            .signed_duration_since(Utc::now())
            .to_std()
            .unwrap_or_default();
        if delay > MAX_INLINE_RETRY_DELAY {
            return Ok(snapshot);
        }
        let cancellation = ctx.cancellation_token();
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                anyhow::bail!("dynamic_workflow cancelled while waiting for a scheduled retry");
            }
            _ = tokio::time::sleep(delay) => {}
        }
        engine.drive(run_id).await?;
    }
    engine.snapshot(run_id).await.map_err(Into::into)
}

pub fn register_dynamic_workflow(registry: &Arc<ToolRegistry>) {
    registry.register(Arc::new(DynamicWorkflowTool::new_registry_bound(
        Arc::clone(registry),
    )));
}

async fn flow_store_for_context(
    ctx: &ToolContext,
    requested_run_id: Option<&str>,
) -> Result<Arc<dyn FlowEventStore>> {
    match ctx.workspace_services.local_root() {
        Some(root) => {
            let store = dynamic_workflow_store_path(root);
            validate_dynamic_workflow_directory(&root.join(".a3s"), ".a3s").await?;
            validate_dynamic_workflow_directory(&store, ".a3s/workflow").await?;
            if let Some(run_id) = requested_run_id.filter(|run_id| safe_workflow_run_id(run_id)) {
                validate_dynamic_workflow_log(&store.join(format!("{run_id}.jsonl"))).await?;
            }
            Ok(Arc::new(LocalFileEventStore::new(store)))
        }
        None => Ok(Arc::new(InMemoryEventStore::new())),
    }
}

async fn validate_dynamic_workflow_directory(path: &Path, label: &str) -> Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            anyhow::bail!("refusing to use symlinked dynamic workflow directory {label}")
        }
        Ok(metadata) if !metadata.is_dir() => {
            anyhow::bail!("dynamic workflow path {label} exists but is not a directory")
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect dynamic workflow path {label}")),
    }
}

async fn validate_dynamic_workflow_log(path: &Path) -> Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => anyhow::bail!(
            "refusing to read or append symlinked dynamic workflow history {}",
            path.display()
        ),
        Ok(metadata) if !metadata.is_file() => anyhow::bail!(
            "dynamic workflow history path {} exists but is not a file",
            path.display()
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("inspect dynamic workflow history {}", path.display())),
    }
}

fn safe_workflow_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

struct PayloadBuilder {
    value: Map<String, Value>,
}

impl PayloadBuilder {
    fn with(mut self, key: &str, value: impl Serialize) -> Self {
        self.value.insert(
            key.to_string(),
            serde_json::to_value(value).unwrap_or(Value::Null),
        );
        self
    }

    fn into_value(self) -> Value {
        Value::Object(self.value)
    }
}

fn invocation_payload(kind: &str, run_id: &str, history: &[FlowEventEnvelope]) -> PayloadBuilder {
    let mut value = Map::new();
    value.insert("kind".to_string(), json!(kind));
    value.insert("run_id".to_string(), json!(run_id));
    value.insert("history".to_string(), json!(history));
    value.insert("step_outputs".to_string(), completed_step_outputs(history));
    value.insert("step_failures".to_string(), failed_step_outputs(history));
    PayloadBuilder { value }
}

fn completed_step_outputs(history: &[FlowEventEnvelope]) -> Value {
    let mut outputs = Map::new();
    for envelope in history {
        if let FlowEvent::StepCompleted { step_id, output } = &envelope.event {
            outputs.insert(step_id.clone(), output.clone());
        }
    }
    Value::Object(outputs)
}

fn failed_step_outputs(history: &[FlowEventEnvelope]) -> Value {
    let mut outputs = Map::new();
    for envelope in history {
        if let FlowEvent::StepFailed {
            step_id,
            attempt,
            error,
        } = &envelope.event
        {
            outputs.insert(
                step_id.clone(),
                json!({
                    "attempt": attempt,
                    "error": error,
                }),
            );
        }
    }
    Value::Object(outputs)
}

fn script_result(result: &ToolResult) -> a3s_flow::Result<Value> {
    result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("script_result"))
        .cloned()
        .ok_or_else(|| {
            a3s_flow::FlowError::Runtime(
                "PTC program result did not include script_result metadata".to_string(),
            )
        })
}

fn default_allowed_tools(registry: &ToolRegistry) -> Vec<String> {
    sanitize_allowed_tools(registry.list())
}

fn sanitize_allowed_tools(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut tools = items.into_iter().collect::<BTreeSet<_>>();
    tools.remove(PROGRAM_TOOL);
    tools.remove(DYNAMIC_WORKFLOW_TOOL);
    tools.remove(PARALLEL_TASK_TOOL);
    tools.into_iter().collect()
}

fn source_hash(source: &str) -> String {
    sha256::digest(source.as_bytes())
}

#[cfg(test)]
#[path = "dynamic_workflow/tests.rs"]
mod tests;
