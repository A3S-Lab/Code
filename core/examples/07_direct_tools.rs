//! # Direct Tool Execution — Bypass the LLM
//!
//! Call tools directly without involving the LLM. Useful for scripting,
//! testing, or building custom pipelines on top of A3S Code's tool layer.
//!
//! ```bash
//! cargo run --example 07_direct_tools
//! ```

use a3s_code_core::{Agent, SessionOptions};
use std::path::PathBuf;
use tempfile::TempDir;

fn find_config() -> PathBuf {
    if let Ok(p) = std::env::var("A3S_CONFIG") {
        return PathBuf::from(p);
    }
    let home = dirs::home_dir().expect("no home dir");
    let candidates = [
        home.join(".a3s/config.hcl"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.a3s/config.hcl"),
    ];
    candidates
        .into_iter()
        .find(|p| p.exists())
        .expect("Config not found. Set A3S_CONFIG or create ~/.a3s/config.hcl")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let agent = Agent::new(find_config().to_str().unwrap()).await?;
    let workspace = TempDir::new()?;
    let ws = workspace.path().to_str().unwrap();

    let opts = SessionOptions::new().with_permissive_policy();
    let session = agent.session(ws, Some(opts))?;

    println!("Direct tool execution — workspace: {}\n", ws);

    // ── 1. Write a file ──
    println!("1. Write a file via `write` tool");
    let r = session
        .tool(
            "write",
            serde_json::json!({
                "path": "hello.txt",
                "content": "Hello from direct tool execution!\nLine 2.\nLine 3."
            }),
        )
        .await?;
    println!(
        "   exit={} output={}\n",
        r.exit_code,
        truncate(&r.output, 80)
    );

    // ── 2. Read it back ──
    println!("2. Read the file via `read` tool");
    let r = session
        .tool("read", serde_json::json!({"path": "hello.txt"}))
        .await?;
    println!(
        "   exit={} content:\n{}\n",
        r.exit_code,
        indent(&r.output, "   ")
    );

    // ── 3. Edit the file ──
    println!("3. Edit the file via `edit` tool");
    let r = session
        .tool(
            "edit",
            serde_json::json!({
                "path": "hello.txt",
                "old_string": "Line 2.",
                "new_string": "Line 2 — edited!"
            }),
        )
        .await?;
    println!(
        "   exit={} output={}\n",
        r.exit_code,
        truncate(&r.output, 80)
    );

    // ── 4. Glob for files ──
    println!("4. Glob for *.txt files");
    let r = session
        .tool("glob", serde_json::json!({"pattern": "*.txt"}))
        .await?;
    println!("   exit={} matches: {}\n", r.exit_code, r.output.trim());

    // ── 5. Run a bash command ──
    println!("5. Execute a bash command");
    let r = session
        .tool(
            "bash",
            serde_json::json!({"command": "cat hello.txt && echo '---' && wc -l hello.txt"}),
        )
        .await?;
    println!(
        "   exit={} output:\n{}\n",
        r.exit_code,
        indent(&r.output, "   ")
    );

    // ── 6. Grep for a pattern ──
    println!("6. Grep for 'edited'");
    let r = session
        .tool(
            "grep",
            serde_json::json!({"pattern": "edited", "path": "."}),
        )
        .await?;
    println!(
        "   exit={} output: {}\n",
        r.exit_code,
        truncate(&r.output, 120)
    );

    // ── 7. List directory ──
    println!("7. List workspace directory");
    let r = session.tool("ls", serde_json::json!({"path": "."})).await?;
    println!(
        "   exit={} output:\n{}",
        r.exit_code,
        indent(&r.output, "   ")
    );

    // ── Convenience methods ──
    println!("\n─── Convenience Methods ───\n");

    let content = session.read_file("hello.txt").await?;
    println!("read_file: {}", truncate(&content, 60));

    let bash_out = session.bash("echo 'convenience!'").await?;
    println!("bash:      {}", bash_out.trim());

    let files = session.glob("*.txt").await?;
    println!("glob:      {:?}", files);

    let grep_out = session.grep("edited").await?;
    println!("grep:      {}", truncate(&grep_out, 60));

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

fn indent(s: &str, prefix: &str) -> String {
    s.lines()
        .map(|l| format!("{prefix}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}
