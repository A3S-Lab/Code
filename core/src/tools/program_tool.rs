//! Tool wrapper for programmatic tool calling.

use crate::program::ProgramCatalog;
use crate::text::truncate_utf8;
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use crate::tools::{registry_bound_tool_invoker, registry_tool_invoker, ToolInvoker, ToolRegistry};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use rquickjs::function::{Async, Func};
use rquickjs::{async_with, AsyncContext, AsyncRuntime, CatchResultExt, Error as JsError, Promise};
use serde::Deserialize;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

const DEFAULT_SCRIPT_TIMEOUT_MS: u64 = 30_000;
/// Scripts allowed to delegate (`task`) run child agents that each take a full
/// LLM turn, so they need a far more generous default timeout.
const DELEGATION_SCRIPT_TIMEOUT_MS: u64 = 600_000;
const DEFAULT_SCRIPT_MAX_TOOL_CALLS: usize = 20;
const DEFAULT_SCRIPT_MAX_OUTPUT_BYTES: usize = 64 * 1024;
const PROGRAM_CANCELLATION_SETTLE_GRACE: Duration = Duration::from_millis(500);
// Engineered workflows include planner, maker, checker, deterministic evidence
// gates, recovery, and event projection logic in one auditable script. Keep a
// firm bound while leaving enough room for explicit contracts and diagnostics.
pub const MAX_PROGRAM_SCRIPT_SOURCE_BYTES: usize = 192 * 1024;

pub struct ProgramTool {
    fallback_invoker: Arc<dyn ToolInvoker>,
}

impl ProgramTool {
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            fallback_invoker: registry_tool_invoker(registry),
        }
    }

    pub fn with_catalog(registry: Arc<ToolRegistry>, _catalog: ProgramCatalog) -> Self {
        Self::new(registry)
    }

    pub(crate) fn with_catalog_registry_bound(
        registry: Arc<ToolRegistry>,
        _catalog: ProgramCatalog,
    ) -> Self {
        Self {
            fallback_invoker: registry_bound_tool_invoker(registry),
        }
    }
}

#[async_trait]
impl Tool for ProgramTool {
    fn name(&self) -> &str {
        "program"
    }

    fn description(&self) -> &str {
        "Run a sandboxed JavaScript PTC script. The script defines async function run(ctx, inputs) and may call only allowed ctx tools."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "type": {
                    "type": "string",
                    "description": "Required. Program kind. Only \"script\" is supported.",
                    "enum": ["script"]
                },
                "inputs": {
                    "type": "object",
                    "description": "Optional. JSON inputs passed to the script as the second argument."
                },
                "language": {
                    "type": "string",
                    "description": "Script language. Only JavaScript is supported.",
                    "enum": ["javascript"]
                },
                "source": {
                    "type": "string",
                    "description": "Inline JavaScript source defining async function run(ctx, inputs)."
                },
                "path": {
                    "type": "string",
                    "description": "Workspace-relative path to a .js or .mjs script defining async function run(ctx, inputs). Used when source is omitted."
                },
                "allowed_tools": {
                    "type": "array",
                    "description": "Tool names the script may call through ctx. Defaults to all registered tools except program, dynamic_workflow, and the legacy parallel_task alias.",
                    "items": { "type": "string" }
                },
                "limits": {
                    "type": "object",
                    "description": "Optional timeoutMs, maxToolCalls, and maxOutputBytes.",
                    "additionalProperties": false,
                    "properties": {
                        "timeoutMs": { "type": "integer", "minimum": 1 },
                        "maxToolCalls": { "type": "integer", "minimum": 1 },
                        "maxOutputBytes": { "type": "integer", "minimum": 1 }
                    }
                }
            },
            "required": ["type"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let Some(kind) = args.get("type").and_then(|value| value.as_str()) else {
            return Ok(ToolOutput::error("type parameter is required"));
        };
        if kind != "script" {
            return Ok(ToolOutput::error(format!(
                "Unsupported program type: {kind}. Only \"script\" is supported."
            )));
        }
        let inputs = args
            .get("inputs")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let invoker = ctx
            .tool_invoker()
            .unwrap_or_else(|| Arc::clone(&self.fallback_invoker));
        execute_script_program(args, inputs, invoker, ctx).await
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScriptLimits {
    timeout_ms: Option<u64>,
    max_tool_calls: Option<usize>,
    max_output_bytes: Option<usize>,
}

#[derive(Debug, Clone)]
struct ScriptCallRecord {
    tool_name: String,
    success: bool,
    exit_code: i32,
    output_bytes: usize,
    metadata: Option<serde_json::Value>,
}

async fn execute_script_program(
    args: &serde_json::Value,
    inputs: serde_json::Value,
    invoker: Arc<dyn ToolInvoker>,
    ctx: &ToolContext,
) -> Result<ToolOutput> {
    let language = args
        .get("language")
        .and_then(|value| value.as_str())
        .unwrap_or("javascript");
    if language != "javascript" {
        return Ok(ToolOutput::error(format!(
            "Unsupported script language: {language}"
        )));
    }

    let source = match load_script_source(args, ctx).await {
        Ok(source) => source,
        Err(message) => return Ok(ToolOutput::error(message)),
    };
    if source.len() > MAX_PROGRAM_SCRIPT_SOURCE_BYTES {
        return Ok(ToolOutput::error(format!(
            "script source is too large: {} bytes exceeds {} bytes",
            source.len(),
            MAX_PROGRAM_SCRIPT_SOURCE_BYTES
        )));
    }
    if let Err(message) = validate_script_source(&source) {
        return Ok(ToolOutput::error(message));
    }

    let allowed_tools = script_allowed_tools(args, invoker.available_tools());
    let limits = script_limits(args);
    match run_quickjs_script(&source, inputs, invoker, ctx.clone(), allowed_tools, limits).await {
        Ok(output) => Ok(output),
        Err(err) => Ok(ToolOutput::error(format!("program script failed: {err}"))),
    }
}

async fn load_script_source(
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> std::result::Result<String, String> {
    if let Some(source) = args.get("source").and_then(|value| value.as_str()) {
        return Ok(source.to_string());
    }

    let Some(path) = args.get("path").and_then(|value| value.as_str()) else {
        return Err("program script requires either source or path".to_string());
    };
    if !(path.ends_with(".js") || path.ends_with(".mjs")) {
        return Err("program script path must point to a .js or .mjs file".to_string());
    }

    let workspace_path = ctx
        .resolve_workspace_path(path)
        .map_err(|err| format!("failed to resolve script path: {err}"))?;
    ctx.workspace_services
        .fs()
        .read_text(&workspace_path)
        .await
        .map_err(|err| format!("failed to read script path '{}': {err}", path))
}

fn script_allowed_tools(args: &serde_json::Value, available_tools: Vec<String>) -> HashSet<String> {
    let mut allowed = args
        .get("allowed_tools")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(ToString::to_string)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_else(|| available_tools.into_iter().collect());

    allowed.remove("program");
    // QuickJS is a single-threaded embedded VM, so PTC scripts must not expose
    // recursive orchestrators or the legacy fan-out alias directly. Unified
    // `task` calls are checked per invocation below; Dynamic Workflows can
    // schedule host-side task fan-out.
    allowed.remove("dynamic_workflow");
    allowed.remove("parallel_task");
    allowed
}

fn script_limits(args: &serde_json::Value) -> ScriptLimits {
    args.get("limits")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or(ScriptLimits {
            timeout_ms: None,
            max_tool_calls: None,
            max_output_bytes: None,
        })
}

fn validate_script_source(source: &str) -> std::result::Result<(), String> {
    let forbidden = [
        ("import ", "imports are not allowed inside PTC scripts"),
        (
            "import(",
            "dynamic imports are not allowed inside PTC scripts",
        ),
        ("eval(", "eval is not allowed inside PTC scripts"),
        (
            "Function(",
            "Function constructor is not allowed inside PTC scripts",
        ),
        ("Worker(", "Worker is not allowed inside PTC scripts"),
        ("WebSocket", "WebSocket is not allowed inside PTC scripts"),
        (
            "fetch(",
            "fetch is not allowed inside PTC scripts; use ctx tools instead",
        ),
    ];

    for (needle, message) in forbidden {
        if source.contains(needle) {
            return Err(message.to_string());
        }
    }
    Ok(())
}

async fn run_quickjs_script(
    source: &str,
    inputs: serde_json::Value,
    invoker: Arc<dyn ToolInvoker>,
    ctx: ToolContext,
    allowed_tools: HashSet<String>,
    limits: ScriptLimits,
) -> Result<ToolOutput> {
    // A script that can delegate runs child agents (each a full LLM turn, often
    // 30s to several minutes), so the 30s default is far too short and silently
    // times out real workflows. Default delegation-capable scripts to a generous
    // timeout; pure compute/search scripts keep the short default. An explicit
    // limits.timeoutMs always wins.
    let delegating = allowed_tools.contains("task");
    let timeout_ms = limits.timeout_ms.unwrap_or(if delegating {
        DELEGATION_SCRIPT_TIMEOUT_MS
    } else {
        DEFAULT_SCRIPT_TIMEOUT_MS
    });
    let max_tool_calls = limits
        .max_tool_calls
        .unwrap_or(DEFAULT_SCRIPT_MAX_TOOL_CALLS);
    let max_output_bytes = limits
        .max_output_bytes
        .unwrap_or(DEFAULT_SCRIPT_MAX_OUTPUT_BYTES);
    let executable_source = script_source_with_host_entrypoint(source)?;
    let parent_cancellation = ctx.cancellation_token();
    let program_cancellation = parent_cancellation.child_token();
    // Captured on the outer multi-threaded runtime (we're async here, before the
    // VM's nested single-thread runtime is built) so host tools run on the
    // session runtime instead of being trapped inside the QuickJS VM runtime.
    let outer = tokio::runtime::Handle::current();
    let state = Arc::new(Mutex::new(ScriptVmState {
        invoker,
        ctx: ctx.with_cancellation(program_cancellation.clone()),
        allowed_tools,
        max_tool_calls,
        max_output_bytes,
        tool_calls: 0,
        records: Vec::new(),
        outer,
    }));

    let vm_state = Arc::clone(&state);
    let mut vm = tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| anyhow!("failed to create program VM runtime: {err}"))?;
        runtime.block_on(run_embedded_script(
            executable_source,
            inputs,
            vm_state,
            timeout_ms,
            program_cancellation,
        ))
    });

    enum Stop {
        Cancelled,
        TimedOut,
    }
    let result = tokio::select! {
        biased;
        _ = parent_cancellation.cancelled() => None,
        result = &mut vm => Some(result),
        _ = tokio::time::sleep(Duration::from_millis(timeout_ms)) => None,
    };
    let stop = if result.is_none() {
        if parent_cancellation.is_cancelled() {
            Some(Stop::Cancelled)
        } else {
            Some(Stop::TimedOut)
        }
    } else {
        None
    };

    if let Some(stop) = stop {
        // The child token is already cancelled when the parent stopped. On an
        // internal deadline, cancel it explicitly so every nested invocation
        // receives the same terminal signal as the VM.
        state.lock().await.ctx.cancellation_token().cancel();
        if timeout(PROGRAM_CANCELLATION_SETTLE_GRACE, &mut vm)
            .await
            .is_err()
        {
            vm.abort();
            let _ = vm.await;
        }
        return Ok(ToolOutput::error(match stop {
            Stop::Cancelled => "program script cancelled by caller".to_string(),
            Stop::TimedOut => format!("program script timed out after {timeout_ms} ms"),
        }));
    }

    let result = result.expect("completed VM result is present");

    match result {
        Ok(Ok(result)) => {
            let records = state.lock().await.records.clone();
            let output = render_script_output(&result, &records, "");
            Ok(ToolOutput::success(output).with_metadata(serde_json::json!({
                "program": {
                    "name": "script",
                    "language": "javascript",
                    "runtime": "embedded-quickjs",
                    "success": true,
                    "tool_calls": records.iter().map(script_record_to_value).collect::<Vec<_>>(),
                },
                "script_result": result,
            })))
        }
        Ok(Err(err)) if is_quickjs_timeout(&err) => Ok(ToolOutput::error(format!(
            "program script timed out after {timeout_ms} ms"
        ))),
        Ok(Err(err)) => Ok(ToolOutput::error(format!("program script error:\n{err}"))),
        Err(err) => Ok(ToolOutput::error(format!(
            "program VM thread failed: {err}"
        ))),
    }
}

fn script_source_with_host_entrypoint(source: &str) -> Result<String> {
    let rewritten = if source.contains("export default async function run") {
        source.replacen("export default async function run", "async function run", 1)
    } else if source.contains("export default function run") {
        source.replacen("export default function run", "function run", 1)
    } else if source.contains("async function run") || source.contains("function run") {
        source.to_string()
    } else {
        return Err(anyhow!(
            "PTC script must define async function run(ctx, inputs)"
        ));
    };

    Ok(format!(
        r#"{rewritten}

globalThis.__a3sResultJson = (async () => JSON.stringify(await run(globalThis.__a3sCtx, globalThis.__a3sInputs)))();
"#
    ))
}

async fn run_embedded_script(
    source: String,
    inputs: serde_json::Value,
    state: Arc<Mutex<ScriptVmState>>,
    timeout_ms: u64,
    cancellation: tokio_util::sync::CancellationToken,
) -> Result<serde_json::Value> {
    let runtime = AsyncRuntime::new()?;
    let started = Instant::now();
    runtime
        .set_interrupt_handler(Some(Box::new(move || {
            cancellation.is_cancelled() || started.elapsed() >= Duration::from_millis(timeout_ms)
        })))
        .await;
    runtime.set_memory_limit(64 * 1024 * 1024).await;
    runtime.set_max_stack_size(512 * 1024).await;

    let context = AsyncContext::full(&runtime).await?;
    let inputs_json = serde_json::to_string(&inputs)?;
    let script = format!("{}\n{}", embedded_script_bootstrap(&inputs_json), source);
    let result_json = async_with!(context => |ctx| {
        let state = Arc::clone(&state);
        let host_tool = move |tool: String, args_json: String| {
            let state = Arc::clone(&state);
            async move { execute_host_tool_json(state, tool, args_json).await }
        };
        if let Err(err) = ctx.globals().set("__a3sHostTool", Func::from(Async(host_tool))) {
            return Err(format!("failed to install program host tool: {err}"));
        }
        let promise: Promise = match ctx.eval(script) {
            Ok(promise) => promise,
            Err(err) => return Err(format!("failed to evaluate program script: {err}")),
        };
        promise
            .into_future::<String>()
            .await
            .catch(&ctx)
            .map_err(|err| err.to_string())
    })
    .await
    .map_err(anyhow::Error::msg)?;

    serde_json::from_str(&result_json)
        .map_err(|err| anyhow!("program script returned invalid JSON: {err}"))
}

struct ScriptVmState {
    invoker: Arc<dyn ToolInvoker>,
    ctx: ToolContext,
    allowed_tools: HashSet<String>,
    max_tool_calls: usize,
    max_output_bytes: usize,
    tool_calls: usize,
    records: Vec<ScriptCallRecord>,
    /// Handle to the OUTER multi-threaded session runtime. The script VM runs on
    /// a nested single-thread runtime; host tool calls are dispatched here so
    /// delegated `task` runs are not trapped inside the VM runtime.
    outer: tokio::runtime::Handle,
}

fn embedded_script_bootstrap(inputs_json: &str) -> String {
    format!(
        r#"
const __a3sCallTool = async (tool, args = {{}}) => {{
  const response = await globalThis.__a3sHostTool(String(tool), JSON.stringify(args ?? {{}}));
  return JSON.parse(response);
}};

const __a3sTools = Object.freeze(new Proxy({{}}, {{
  get(_target, prop) {{
    if (typeof prop !== "string" || prop === "then") return undefined;
    return (args = {{}}) => __a3sCallTool(prop, args);
  }},
  has(_target, prop) {{
    return typeof prop === "string";
  }},
}}));

const __a3sReadArgs = (path, options = {{}}) => ({{ ...(options ?? {{}}), file_path: path }});
const __a3sLegacySearchArgs = (mode, query, options = {{}}) => {{
  const args = {{ ...(options ?? {{}}), mode, query }};
  if (Object.prototype.hasOwnProperty.call(args, "glob")) {{
    args.include = args.glob;
    delete args.glob;
  }}
  if (Object.prototype.hasOwnProperty.call(args, "-i")) {{
    args.case_sensitive = !args["-i"];
    delete args["-i"];
  }}
  return args;
}};
const __a3sCtx = Object.freeze({{
  tool: __a3sCallTool,
  tools: __a3sTools,
  readFile: (path, options = {{}}) => __a3sCallTool("read", __a3sReadArgs(path, options)).then((r) => r.text ?? r.output),
  read: (path, options = {{}}) => __a3sCallTool("read", __a3sReadArgs(path, options)),
  search: (query, options = {{}}) => __a3sCallTool("search", {{ ...options, query }}).then((r) => r.output),
  grep: (query, options = {{}}) => __a3sCallTool("search", __a3sLegacySearchArgs("grep", query, options)).then((r) => r.output),
  bm25: (query, options = {{}}) => __a3sCallTool("search", __a3sLegacySearchArgs("bm25", query, options)).then((r) => r.output),
  glob: (query, options = {{}}) => __a3sCallTool("search", {{ ...options, mode: "glob", query }}).then((r) => r.output),
  ls: (path = ".") => __a3sCallTool("ls", {{ path }}).then((r) => r.output),
  bash: (command) => __a3sCallTool("bash", {{ command }}).then((r) => r.output),
  git: (args = {{}}) => __a3sCallTool("git", args),
  webSearch: (params) => __a3sCallTool("web_search", params),
  verify: (args) => __a3sCallTool("bash", args),
}});

Object.defineProperty(globalThis, "__a3sCtx", {{ value: __a3sCtx, configurable: false }});
Object.defineProperty(globalThis, "__a3sInputs", {{ value: {inputs_json}, configurable: false }});
Object.defineProperty(globalThis, "fetch", {{ value: undefined, configurable: false, writable: false }});
Object.defineProperty(globalThis, "WebSocket", {{ value: undefined, configurable: false, writable: false }});
Object.defineProperty(globalThis, "Worker", {{ value: undefined, configurable: false, writable: false }});
"#
    )
}

async fn execute_host_tool_json(
    state: Arc<Mutex<ScriptVmState>>,
    tool: String,
    args_json: String,
) -> rquickjs::Result<String> {
    let args = serde_json::from_str(&args_json).map_err(|err| {
        JsError::new_from_js_message("string", "object", format!("invalid tool args JSON: {err}"))
    })?;
    let (invoker, ctx, max_output_bytes, outer) = {
        let mut script = state.lock().await;
        if !script_tool_is_allowed(&script.allowed_tools, &tool, &args) {
            return Err(JsError::new_from_js_message(
                "tool",
                "allowed tool",
                format!("tool '{tool}' is not allowed for this PTC script"),
            ));
        }
        script.tool_calls += 1;
        if script.tool_calls > script.max_tool_calls {
            return Err(JsError::new_from_js_message(
                "tool call",
                "limited tool call",
                format!("PTC script exceeded maxToolCalls={}", script.max_tool_calls),
            ));
        }
        (
            Arc::clone(&script.invoker),
            script.ctx.clone(),
            script.max_output_bytes,
            script.outer.clone(),
        )
    };

    // Run the tool on the OUTER multi-threaded runtime (not this nested
    // single-thread VM runtime) so host tools can use the session runtime
    // normally. `parallel_task` is intentionally filtered before this point.
    let tool_for_spawn = tool.clone();
    let result = outer
        .spawn(async move {
            invoker
                .invoke(ctx.nested_tool_invocation(tool_for_spawn, args), &ctx)
                .await
        })
        .await
        .map_err(|err| JsError::new_from_js_message("tool", "spawn", err.to_string()))?;
    let mut output = result.output;
    if output.len() > max_output_bytes {
        output = truncate_utf8(&output, max_output_bytes).to_string();
    }
    let success = result.exit_code == 0;
    let metadata = result.metadata.clone();
    let exit_code = result.exit_code;
    let name = result.name;
    let text = (tool == "read" && success)
        .then(|| program_read_text(&output, metadata.as_ref()))
        .flatten();

    {
        let mut script = state.lock().await;
        script.records.push(ScriptCallRecord {
            tool_name: tool,
            success,
            exit_code,
            output_bytes: output.len(),
            metadata: metadata.clone(),
        });
    }

    serde_json::to_string(&serde_json::json!({
        "name": name,
        "output": output,
        "text": text,
        "exitCode": exit_code,
        "metadata": metadata,
    }))
    .map_err(|err| JsError::new_from_js_message("tool result", "json", err.to_string()))
}

/// Recover the line payload from the single-file `read` tool's anchored
/// display format. `ctx.readFile` is a programmatic convenience API, while
/// `ctx.read` deliberately retains the model-facing line numbers and
/// continuation footer. The metadata count makes the transform structural:
/// content that happens to resemble a continuation footer is never removed.
fn program_read_text(output: &str, metadata: Option<&serde_json::Value>) -> Option<String> {
    let returned_lines = metadata?
        .pointer("/range/returned_lines")?
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())?;
    if returned_lines == 0 {
        return Some(String::new());
    }

    let mut text = String::new();
    let mut rendered_lines = output.split_inclusive('\n');
    for _ in 0..returned_lines {
        let rendered = rendered_lines.next()?;
        let (anchor, content) = rendered.split_once('\t')?;
        if anchor.len() != 6 || anchor.trim().parse::<usize>().is_err() {
            return None;
        }
        text.push_str(content);
    }
    Some(text)
}

fn script_tool_is_allowed(
    allowed_tools: &HashSet<String>,
    tool: &str,
    args: &serde_json::Value,
) -> bool {
    if tool == "task"
        && args
            .get("tasks")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|tasks| tasks.len() > 1)
    {
        return false;
    }
    if allowed_tools.contains(tool) {
        return true;
    }
    tool == "search"
        && args
            .get("mode")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|mode| allowed_tools.contains(mode))
}

fn is_quickjs_timeout(err: &anyhow::Error) -> bool {
    let text = err.to_string();
    text.contains("interrupted") || text.contains("InternalError")
}

fn script_record_to_value(record: &ScriptCallRecord) -> serde_json::Value {
    serde_json::json!({
        "tool_name": record.tool_name,
        "success": record.success,
        "exit_code": record.exit_code,
        "output_bytes": record.output_bytes,
        "metadata": record.metadata,
    })
}

fn render_script_output(
    result: &serde_json::Value,
    records: &[ScriptCallRecord],
    stderr: &str,
) -> String {
    let mut output = String::from("Program script completed.");
    if let Some(summary) = result.get("summary").and_then(|value| value.as_str()) {
        output.push('\n');
        output.push_str(summary);
    }

    output.push_str(&format!("\n\nTool calls: {}", records.len()));
    for (index, record) in records.iter().enumerate() {
        output.push_str(&format!(
            "\n{}. {} ({}, exit_code={}, output_bytes={})",
            index + 1,
            record.tool_name,
            if record.success { "ok" } else { "failed" },
            record.exit_code,
            record.output_bytes
        ));
    }

    output.push_str("\n\nResult:\n");
    output.push_str(&serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string()));

    if !stderr.is_empty() {
        output.push_str("\n\nstderr:\n");
        output.push_str(stderr);
    }

    output
}

#[cfg(test)]
#[path = "program_tool/tests.rs"]
mod tests;
