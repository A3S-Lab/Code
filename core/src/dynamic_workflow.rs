//! A3S Flow-backed dynamic workflow runtime.
//!
//! `DynamicWorkflowRuntime` lets hosts run a sandboxed PTC script as an A3S
//! Flow runtime. Flow owns durable replay and step lifecycle; A3S Code's
//! existing `program` tool remains the sandbox and tool-call boundary.

use crate::tools::{Tool, ToolContext, ToolOutput, ToolRegistry, ToolResult};
use a3s_flow::{
    FlowEngine, FlowEvent, FlowEventEnvelope, FlowEventStore, FlowRuntime, InMemoryEventStore,
    LocalFileEventStore, RuntimeCommand, StepInvocation, WorkflowInvocation, WorkflowRunStatus,
    WorkflowSpec,
};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

const DYNAMIC_WORKFLOW_TOOL: &str = "dynamic_workflow";
const PROGRAM_TOOL: &str = "program";
const PARALLEL_TASK_TOOL: &str = "parallel_task";

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
}

/// Runs A3S Flow workflow and step invocations through a sandboxed PTC script.
#[derive(Clone)]
pub struct DynamicWorkflowRuntime {
    registry: Arc<ToolRegistry>,
    context: ToolContext,
    source: Arc<str>,
    allowed_tools: Vec<String>,
    limits: DynamicWorkflowScriptLimits,
}

impl DynamicWorkflowRuntime {
    pub fn new(
        registry: Arc<ToolRegistry>,
        context: ToolContext,
        source: impl Into<String>,
    ) -> Self {
        let allowed_tools = default_allowed_tools(&registry);
        Self {
            registry,
            context,
            source: Arc::from(source.into()),
            allowed_tools,
            limits: DynamicWorkflowScriptLimits::default(),
        }
    }

    pub fn with_allowed_tools(mut self, allowed_tools: impl IntoIterator<Item = String>) -> Self {
        self.allowed_tools = sanitize_allowed_tools(allowed_tools);
        self
    }

    pub fn with_limits(mut self, limits: DynamicWorkflowScriptLimits) -> Self {
        self.limits = limits;
        self
    }

    async fn run_script(&self, payload: Value) -> a3s_flow::Result<ToolResult> {
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
            .registry
            .execute_with_context(PROGRAM_TOOL, &args, &self.context)
            .await
            .map_err(|err| a3s_flow::FlowError::Runtime(err.to_string()))?;
        if result.exit_code != 0 {
            return Err(a3s_flow::FlowError::Runtime(result.output));
        }
        Ok(result)
    }

    async fn run_tool_step(&self, tool_name: &str, args: Value) -> a3s_flow::Result<Value> {
        let result = self
            .registry
            .execute_with_context(tool_name, &args, &self.context)
            .await
            .map_err(|err| a3s_flow::FlowError::Runtime(err.to_string()))?;
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
        let result = self.run_script(payload.into_value()).await?;
        serde_json::from_value(script_result(&result)?).map_err(a3s_flow::FlowError::from)
    }

    async fn run_step(&self, invocation: StepInvocation) -> a3s_flow::Result<Value> {
        if invocation.step_name == PARALLEL_TASK_TOOL {
            return self
                .run_tool_step(PARALLEL_TASK_TOOL, invocation.input)
                .await;
        }

        let payload = invocation_payload("step", &invocation.run_id, &invocation.history)
            .with("step_id", invocation.step_id)
            .with("step_name", invocation.step_name)
            .with("input", invocation.input);
        let result = self.run_script(payload.into_value()).await?;
        script_result(&result)
    }
}

/// Model-visible tool that executes a dynamic workflow through A3S Flow.
pub struct DynamicWorkflowTool {
    registry: Arc<ToolRegistry>,
}

impl DynamicWorkflowTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self { registry }
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
                    "description": "JavaScript PTC source defining async function run(ctx, inputs). For inputs.kind='workflow', return a Flow command: {type:'complete', output}, {type:'fail', error}, {type:'schedule_step', step_id, step_name, input, retry?}, or {type:'schedule_steps', steps:[...]}. For inputs.kind='step', return the step JSON output. A scheduled step with step_name='parallel_task' bypasses QuickJS and calls the host parallel_task tool directly with input as its arguments."
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
                    "description": "Tool names the workflow script may call through ctx. Defaults to all registered tools except program, dynamic_workflow, and parallel_task. Login-registered tools such as runtime are allowed when present.",
                    "items": { "type": "string" }
                },
                "limits": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "timeoutMs": { "type": "integer", "minimum": 1 },
                        "maxToolCalls": { "type": "integer", "minimum": 1 },
                        "maxOutputBytes": { "type": "integer", "minimum": 1 }
                    }
                }
            },
            "required": ["source"]
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
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
            .unwrap_or_else(|| default_allowed_tools(&self.registry));
        let limits = args
            .get("limits")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();

        let runtime = Arc::new(
            DynamicWorkflowRuntime::new(Arc::clone(&self.registry), ctx.clone(), source)
                .with_allowed_tools(allowed_tools)
                .with_limits(limits),
        );
        let store = flow_store_for_context(ctx);
        let engine = FlowEngine::new(store, runtime);
        let source_hash = source_hash(source);
        let spec = WorkflowSpec::rust_embedded(
            "a3s-code.dynamic-workflow",
            source_hash.as_str(),
            "ptc",
            "run",
        );

        let run_id = match args.get("run_id").and_then(Value::as_str) {
            Some(run_id) => match engine.start_with_id(run_id, spec, input).await {
                Ok(run_id) => run_id,
                Err(err) => return Ok(ToolOutput::error(err.to_string())),
            },
            None => match engine.start(spec, input).await {
                Ok(run_id) => run_id,
                Err(err) => return Ok(ToolOutput::error(err.to_string())),
            },
        };

        let snapshot = match engine.snapshot(&run_id).await {
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
            _ => ToolOutput::success(output),
        };

        Ok(output.with_metadata(metadata))
    }
}

pub fn register_dynamic_workflow(registry: &Arc<ToolRegistry>) {
    registry.register(Arc::new(DynamicWorkflowTool::new(Arc::clone(registry))));
}

fn flow_store_for_context(ctx: &ToolContext) -> Arc<dyn FlowEventStore> {
    match ctx.workspace_services.local_root() {
        Some(root) => Arc::new(LocalFileEventStore::new(
            root.join(".a3s-flow").join("dynamic-workflows"),
        )),
        None => Arc::new(InMemoryEventStore::new()),
    }
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
mod tests {
    use super::*;
    use crate::tools::{ToolExecutor, ToolOutput};

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
    }

    #[tokio::test]
    async fn dynamic_workflow_step_can_call_host_parallel_task_without_ptc_parallel_task() {
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
}
