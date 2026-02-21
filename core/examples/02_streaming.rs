//! # Streaming Events — Real-Time Agent Visibility
//!
//! Subscribe to the agent's event stream to watch it think and act in real time.
//! Demonstrates all event types: TextDelta, ToolStart, ToolEnd, TurnStart, etc.
//!
//! ```bash
//! cargo run --example 02_streaming
//! ```

use a3s_code_core::{Agent, AgentEvent};
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

    // Pre-create a file for the agent to work with
    std::fs::write(
        workspace.path().join("users.json"),
        r#"[{"name":"Alice","role":"admin"},{"name":"Bob","role":"viewer"},{"name":"Carol","role":"editor"}]"#,
    )?;

    let session = agent.session(workspace.path().to_str().unwrap(), None)?;
    println!(
        "Streaming example — workspace: {}\n",
        workspace.path().display()
    );

    // Stream returns (receiver, join_handle)
    let (mut rx, _handle) = session
        .stream(
            "Read users.json, then create a Markdown table in users.md listing each user's name and role.",
            None,
        )
        .await?;

    let mut tool_count = 0usize;
    let mut text_chars = 0usize;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Start { .. } => {
                println!("▶ Agent started");
            }
            AgentEvent::TurnStart { turn } => {
                println!("┌─ Turn {turn}");
            }
            AgentEvent::ToolStart { name, .. } => {
                tool_count += 1;
                print!("│  🔧 {name}...");
            }
            AgentEvent::ToolEnd { exit_code, .. } => {
                let icon = if exit_code == 0 { "✓" } else { "✗" };
                println!(" {icon}");
            }
            AgentEvent::ToolInputDelta { .. } => {
                // Tool argument streaming — could show partial JSON here
            }
            AgentEvent::TextDelta { text } => {
                text_chars += text.len();
            }
            AgentEvent::TurnEnd { turn, usage } => {
                println!("└─ Turn {turn} done ({} tokens)", usage.total_tokens);
            }
            AgentEvent::End { usage, .. } => {
                println!("\n■ Agent finished");
                println!(
                    "  Tools: {tool_count}, Text: {text_chars} chars, Tokens: {}",
                    usage.total_tokens
                );
                break;
            }
            AgentEvent::Error { message } => {
                eprintln!("✗ Error: {message}");
                break;
            }
            _ => {} // #[non_exhaustive] — always include wildcard
        }
    }

    // Verify output
    let md = workspace.path().join("users.md");
    if md.exists() {
        println!(
            "\n✓ users.md created ({} bytes)",
            std::fs::metadata(&md)?.len()
        );
    }

    Ok(())
}
