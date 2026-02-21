//! # Queue & Lanes — Priority-Based Tool Scheduling
//!
//! Enables the SessionLaneQueue for priority-based tool execution.
//! Read-only tools (glob, grep, read) run in parallel on the Query lane,
//! while write tools (bash, write, edit) are serialized on the Execute lane.
//!
//! ```bash
//! cargo run --example 09_queue_lanes
//! ```

use a3s_code_core::{Agent, AgentEvent, SessionOptions, SessionQueueConfig};
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
    tracing_subscriber::fmt()
        .with_env_filter("a3s_code_core=info")
        .init();

    let agent = Agent::new(find_config().to_str().unwrap()).await?;
    let workspace = TempDir::new()?;

    // Pre-create some files for the agent to work with
    for i in 1..=5 {
        std::fs::write(
            workspace.path().join(format!("file{i}.txt")),
            format!("Content of file {i}\nLine 2 of file {i}\n"),
        )?;
    }

    // Configure the queue
    let queue_config = SessionQueueConfig::default()
        .with_lane_features() // DLQ, metrics, alerts
        .with_timeout(60_000);

    let opts = SessionOptions::new()
        .with_queue_config(queue_config)
        .with_permissive_policy();

    let session = agent.session(workspace.path().to_str().unwrap(), Some(opts))?;

    println!(
        "Queue & Lanes example — workspace: {}",
        workspace.path().display()
    );
    println!("Queue active: {}\n", session.has_queue());

    // Stream to observe queue events
    let (mut rx, _handle) = session
        .stream(
            "Read all 5 text files in the workspace, then create a summary.md \
             that lists each file's first line. Use glob to find them first.",
            None,
        )
        .await?;

    let mut tool_count = 0;
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::ToolStart { name, .. } => {
                tool_count += 1;
                print!("  🔧 {name}...");
            }
            AgentEvent::ToolEnd { exit_code, .. } => {
                println!(" {}", if exit_code == 0 { "✓" } else { "✗" });
            }
            // Queue-specific events
            AgentEvent::CommandRetry {
                command_type,
                attempt,
                lane,
                ..
            } => {
                println!("  ⟳ Retry: {command_type} on {lane} (attempt {attempt})");
            }
            AgentEvent::CommandDeadLettered {
                command_type,
                lane,
                error,
                ..
            } => {
                println!("  ☠ DLQ: {command_type} on {lane} — {error}");
            }
            AgentEvent::QueueAlert { level, message, .. } => {
                println!("  ⚠ Queue alert [{level}]: {message}");
            }
            AgentEvent::End { usage, .. } => {
                println!(
                    "\n■ Done — {tool_count} tools, {} tokens",
                    usage.total_tokens
                );
                break;
            }
            AgentEvent::Error { message } => {
                eprintln!("✗ Error: {message}");
                break;
            }
            _ => {}
        }
    }

    // Check queue stats
    let stats = session.queue_stats().await;
    println!("\nQueue stats:");
    println!("  Total pending:  {}", stats.total_pending);
    println!("  Total active:   {}", stats.total_active);
    println!("  External tasks: {}", stats.external_pending);

    // Check dead letters
    let dlq = session.dead_letters().await;
    println!("  Dead letters:   {}", dlq.len());

    // Verify output
    let summary = workspace.path().join("summary.md");
    if summary.exists() {
        println!(
            "\n✓ summary.md created ({} bytes)",
            std::fs::metadata(&summary)?.len()
        );
    }

    Ok(())
}
