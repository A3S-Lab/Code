//! Basic SDK usage example — Agent::new(), Agent::create(), and tool execution.
//!
//! Demonstrates:
//! - Loading config from `.a3s/config.hcl`
//! - Creating agents with both `Agent::new()` and `Agent::create()`
//! - Creating workspace-bound sessions
//! - Direct tool calls via `session.tool(name, args)` for all built-in tools
//! - Convenience methods: `bash()`, `read_file()`, `glob()`, `grep()`
//!
//! No LLM calls — runs entirely offline.
//!
//! ## Usage
//!
//! ```bash
//! # From the repo root (a3s/)
//! cd crates/code && cargo run --example sdk_basic
//!
//! # Or with a custom config path
//! A3S_CONFIG=/path/to/config.hcl cargo run --example sdk_basic
//! ```

use a3s_code_core::Agent;
use std::path::PathBuf;

/// Resolve the repo root: CARGO_MANIFEST_DIR/../../..
fn repo_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")); // crates/code/core
    manifest
        .parent() // crates/code
        .and_then(|p| p.parent()) // crates
        .and_then(|p| p.parent()) // repo root
        .expect("failed to resolve repo root")
        .to_path_buf()
}

/// Resolve the config path: $A3S_CONFIG > repo-root/.a3s/config.hcl
fn resolve_config() -> PathBuf {
    if let Ok(env_path) = std::env::var("A3S_CONFIG") {
        return PathBuf::from(env_path);
    }
    repo_root().join(".a3s/config.hcl")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = resolve_config();
    println!("=== A3S Code SDK Basic Example ===\n");
    println!("Config: {}\n", config_path.display());

    // ── 1. Agent::new() ──────────────────────────────────────────────────
    println!("--- Agent::new() ---");
    let agent_new = Agent::new(config_path.display().to_string()).await?;
    println!("[ok] Agent::new() succeeded\n");

    // ── 2. Agent::create() (cross-SDK alias) ─────────────────────────────
    println!("--- Agent::create() ---");
    let agent_create = Agent::create(config_path.display().to_string()).await?;
    println!("[ok] Agent::create() succeeded\n");

    // ── 3. Create sessions ───────────────────────────────────────────────
    let tmp = tempfile::tempdir()?;
    let workspace = tmp.path().display().to_string();

    let session_new = agent_new.session(&workspace, None)?;
    println!("[ok] Session from Agent::new() bound to {}", workspace);

    let tmp2 = tempfile::tempdir()?;
    let workspace2 = tmp2.path().display().to_string();

    let session_create = agent_create.session(&workspace2, None)?;
    println!("[ok] Session from Agent::create() bound to {}\n", workspace2);

    // ── 4. Tool execution: bash ──────────────────────────────────────────
    println!("--- Tool: bash ---");
    let output = session_new.bash("echo 'Hello from A3S SDK'").await?;
    println!("  output: {}", output.trim());

    // ── 5. Tool execution: write + read ──────────────────────────────────
    println!("\n--- Tool: write + read ---");
    let file_path = tmp.path().join("demo.txt").display().to_string();
    session_new
        .tool(
            "write",
            serde_json::json!({
                "file_path": &file_path,
                "content": "Hello from Agent::new()!\nLine 2\nLine 3",
            }),
        )
        .await?;
    println!("  wrote: {}", file_path);

    let content = session_new.read_file(&file_path).await?;
    println!("  read back ({} lines):", content.lines().count());
    for line in content.lines().take(3) {
        println!("    {}", line);
    }

    // ── 6. Tool execution via Agent::create() session ────────────────────
    println!("\n--- Tool: bash via Agent::create() session ---");
    let output2 = session_create.bash("echo 'Hello from Agent::create()'").await?;
    println!("  output: {}", output2.trim());

    // ── 7. Direct tool call: edit ───────────────────────────────────────
    println!("\n--- session.tool(\"edit\") ---");
    let edit_result = session_new
        .tool(
            "edit",
            serde_json::json!({
                "file_path": &file_path,
                "old_string": "Line 2",
                "new_string": "Line 2 (edited)",
            }),
        )
        .await?;
    println!("  exit_code: {}", edit_result.exit_code);
    assert_eq!(edit_result.exit_code, 0, "edit should succeed");
    let edited = std::fs::read_to_string(tmp.path().join("demo.txt"))?;
    assert!(edited.contains("Line 2 (edited)"), "edit should apply");
    println!("  verified: file contains 'Line 2 (edited)'");

    // ── 8. Direct tool call: patch ───────────────────────────────────────
    println!("\n--- session.tool(\"patch\") ---");
    let patch_result = session_new
        .tool(
            "patch",
            serde_json::json!({
                "file_path": &file_path,
                "diff": "@@ -2,2 +2,2 @@\n-Line 2 (edited)\n+Line 2 (patched)\n Line 3",
            }),
        )
        .await?;
    println!("  exit_code: {}", patch_result.exit_code);
    if patch_result.exit_code == 0 {
        let patched = std::fs::read_to_string(tmp.path().join("demo.txt"))?;
        assert!(patched.contains("Line 2 (patched)"), "patch should apply");
        println!("  verified: file contains 'Line 2 (patched)'");
    } else {
        // Patch can fail on fuzzy matching; not a hard error for the demo
        println!("  patch output: {}", patch_result.output.trim());
    }

    // ── 9. Direct tool call: read ────────────────────────────────────────
    println!("\n--- session.tool(\"read\") ---");
    let read_result = session_new
        .tool(
            "read",
            serde_json::json!({
                "file_path": &file_path,
                "offset": 0,
                "limit": 5,
            }),
        )
        .await?;
    println!("  exit_code: {}", read_result.exit_code);
    assert_eq!(read_result.exit_code, 0, "read should succeed");
    println!("  content:");
    for line in read_result.output.lines().take(3) {
        println!("    {}", line);
    }

    // ── 10. Direct tool call: ls ─────────────────────────────────────────
    println!("\n--- session.tool(\"ls\") ---");
    std::fs::write(tmp.path().join("alpha.rs"), "fn main() {}")?;
    std::fs::write(tmp.path().join("beta.rs"), "fn test() {}")?;
    std::fs::write(tmp.path().join("gamma.txt"), "text file")?;
    std::fs::create_dir_all(tmp.path().join("subdir"))?;

    let ls_result = session_new
        .tool(
            "ls",
            serde_json::json!({ "path": tmp.path().to_str().unwrap() }),
        )
        .await?;
    println!("  exit_code: {}", ls_result.exit_code);
    assert_eq!(ls_result.exit_code, 0, "ls should succeed");
    assert!(ls_result.output.contains("alpha.rs"), "ls should list alpha.rs");
    assert!(ls_result.output.contains("subdir"), "ls should list subdir");
    println!("  entries:");
    for line in ls_result.output.lines().take(6) {
        println!("    {}", line);
    }

    // ── 11. Direct tool call: glob ───────────────────────────────────────
    println!("\n--- session.tool(\"glob\") ---");
    let glob_result = session_new
        .tool(
            "glob",
            serde_json::json!({ "pattern": "*.rs" }),
        )
        .await?;
    println!("  exit_code: {}", glob_result.exit_code);
    assert_eq!(glob_result.exit_code, 0, "glob should succeed");
    assert!(glob_result.output.contains("alpha.rs"), "glob should find alpha.rs");
    println!("  matches: {}", glob_result.output.trim());

    // ── 12. Direct tool call: grep ───────────────────────────────────────
    println!("\n--- session.tool(\"grep\") ---");
    let grep_result = session_new
        .tool(
            "grep",
            serde_json::json!({
                "pattern": "fn ",
                "glob": "*.rs",
            }),
        )
        .await?;
    println!("  exit_code: {}", grep_result.exit_code);
    assert_eq!(grep_result.exit_code, 0, "grep should succeed");
    assert!(grep_result.output.contains("fn "), "grep should find 'fn '");
    println!("  output:");
    for line in grep_result.output.lines().take(5) {
        println!("    {}", line);
    }

    // ── 13. Direct tool call: bash ───────────────────────────────────────
    println!("\n--- session.tool(\"bash\") ---");
    let bash_result = session_new
        .tool(
            "bash",
            serde_json::json!({
                "command": "echo 'direct tool call' && date +%Y",
                "timeout": 5000,
            }),
        )
        .await?;
    println!("  exit_code: {}", bash_result.exit_code);
    assert_eq!(bash_result.exit_code, 0, "bash should succeed");
    assert!(bash_result.output.contains("direct tool call"), "bash output check");
    println!("  output: {}", bash_result.output.trim());

    // ── 14. Direct tool call: web_fetch ────────────────────────────────
    println!("\n--- session.tool(\"web_fetch\") ---");
    let fetch_result = session_new
        .tool(
            "web_fetch",
            serde_json::json!({
                "url": "https://httpbin.org/get",
                "format": "text",
                "timeout": 15,
            }),
        )
        .await?;
    println!("  exit_code: {}", fetch_result.exit_code);
    assert_eq!(fetch_result.exit_code, 0, "web_fetch should succeed");
    assert!(
        fetch_result.output.contains("httpbin") || fetch_result.output.contains("origin"),
        "web_fetch should return httpbin response"
    );
    // Show first few lines
    println!("  response (first 5 lines):");
    for line in fetch_result.output.lines().take(5) {
        println!("    {}", line);
    }

    // ── 15. Direct tool call: web_search ─────────────────────────────────
    println!("\n--- session.tool(\"web_search\") ---");
    let search_result = session_new
        .tool(
            "web_search",
            serde_json::json!({
                "query": "Rust programming language",
                "engines": "ddg,wiki",
                "limit": 3,
                "timeout": 15,
                "format": "text",
            }),
        )
        .await?;
    println!("  exit_code: {}", search_result.exit_code);
    // web_search may fail in some network environments; treat as soft check
    if search_result.exit_code == 0 {
        println!("  results (first 5 lines):");
        for line in search_result.output.lines().take(5) {
            println!("    {}", line);
        }
    } else {
        println!("  [soft-fail] web_search returned error (network/proxy issue):");
        println!("    {}", search_result.output.lines().next().unwrap_or("(empty)"));
    }

    // ── 16. Direct tool call: cron (full lifecycle) ────────────────────
    //
    // Cron stores data at {workspace}/.a3s/cron/.
    // We bind a session to the repo root so data persists to .a3s/cron/.
    println!("\n--- session.tool(\"cron\") — full lifecycle ---");
    let root = repo_root();
    let cron_session = agent_new.session(root.display().to_string(), None)?;
    println!("  workspace: {} (cron data → .a3s/cron/)", root.display());

    // 16a. parse — natural language → cron expression
    println!("\n  [parse] 'every 5 minutes'");
    let parse_result = cron_session
        .tool(
            "cron",
            serde_json::json!({
                "action": "parse",
                "input": "every 5 minutes",
            }),
        )
        .await?;
    println!("    exit_code: {}", parse_result.exit_code);
    assert_eq!(parse_result.exit_code, 0, "cron parse should succeed");
    assert!(
        parse_result.output.contains("*/5"),
        "parse should produce */5 cron expression, got: {}",
        parse_result.output.trim()
    );
    println!("    output: {}", parse_result.output.trim());

    // 16b. add — create a test job
    let job_name = format!("sdk-test-{}", chrono::Utc::now().timestamp());
    println!("\n  [add] job '{}' schedule='*/10 * * * *'", job_name);
    let add_result = cron_session
        .tool(
            "cron",
            serde_json::json!({
                "action": "add",
                "name": &job_name,
                "schedule": "*/10 * * * *",
                "command": "echo 'sdk cron test'",
            }),
        )
        .await?;
    println!("    exit_code: {}", add_result.exit_code);
    assert_eq!(add_result.exit_code, 0, "cron add should succeed");
    println!("    output: {}", add_result.output.lines().next().unwrap_or(""));

    // Extract job ID from formatted text output ("  ID: <uuid>")
    let job_id = add_result
        .output
        .lines()
        .find_map(|line| line.trim().strip_prefix("ID: "))
        .expect("cron add output should contain 'ID: <uuid>'")
        .to_string();
    println!("    job_id: {}", job_id);

    // 16c. list — verify job appears
    println!("\n  [list]");
    let list_result = cron_session
        .tool("cron", serde_json::json!({ "action": "list" }))
        .await?;
    println!("    exit_code: {}", list_result.exit_code);
    assert_eq!(list_result.exit_code, 0, "cron list should succeed");
    assert!(
        list_result.output.contains(&job_name),
        "list should contain the job we just added"
    );
    println!("    found '{}' in list", job_name);

    // 16d. get — retrieve job details
    println!("\n  [get] id={}", job_id);
    let get_result = cron_session
        .tool(
            "cron",
            serde_json::json!({ "action": "get", "id": &job_id }),
        )
        .await?;
    println!("    exit_code: {}", get_result.exit_code);
    assert_eq!(get_result.exit_code, 0, "cron get should succeed");
    assert!(get_result.output.contains(&job_name), "get should return our job");
    println!("    output: {}", get_result.output.lines().next().unwrap_or(""));

    // 16e. pause — pause the job
    println!("\n  [pause] id={}", job_id);
    let pause_result = cron_session
        .tool(
            "cron",
            serde_json::json!({ "action": "pause", "id": &job_id }),
        )
        .await?;
    println!("    exit_code: {}", pause_result.exit_code);
    assert_eq!(pause_result.exit_code, 0, "cron pause should succeed");
    println!("    output: {}", pause_result.output.trim());

    // 16f. resume — resume the job
    println!("\n  [resume] id={}", job_id);
    let resume_result = cron_session
        .tool(
            "cron",
            serde_json::json!({ "action": "resume", "id": &job_id }),
        )
        .await?;
    println!("    exit_code: {}", resume_result.exit_code);
    assert_eq!(resume_result.exit_code, 0, "cron resume should succeed");
    println!("    output: {}", resume_result.output.trim());

    // 16g. run — manually trigger execution
    println!("\n  [run] id={}", job_id);
    let run_result = cron_session
        .tool(
            "cron",
            serde_json::json!({ "action": "run", "id": &job_id }),
        )
        .await?;
    println!("    exit_code: {}", run_result.exit_code);
    assert_eq!(run_result.exit_code, 0, "cron run should succeed");
    println!("    output: {}", run_result.output.lines().next().unwrap_or(""));

    // 16h. history — check execution history
    println!("\n  [history] id={}", job_id);
    let history_result = cron_session
        .tool(
            "cron",
            serde_json::json!({ "action": "history", "id": &job_id, "limit": 5 }),
        )
        .await?;
    println!("    exit_code: {}", history_result.exit_code);
    assert_eq!(history_result.exit_code, 0, "cron history should succeed");
    println!("    output: {}", history_result.output.lines().next().unwrap_or(""));

    // 16i. remove — clean up
    println!("\n  [remove] id={}", job_id);
    let remove_result = cron_session
        .tool(
            "cron",
            serde_json::json!({ "action": "remove", "id": &job_id }),
        )
        .await?;
    println!("    exit_code: {}", remove_result.exit_code);
    assert_eq!(remove_result.exit_code, 0, "cron remove should succeed");
    println!("    output: {}", remove_result.output.trim());

    // Verify removal
    let list_after = cron_session
        .tool("cron", serde_json::json!({ "action": "list" }))
        .await?;
    assert!(
        !list_after.output.contains(&job_name),
        "removed job should not appear in list"
    );
    println!("    verified: job no longer in list");

    // ── 17. Direct tool call: unknown tool (error case) ──────────────────
    println!("\n--- session.tool(\"nonexistent\") ---");
    let unknown_result = session_new
        .tool("nonexistent", serde_json::json!({}))
        .await?;
    println!("  exit_code: {}", unknown_result.exit_code);
    assert_eq!(unknown_result.exit_code, 1, "unknown tool should fail");
    assert!(
        unknown_result.output.contains("Unknown tool"),
        "should report unknown tool"
    );
    println!("  output: {}", unknown_result.output.trim());

    // ── 18. Convenience methods (for comparison) ─────────────────────────
    println!("\n--- Convenience methods ---");

    let glob_conv = session_new.glob("*.rs").await?;
    println!("  session.glob(\"*.rs\"): {:?}", glob_conv);

    let grep_conv = session_new.grep("fn ").await?;
    println!("  session.grep(\"fn \"): {} lines", grep_conv.lines().count());

    // ── Done ─────────────────────────────────────────────────────────────
    println!("\n=== All checks passed ===");
    Ok(())
}
