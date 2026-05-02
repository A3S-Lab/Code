use a3s_code_core::tools::ToolExecutor;
use serde_json::json;

#[tokio::test]
async fn program_script_runs_embedded_quickjs_vm_with_workspace_js_path_and_ctx_tools() {
    let dir = tempfile::tempdir().expect("tempdir");
    let src_dir = dir.path().join("src");
    std::fs::create_dir_all(&src_dir).expect("create src");
    std::fs::write(
        src_dir.join("auth.rs"),
        r#"
pub struct PermissionPolicy {
    pub default_decision: String,
}
"#,
    )
    .expect("write auth source");
    std::fs::create_dir_all(dir.path().join("scripts/ptc")).expect("create scripts");
    std::fs::write(
        dir.path().join("scripts/ptc/search-auth.js"),
        r#"
export default async function run(ctx, inputs) {
  const hits = await ctx.grep(inputs.query, { path: "src", glob: "*.rs" });
  const filesText = await ctx.glob("src/**/*.rs");
  const files = filesText.split("\n").map((line) => line.trim()).filter(Boolean);
  const evidence = [];

  for (const file of files) {
    const content = await ctx.readFile(file);
    if (content.includes(inputs.query)) {
      evidence.push({
        file,
        hasDefaultDecision: content.includes("default_decision"),
      });
    }
  }

  return {
    summary: `found ${evidence.length} matching file(s)`,
    hits,
    evidence,
  };
}
"#,
    )
    .expect("write ptc script");

    let executor = ToolExecutor::new(dir.path().to_string_lossy().to_string());
    let result = executor
        .execute(
            "program",
            &json!({
                "type": "script",
                "path": "scripts/ptc/search-auth.js",
                "language": "javascript",
                "inputs": { "query": "PermissionPolicy" },
                "allowed_tools": ["grep", "glob", "read"],
                "limits": {
                    "timeoutMs": 30000,
                    "maxToolCalls": 10,
                    "maxOutputBytes": 65536
                }
            }),
        )
        .await
        .expect("program tool execution");

    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("Program script completed."));
    assert!(result.output.contains("found 1 matching file(s)"));
    assert!(result.output.contains("src/auth.rs"));
    assert!(result.output.contains("hasDefaultDecision"));

    let metadata = result.metadata.expect("program metadata");
    assert_eq!(metadata["program"]["name"], "script");
    assert_eq!(metadata["program"]["language"], "javascript");
    assert_eq!(metadata["program"]["runtime"], "embedded-quickjs");
    let tool_calls = metadata["program"]["tool_calls"].as_array().unwrap();
    assert!(tool_calls.len() >= 3);
    let tool_names = tool_calls
        .iter()
        .filter_map(|call| call["tool_name"].as_str())
        .collect::<Vec<_>>();
    assert!(tool_names.contains(&"grep"));
    assert!(tool_names.contains(&"glob"));
    assert!(tool_names.contains(&"read"));
    assert_eq!(
        metadata["script_result"]["evidence"][0]["hasDefaultDecision"],
        true
    );
}
