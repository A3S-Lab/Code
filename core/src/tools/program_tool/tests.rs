use super::*;
use async_trait::async_trait;
use std::path::PathBuf;

struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }

    fn description(&self) -> &str {
        "Echo test tool"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            }
        })
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let message = args
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        Ok(ToolOutput::success(format!("echo:{message}")))
    }
}

struct ParallelTaskProbeTool;

struct TaskProbeTool;

#[async_trait]
impl Tool for ParallelTaskProbeTool {
    fn name(&self) -> &str {
        "parallel_task"
    }

    fn description(&self) -> &str {
        "Probe tool used to verify PTC sandbox filtering."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::success("should-not-run"))
    }
}

#[async_trait]
impl Tool for TaskProbeTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Probe tool used to verify unified task fan-out filtering."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }

    async fn execute(&self, _args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        Ok(ToolOutput::success("should-not-run"))
    }
}

struct CancellationAwareTool {
    started: Arc<tokio::sync::Notify>,
    cancellations: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl Tool for CancellationAwareTool {
    fn name(&self) -> &str {
        "cancellation_aware"
    }

    fn description(&self) -> &str {
        "waits for invocation cancellation"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }

    async fn execute(&self, _args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        self.started.notify_one();
        ctx.cancellation_token().cancelled().await;
        self.cancellations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ToolOutput::success("nested call settled"))
    }
}

#[tokio::test]
async fn program_tool_rejects_non_script_type() {
    let tool = ProgramTool::new(Arc::new(ToolRegistry::new(PathBuf::from("/tmp"))));
    let output = tool
        .execute(
            &serde_json::json!({ "type": "program_code_search" }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();

    assert!(!output.success);
    assert!(output.content.contains("Only \"script\" is supported"));
}

#[tokio::test]
async fn program_tool_rejects_missing_script_source_and_path() {
    let tool = ProgramTool::new(Arc::new(ToolRegistry::new(PathBuf::from("/tmp"))));
    let output = tool
        .execute(
            &serde_json::json!({ "type": "script" }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();

    assert!(!output.success);
    assert!(output.content.contains("requires either source or path"));
}

#[tokio::test]
async fn program_tool_rejects_unsupported_language() {
    let tool = ProgramTool::new(Arc::new(ToolRegistry::new(PathBuf::from("/tmp"))));
    let output = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "language": "typescript",
                "source": "async function run() { return {}; }"
            }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();

    assert!(!output.success);
    assert!(output.content.contains("Unsupported script language"));
}

#[tokio::test]
async fn program_tool_rejects_source_above_engineered_workflow_limit() {
    let tool = ProgramTool::new(Arc::new(ToolRegistry::new(PathBuf::from("/tmp"))));
    let source = "x".repeat(MAX_PROGRAM_SCRIPT_SOURCE_BYTES + 1);
    let output = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "source": source
            }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();

    assert!(!output.success);
    assert!(output.content.contains("script source is too large"));
    assert!(output.content.contains(&format!(
        "exceeds {} bytes",
        MAX_PROGRAM_SCRIPT_SOURCE_BYTES
    )));
}

#[tokio::test]
async fn program_tool_accepts_auditable_workflow_above_legacy_limit() {
    let tool = ProgramTool::new(Arc::new(ToolRegistry::new(PathBuf::from("/tmp"))));
    let source = format!(
        "async function run() {{ return {{ accepted: true }}; }}{}",
        " ".repeat(140 * 1024)
    );
    let output = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "source": source
            }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();

    assert!(output.success, "{}", output.content);
    assert!(output.content.contains("accepted"), "{}", output.content);
}

#[tokio::test]
async fn program_tool_rejects_unsupported_script_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("script.txt"), "async function run() {}").unwrap();
    let tool = ProgramTool::new(Arc::new(ToolRegistry::new(dir.path().to_path_buf())));
    let output = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "path": "script.txt"
            }),
            &ToolContext::new(dir.path().to_path_buf()),
        )
        .await
        .unwrap();

    assert!(!output.success);
    assert!(output.content.contains(".js or .mjs file"));
}

#[test]
fn program_tool_default_allowed_tools_include_registry_tools_except_program() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    registry.register(Arc::new(EchoTool));
    registry.register_builtin(Arc::new(ProgramTool::new(Arc::new(ToolRegistry::new(
        PathBuf::from("/tmp"),
    )))));

    let allowed = script_allowed_tools(&serde_json::json!({}), registry.list());

    assert!(allowed.contains("echo"));
    assert!(!allowed.contains("program"));
}

#[test]
fn program_tool_forbids_recursive_or_legacy_orchestrators_in_scripts() {
    let registry = ToolRegistry::new(PathBuf::from("/tmp"));
    let args = serde_json::json!({
        "allowed_tools": [
            "parallel_task",
            "dynamic_workflow",
            "task",
            "program",
            "echo"
        ]
    });
    let allowed = script_allowed_tools(&args, registry.list());
    assert!(!allowed.contains("parallel_task"));
    assert!(!allowed.contains("dynamic_workflow"));
    assert!(allowed.contains("task"));
    assert!(allowed.contains("echo"));
    assert!(!allowed.contains("program"));
}

#[tokio::test]
async fn program_tool_rejects_parallel_task_even_when_explicitly_allowed() {
    let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
    registry.register(Arc::new(ParallelTaskProbeTool));
    let tool = ProgramTool::new(Arc::clone(&registry));

    let output = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "source": r#"
                        async function run(ctx) {
                            return await ctx.tool("parallel_task", { tasks: [] });
                        }
                    "#,
                "allowed_tools": ["parallel_task"]
            }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();

    assert!(!output.success);
    assert!(output
        .content
        .contains("tool 'parallel_task' is not allowed"));
    assert!(!output.content.contains("should-not-run"));
}

#[tokio::test]
async fn program_tool_rejects_unified_task_fanout_but_keeps_single_delegation() {
    let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
    registry.register(Arc::new(TaskProbeTool));
    let tool = ProgramTool::new(Arc::clone(&registry));

    let rejected = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "source": r#"
                    async function run(ctx) {
                        return await ctx.tool("task", { tasks: [{}, {}] });
                    }
                "#,
                "allowed_tools": ["task"]
            }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();
    assert!(!rejected.success);
    assert!(rejected.content.contains("tool 'task' is not allowed"));
    assert!(!rejected.content.contains("should-not-run"));

    let allowed = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "source": r#"
                    async function run(ctx) {
                        return await ctx.tool("task", { tasks: [{}] });
                    }
                "#,
                "allowed_tools": ["task"]
            }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();
    assert!(allowed.success, "{}", allowed.content);
    assert!(allowed.content.contains("should-not-run"));
}

#[tokio::test]
async fn program_tool_source_uses_default_all_registered_tools() {
    let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
    registry.register(Arc::new(EchoTool));
    let tool = ProgramTool::new(Arc::clone(&registry));
    let output = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "source": r#"
                        async function run(ctx, inputs) {
                            const result = await ctx.tool("echo", { message: inputs.message });
                            return { summary: result.output, result };
                        }
                    "#,
                "inputs": { "message": "hello" }
            }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();

    assert!(output.success, "{}", output.content);
    assert!(output.content.contains("echo:hello"));
    let metadata = output.metadata.unwrap();
    assert_eq!(metadata["program"]["runtime"], "embedded-quickjs");
    assert_eq!(metadata["script_result"]["summary"], "echo:hello");
}

#[tokio::test]
async fn program_tool_ctx_read_file_passes_offset_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "one\ntwo\nthree\n").unwrap();
    let executor = crate::tools::ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let tool = ProgramTool::new(Arc::clone(executor.registry()));

    let output = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "source": r#"
                        async function run(ctx) {
                            const text = await ctx.readFile("notes.txt", { offset: 1, limit: 1 });
                            const result = await ctx.read("notes.txt", { offset: 2, limit: 1 });
                            return { text, raw: result.output };
                        }
                    "#,
                "allowed_tools": ["read"]
            }),
            &ToolContext::new(dir.path().to_path_buf()),
        )
        .await
        .unwrap();

    assert!(output.success, "{}", output.content);
    let metadata = output.metadata.unwrap();
    let text = metadata["script_result"]["text"].as_str().unwrap();
    assert_eq!(text, "two\n");
    let raw = metadata["script_result"]["raw"].as_str().unwrap();
    assert!(raw.contains("three"));
    assert!(!raw.contains("two"));
}

#[test]
fn program_read_text_uses_range_metadata_not_content_heuristics() {
    let output = "     7\talpha\n     8\t... (more lines available; continue with offset=9)\n\n... (more lines available; continue with offset=9)\n";
    let metadata = serde_json::json!({"range": {"returned_lines": 2}});

    assert_eq!(
        program_read_text(output, Some(&metadata)).as_deref(),
        Some("alpha\n... (more lines available; continue with offset=9)\n")
    );
}

#[tokio::test]
async fn program_tool_exposes_ctx_tools_proxy_for_named_tools() {
    let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
    registry.register(Arc::new(EchoTool));
    let tool = ProgramTool::new(Arc::clone(&registry));
    let output = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "source": r#"
                        async function run(ctx, inputs) {
                            const result = await ctx.tools.echo({ message: inputs.message });
                            return { summary: result.output, result };
                        }
                    "#,
                "inputs": { "message": "proxy" }
            }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();

    assert!(output.success, "{}", output.content);
    let metadata = output.metadata.unwrap();
    assert_eq!(metadata["script_result"]["summary"], "echo:proxy");
    assert_eq!(metadata["program"]["tool_calls"][0]["tool_name"], "echo");
}

#[tokio::test]
async fn program_tool_ctx_tools_proxy_respects_allowed_tools() {
    let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
    registry.register(Arc::new(EchoTool));
    let tool = ProgramTool::new(Arc::clone(&registry));
    let output = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "source": r#"
                        async function run(ctx) {
                            await ctx.tools.echo({ message: "blocked" });
                            return {};
                        }
                    "#,
                "allowed_tools": ["read"]
            }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();

    assert!(!output.success);
    assert!(output.content.contains("tool 'echo' is not allowed"));
}

#[tokio::test]
async fn program_tool_explicit_allowed_tools_restrict_default_tools() {
    let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
    registry.register(Arc::new(EchoTool));
    let tool = ProgramTool::new(Arc::clone(&registry));
    let output = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "source": r#"
                        async function run(ctx) {
                            await ctx.tool("echo", { message: "blocked" });
                            return {};
                        }
                    "#,
                "allowed_tools": ["read"]
            }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();

    assert!(!output.success);
    assert!(output.content.contains("tool 'echo' is not allowed"));
}

#[tokio::test]
async fn program_tool_enforces_max_tool_calls() {
    let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
    registry.register(Arc::new(EchoTool));
    let tool = ProgramTool::new(Arc::clone(&registry));
    let output = tool
        .execute(
            &serde_json::json!({
                "type": "script",
                "source": r#"
                        async function run(ctx) {
                            await ctx.tool("echo", { message: "one" });
                            await ctx.tool("echo", { message: "two" });
                            return {};
                        }
                    "#,
                "limits": { "maxToolCalls": 1 }
            }),
            &ToolContext::new(PathBuf::from("/tmp")),
        )
        .await
        .unwrap();

    assert!(!output.success);
    assert!(output.content.contains("exceeded maxToolCalls=1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn program_tool_cancellation_stops_vm_and_settles_nested_call() {
    let started = Arc::new(tokio::sync::Notify::new());
    let cancellations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let registry = Arc::new(ToolRegistry::new(PathBuf::from("/tmp")));
    registry.register(Arc::new(CancellationAwareTool {
        started: Arc::clone(&started),
        cancellations: Arc::clone(&cancellations),
    }));
    let tool = Arc::new(ProgramTool::new(Arc::clone(&registry)));
    let cancellation = tokio_util::sync::CancellationToken::new();
    let ctx = ToolContext::new(PathBuf::from("/tmp")).with_cancellation(cancellation.clone());
    let args = serde_json::json!({
        "type": "script",
        "source": r#"
                async function run(ctx) {
                    const result = await ctx.tool("cancellation_aware", {});
                    return { summary: result.output };
                }
            "#,
        "allowed_tools": ["cancellation_aware"],
        "limits": { "timeoutMs": 30_000 }
    });

    let run = tokio::spawn(async move { tool.execute(&args, &ctx).await.unwrap() });
    tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
        .await
        .expect("nested tool should start");
    cancellation.cancel();
    let output = tokio::time::timeout(std::time::Duration::from_secs(1), run)
        .await
        .expect("program cancellation should converge")
        .unwrap();

    assert!(!output.success, "{}", output.content);
    assert!(output.content.contains("cancelled"), "{}", output.content);
    assert_eq!(
        cancellations.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "nested tool must settle before the program returns"
    );
}

#[test]
fn program_tool_rejects_fetch_source_access() {
    let err = validate_script_source("export default async function run() { return fetch('/'); }")
        .unwrap_err();
    assert!(err.contains("fetch is not allowed"));
}

#[test]
fn program_tool_accepts_plain_function_run_entrypoint() {
    let source = script_source_with_host_entrypoint(
        "async function run(ctx, inputs) { return { summary: inputs.message }; }",
    )
    .unwrap();

    assert!(source.contains("globalThis.__a3sResultJson"));
    assert!(source.contains("async function run"));
}

#[test]
fn program_tool_renders_result_summary_and_tool_records() {
    let output = render_script_output(
        &serde_json::json!({ "summary": "done", "items": [1] }),
        &[ScriptCallRecord {
            tool_name: "echo".to_string(),
            success: true,
            exit_code: 0,
            output_bytes: 8,
            metadata: Some(serde_json::json!({ "kind": "test" })),
        }],
        "",
    );

    assert!(output.contains("Program script completed."));
    assert!(output.contains("done"));
    assert!(output.contains("echo (ok"));
    assert!(output.contains("\"items\""));
}
