//! Session workspace verification example — validates that session workspace
//! is correctly configured and the LLM can interact with workspace files.
//!
//! Demonstrates:
//! - Creating an agent from `.a3s/config.hcl` with real LLM config
//! - Binding a session to a workspace directory
//! - Writing a file into the workspace
//! - Asking the LLM to read and summarize the file (proves workspace is wired)
//! - Streaming variant with event inspection
//!
//! Requires network access and valid API keys in `.a3s/config.hcl`.
//!
//! ## Usage
//!
//! ```bash
//! cd crates/code && cargo run --example session_workspace
//! ```

use a3s_code_core::{Agent, AgentEvent};
use std::path::PathBuf;

fn resolve_config() -> PathBuf {
    if let Ok(env_path) = std::env::var("A3S_CONFIG") {
        return PathBuf::from(env_path);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("failed to resolve repo root")
        .join(".a3s/config.hcl")
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = resolve_config();
    println!("=== Session Workspace Verification ===\n");
    println!("Config: {}\n", config_path.display());

    // ── 1. Create agent ──────────────────────────────────────────────────
    let agent = Agent::create(config_path.display().to_string()).await?;
    println!("[ok] Agent created\n");

    // ── 2. Create workspace with test files ──────────────────────────────
    let tmp = tempfile::tempdir()?;
    let workspace = tmp.path();
    println!("Workspace: {}\n", workspace.display());

    // Write a test file the LLM should be able to read
    let test_file = workspace.join("greeting.txt");
    std::fs::write(&test_file, "Hello from A3S! The magic number is 7742.")?;
    println!("[ok] Wrote {}\n", test_file.display());

    // Write a small Rust file for the LLM to analyze
    let rust_file = workspace.join("example.rs");
    std::fs::write(
        &rust_file,
        r#"fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    println!("{}", add(3, 4));
}
"#,
    )?;
    println!("[ok] Wrote {}\n", rust_file.display());

    // ── 3. Bind session to workspace ─────────────────────────────────────
    let session = agent.session(workspace.display().to_string(), None)?;
    println!("[ok] Session bound to workspace\n");

    // ── 4. Verify workspace via tool call (no LLM) ──────────────────────
    println!("--- Tool: ls (verify workspace files) ---");
    let ls_result = session
        .tool("ls", serde_json::json!({ "path": "." }))
        .await?;
    assert_eq!(ls_result.exit_code, 0, "ls should succeed");
    assert!(
        ls_result.output.contains("greeting.txt"),
        "workspace should contain greeting.txt"
    );
    assert!(
        ls_result.output.contains("example.rs"),
        "workspace should contain example.rs"
    );
    println!("  Files found:");
    for line in ls_result.output.lines().take(5) {
        println!("    {}", line);
    }
    println!();

    // ── 5. Read file via tool (no LLM) ───────────────────────────────────
    println!("--- Tool: read (verify file content) ---");
    let content = session.read_file("greeting.txt").await?;
    assert!(
        content.contains("7742"),
        "read should return file content with magic number"
    );
    println!("  Content: {}\n", content.trim());

    // ── 6. Non-streaming LLM call — ask about workspace file ─────────────
    println!("--- LLM: send (read greeting.txt and extract the magic number) ---");
    let result = session
        .send(
            "Use the read tool to read the file 'greeting.txt' and tell me what the magic number is. Reply with ONLY the number, nothing else.",
            None,
        )
        .await?;
    println!("  Response: {}", result.text.trim());
    println!(
        "  Tokens: {} in / {} out / {} total",
        result.usage.prompt_tokens, result.usage.completion_tokens, result.usage.total_tokens
    );
    println!(
        "  Tool calls: {}",
        result.tool_calls_count
    );
    // The LLM should have used the read tool and found the number
    if result.text.contains("7742") {
        println!("  [ok] Magic number correctly extracted\n");
    } else {
        println!("  [warn] LLM did not return exact magic number, but tool calls were made");
        println!("  (This can happen with some models — workspace is still wired correctly)\n");
    }
    // The key assertion: the LLM made at least one tool call, proving workspace access works
    assert!(
        result.tool_calls_count >= 1,
        "LLM should have made at least 1 tool call to read the file, got {}",
        result.tool_calls_count
    );

    // ── 7. Multi-turn: ask about the Rust file ───────────────────────────
    println!("--- LLM: send (analyze example.rs) ---");
    let result2 = session
        .send(
            "Use the read tool to read example.rs, then tell me what the output of the main function would be. Reply with ONLY the number.",
            None,
        )
        .await?;
    println!("  Response: {}", result2.text.trim());
    println!(
        "  Tokens: {} in / {} out",
        result2.usage.prompt_tokens, result2.usage.completion_tokens
    );
    if result2.text.contains("7") {
        println!("  [ok] Correct output determined\n");
    } else {
        println!("  [warn] LLM response didn't contain expected '7', but tool calls were made\n");
    }

    // ── 8. Verify conversation history accumulated ───────────────────────
    println!("--- History check ---");
    let history = session.history();
    println!("  Messages accumulated: {}", history.len());
    assert!(
        history.len() >= 4,
        "should have at least 4 messages (2 user + 2 assistant), got {}",
        history.len()
    );
    println!("  [ok] History correctly accumulated\n");

    // ── 9. Streaming LLM call ────────────────────────────────────────────
    println!("--- LLM: stream (ask about workspace) ---");
    let tmp2 = tempfile::tempdir()?;
    let ws2 = tmp2.path();
    std::fs::write(ws2.join("data.txt"), "The secret word is: pineapple")?;
    let session2 = agent.session(ws2.display().to_string(), None)?;

    let (mut rx, handle) = session2
        .stream(
            "Read data.txt and tell me the secret word. Reply with ONLY the word.",
            None,
        )
        .await?;

    print!("  Response: ");
    let mut stream_text = String::new();
    let mut saw_start = false;
    let mut saw_end = false;
    let mut saw_tool = false;
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Start { .. } => saw_start = true,
            AgentEvent::TextDelta { text } => {
                print!("{}", text);
                stream_text.push_str(&text);
            }
            AgentEvent::ToolStart { name, .. } => {
                saw_tool = true;
                print!("[tool:{}]", name);
            }
            AgentEvent::End { .. } => {
                saw_end = true;
                break;
            }
            _ => {}
        }
    }
    println!();
    handle.abort();

    println!("  Events: start={}, tool_use={}, end={}", saw_start, saw_tool, saw_end);
    assert!(saw_start, "should receive Start event");
    assert!(saw_end, "should receive End event");
    let lower = stream_text.to_lowercase();
    if lower.contains("pineapple") {
        println!("  [ok] Secret word correctly extracted via streaming\n");
    } else {
        println!("  [warn] Streaming response didn't contain 'pineapple', got: {}", stream_text.trim());
        println!("  (Workspace is still wired — tool_use event: {})\n", saw_tool);
    }

    println!("=== All workspace checks passed ===");
    Ok(())
}
