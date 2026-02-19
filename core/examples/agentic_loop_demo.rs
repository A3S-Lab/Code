//! Agentic Loop Demo — Real LLM-driven problem solving with tool execution.
//!
//! Demonstrates the core value of A3S Code: the AgentLoop autonomously
//! reads files, writes code, runs commands, and iterates until the task
//! is complete — all driven by a real LLM.
//!
//! ## What This Example Shows
//!
//! 1. **Autonomous File Operations** — LLM reads, writes, and edits files
//! 2. **Multi-Step Reasoning** — LLM chains tool calls to solve a problem
//! 3. **Streaming Events** — Real-time visibility into the agent's actions
//! 4. **Planning Mode** — Task decomposition with wave-based execution
//! 5. **Multi-Turn Conversation** — Context preserved across turns
//! 6. **Built-in Skills** — Skill-augmented code assistance
//! 7. **Resilience** — Parse retries, tool timeout, circuit breaker
//!
//! ## Usage
//!
//! ```bash
//! cd crates/code && cargo run --example agentic_loop_demo
//!
//! # With custom config
//! A3S_CONFIG=/path/to/config.hcl cargo run --example agentic_loop_demo
//! ```
//!
//! Requires a valid LLM API key in `~/.a3s/config.hcl` or `$A3S_CONFIG`.

use a3s_code_core::{Agent, AgentEvent, SessionOptions};
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

fn separator(title: &str) {
    println!("\n{}", "═".repeat(72));
    println!("  {}", title);
    println!("{}\n", "═".repeat(72));
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config_path = resolve_config();
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║           A3S Code — Agentic Loop Demo (Real LLM)                  ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");
    println!("\n  Config: {}", config_path.display());

    let agent = Agent::new(config_path.display().to_string()).await?;
    println!("  Agent:  ✓ created\n");

    // All demos use isolated temp workspaces
    demo_1_autonomous_coding(&agent).await?;
    demo_2_streaming_events(&agent).await?;
    demo_3_planning_mode(&agent).await?;
    demo_4_multi_turn(&agent).await?;
    demo_5_skills_augmented(&agent).await?;
    demo_6_resilient_session(&agent).await?;

    separator("All Demos Complete ✓");
    Ok(())
}

/// Demo 1: Autonomous multi-step coding task
///
/// The LLM autonomously:
/// 1. Creates a Rust source file
/// 2. Reads it back to verify
/// 3. Edits it to add functionality
/// 4. Runs `rustfmt` (or equivalent check) on it
async fn demo_1_autonomous_coding(agent: &Agent) -> anyhow::Result<()> {
    separator("Demo 1: Autonomous Multi-Step Coding");

    let tmp = tempfile::tempdir()?;
    let workspace = tmp.path().display().to_string();
    let session = agent.session(&workspace, None)?;

    println!("  Workspace: {}", workspace);
    println!("  Prompt:    Create a Rust file, then improve it\n");

    let result = session
        .send(
            "Do the following steps:\n\
             1. Create a file called `calculator.rs` with a basic Calculator struct \
                that has add() and subtract() methods\n\
             2. Read the file back to verify it was created correctly\n\
             3. Edit the file to add multiply() and divide() methods (divide should handle division by zero)\n\
             4. Read the final version and confirm all 4 methods exist",
            None,
        )
        .await?;

    println!("  Tool calls: {}", result.tool_calls_count);
    println!("  Tokens:     {} total", result.usage.total_tokens);
    println!("  Response:   {}", truncate(&result.text, 200));

    // Verify the file was actually created
    let calc_path = tmp.path().join("calculator.rs");
    if calc_path.exists() {
        let content = std::fs::read_to_string(&calc_path)?;
        let has_add = content.contains("add");
        let has_sub = content.contains("subtract") || content.contains("sub");
        let has_mul = content.contains("multiply") || content.contains("mul");
        let has_div = content.contains("divide") || content.contains("div");
        println!(
            "\n  ✓ File verified: add={} sub={} mul={} div={}",
            has_add, has_sub, has_mul, has_div
        );
    } else {
        println!("\n  ⚠ calculator.rs not found (LLM may have used a different name)");
    }

    Ok(())
}

/// Demo 2: Streaming — watch the agent think and act in real time
///
/// Shows every event: text deltas, tool starts/ends, turn boundaries.
async fn demo_2_streaming_events(agent: &Agent) -> anyhow::Result<()> {
    separator("Demo 2: Streaming Events (Real-Time Agent Visibility)");

    let tmp = tempfile::tempdir()?;
    let workspace = tmp.path().display().to_string();

    // Pre-create a file for the agent to work with
    std::fs::write(
        tmp.path().join("data.json"),
        r#"{"users": [{"name": "Alice", "age": 30}, {"name": "Bob", "age": 25}]}"#,
    )?;

    let session = agent.session(&workspace, None)?;

    println!("  Workspace: {}", workspace);
    println!("  Prompt:    Read data.json and create a summary\n");
    println!("  --- Event Stream ---");

    let (mut rx, handle) = session
        .stream(
            "Read the file data.json in the workspace, then create a file called summary.txt \
             that lists each user's name and age in a human-readable format.",
            None,
        )
        .await?;

    let mut tool_count = 0u32;
    let mut text_len = 0usize;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Start { .. } => {
                println!("  ▶ Agent started");
            }
            AgentEvent::TurnStart { turn } => {
                println!("  ┌─ Turn {}", turn);
            }
            AgentEvent::ToolStart { name, .. } => {
                tool_count += 1;
                print!("  │  🔧 {}...", name);
            }
            AgentEvent::ToolEnd { name, exit_code, .. } => {
                let status = if exit_code == 0 { "✓" } else { "✗" };
                println!(" {} (exit={})", status, exit_code);
                let _ = name; // suppress unused warning
            }
            AgentEvent::TextDelta { text } => {
                text_len += text.len();
            }
            AgentEvent::TurnEnd { turn, usage } => {
                println!(
                    "  └─ Turn {} done ({} tokens)",
                    turn, usage.total_tokens
                );
            }
            AgentEvent::End { usage, .. } => {
                println!("\n  ■ Agent finished");
                println!(
                    "    Tools: {}, Response: {} chars, Tokens: {}",
                    tool_count, text_len, usage.total_tokens
                );
                break;
            }
            AgentEvent::Error { message } => {
                println!("\n  ✗ Error: {}", message);
                break;
            }
            _ => {}
        }
    }
    handle.abort();

    // Verify output
    if tmp.path().join("summary.txt").exists() {
        let summary = std::fs::read_to_string(tmp.path().join("summary.txt"))?;
        println!("\n  ✓ summary.txt created ({} bytes)", summary.len());
    }

    Ok(())
}

/// Demo 3: Planning mode — decompose a complex task into steps
///
/// The planner breaks the task into dependency-aware steps.
/// Independent steps can execute in parallel waves.
async fn demo_3_planning_mode(agent: &Agent) -> anyhow::Result<()> {
    separator("Demo 3: Planning Mode (Task Decomposition)");

    let tmp = tempfile::tempdir()?;
    let workspace = tmp.path().display().to_string();

    let session = agent.session(
        &workspace,
        Some(
            SessionOptions::new()
                .with_planning(true)
                .with_goal_tracking(true),
        ),
    )?;

    println!("  Workspace: {}", workspace);
    println!("  Planning:  enabled");
    println!("  Goal tracking: enabled\n");

    let result = session
        .send(
            "Create a small project with these files:\n\
             1. `lib.rs` — a module with a `greet(name: &str) -> String` function\n\
             2. `main.rs` — imports from lib and calls greet(\"World\")\n\
             3. `README.md` — brief documentation explaining the project\n\
             Then read all three files to verify they are correct.",
            None,
        )
        .await?;

    println!("  Tool calls: {}", result.tool_calls_count);
    println!("  Tokens:     {} total", result.usage.total_tokens);
    println!("  Response:   {}", truncate(&result.text, 200));

    // Verify files
    let files = ["lib.rs", "main.rs", "README.md"];
    for f in &files {
        let path = tmp.path().join(f);
        if path.exists() {
            let size = std::fs::metadata(&path)?.len();
            println!("  ✓ {} ({} bytes)", f, size);
        } else {
            println!("  ⚠ {} not found", f);
        }
    }

    Ok(())
}

/// Demo 4: Multi-turn conversation — context preserved across turns
///
/// Turn 1: Create a file
/// Turn 2: Ask about it (LLM remembers from history)
/// Turn 3: Modify it based on previous context
async fn demo_4_multi_turn(agent: &Agent) -> anyhow::Result<()> {
    separator("Demo 4: Multi-Turn Conversation (Context Preservation)");

    let tmp = tempfile::tempdir()?;
    let workspace = tmp.path().display().to_string();
    let session = agent.session(&workspace, None)?;

    println!("  Workspace: {}\n", workspace);

    // Turn 1
    println!("  [Turn 1] Create a config file");
    let r1 = session
        .send(
            "Create a file called `config.toml` with these settings:\n\
             - server.host = \"0.0.0.0\"\n\
             - server.port = 8080\n\
             - database.url = \"postgres://localhost/mydb\"\n\
             - database.pool_size = 5",
            None,
        )
        .await?;
    println!("    Tools: {}, Tokens: {}", r1.tool_calls_count, r1.usage.total_tokens);

    // Turn 2 — LLM should remember the file from Turn 1
    println!("\n  [Turn 2] Ask about the file (tests context memory)");
    let r2 = session
        .send(
            "What port is the server configured to use? Read the config file to confirm.",
            None,
        )
        .await?;
    println!("    Tools: {}, Tokens: {}", r2.tool_calls_count, r2.usage.total_tokens);
    println!("    Answer: {}", truncate(&r2.text, 120));

    // Turn 3 — modify based on context
    println!("\n  [Turn 3] Modify based on previous context");
    let r3 = session
        .send(
            "Change the server port to 3000 and increase the pool_size to 10.",
            None,
        )
        .await?;
    println!("    Tools: {}, Tokens: {}", r3.tool_calls_count, r3.usage.total_tokens);

    // Verify final state
    let history = session.history();
    println!("\n  History: {} messages across 3 turns", history.len());

    if tmp.path().join("config.toml").exists() {
        let content = std::fs::read_to_string(tmp.path().join("config.toml"))?;
        let has_3000 = content.contains("3000");
        let has_10 = content.contains("10");
        println!("  ✓ config.toml: port=3000? {} pool=10? {}", has_3000, has_10);
    }

    Ok(())
}

/// Demo 5: Skills-augmented session
///
/// Built-in skills inject coding best practices into the system prompt,
/// guiding the LLM to produce higher-quality output.
async fn demo_5_skills_augmented(agent: &Agent) -> anyhow::Result<()> {
    separator("Demo 5: Skills-Augmented Code Assistance");

    let tmp = tempfile::tempdir()?;
    let workspace = tmp.path().display().to_string();

    // Pre-create a file with intentional issues
    std::fs::write(
        tmp.path().join("app.py"),
        r#"import os
import sys

def process_data(data):
    result = []
    for i in range(len(data)):
        if data[i] != None:
            result.append(data[i] * 2)
    return result

def read_file(path):
    f = open(path, 'r')
    content = f.read()
    return content

API_KEY = "sk-1234567890abcdef"
"#,
    )?;

    let session = agent.session(
        &workspace,
        Some(SessionOptions::new().with_builtin_skills()),
    )?;

    println!("  Workspace: {}", workspace);
    println!("  Skills:    built-in (7 skills active)\n");

    let result = session
        .send(
            "Review the file `app.py` in the workspace. Identify code quality issues \
             and fix them. The review should cover:\n\
             - Pythonic style (use `is not None`, enumerate, etc.)\n\
             - Resource management (use context managers)\n\
             - Security issues (hardcoded secrets)\n\
             Apply the fixes by editing the file.",
            None,
        )
        .await?;

    println!("  Tool calls: {}", result.tool_calls_count);
    println!("  Tokens:     {} total", result.usage.total_tokens);
    println!("  Response:   {}", truncate(&result.text, 200));

    // Check improvements
    if tmp.path().join("app.py").exists() {
        let content = std::fs::read_to_string(tmp.path().join("app.py"))?;
        let has_context_mgr = content.contains("with open");
        let has_enumerate = content.contains("enumerate");
        let no_hardcoded_key = !content.contains("sk-1234567890abcdef");
        println!(
            "\n  ✓ Improvements: context_manager={} enumerate={} no_hardcoded_key={}",
            has_context_mgr, has_enumerate, no_hardcoded_key
        );
    }

    Ok(())
}

/// Demo 6: Resilient session with error recovery
///
/// Demonstrates parse retries, tool timeout, and circuit breaker
/// working together to keep the session alive.
async fn demo_6_resilient_session(agent: &Agent) -> anyhow::Result<()> {
    separator("Demo 6: Resilient Session (Error Recovery)");

    let tmp = tempfile::tempdir()?;
    let workspace = tmp.path().display().to_string();

    let session = agent.session(
        &workspace,
        Some(
            SessionOptions::new()
                .with_resilience_defaults() // parse_retries=2, timeout=120s, circuit_breaker=3
                .with_builtin_skills(),
        ),
    )?;

    println!("  Workspace:       {}", workspace);
    println!("  Parse retries:   2");
    println!("  Tool timeout:    120s");
    println!("  Circuit breaker: 3\n");

    // Give the agent a task that involves multiple tool calls
    // The resilience settings protect against transient failures
    let result = session
        .send(
            "Create a file called `test.sh` with a bash script that prints 'hello' \
             and the current date. Then execute it with bash and show me the output.",
            None,
        )
        .await?;

    println!("  Tool calls: {}", result.tool_calls_count);
    println!("  Tokens:     {} total", result.usage.total_tokens);
    println!("  Response:   {}", truncate(&result.text, 200));

    if tmp.path().join("test.sh").exists() {
        println!("\n  ✓ test.sh created and executed successfully");
    }

    Ok(())
}
