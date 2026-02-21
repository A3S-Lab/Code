//! # Auto-Compaction & Resilience — Context Window Management
//!
//! Demonstrates auto-compaction (summarizes history when context fills up),
//! parse error recovery, tool timeout, and circuit breaker.
//!
//! ```bash
//! cargo run --example 10_resilience
//! ```

use a3s_code_core::{Agent, AgentEvent, SessionOptions};
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

    // Configure resilience features
    let opts = SessionOptions::new()
        .with_auto_compact(true)
        .with_auto_compact_threshold(0.7) // compact at 70% context usage
        .with_parse_retries(3) // retry up to 3 times on malformed tool args
        .with_tool_timeout(30_000) // 30s per tool
        .with_circuit_breaker(5) // abort after 5 consecutive LLM failures
        .with_permissive_policy();

    let session = agent.session(workspace.path().to_str().unwrap(), Some(opts))?;

    println!(
        "Resilience example — workspace: {}\n",
        workspace.path().display()
    );
    println!("Configuration:");
    println!("  Auto-compact:     enabled (threshold: 70%)");
    println!("  Parse retries:    3");
    println!("  Tool timeout:     30s");
    println!("  Circuit breaker:  5\n");

    // Stream to observe compaction and error events
    let (mut rx, _handle) = session
        .stream(
            "Do the following multi-step task:\n\
             1. Create a file `data.csv` with 20 rows of sample data (id, name, score)\n\
             2. Read the file and compute the average score\n\
             3. Create a `report.md` with the analysis results\n\
             4. Create a bash script `analyze.sh` that uses awk to compute stats\n\
             5. Run the script and include its output in the report",
            None,
        )
        .await?;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::ToolStart { name, .. } => {
                print!("  🔧 {name}...");
            }
            AgentEvent::ToolEnd { exit_code, .. } => {
                println!(" {}", if exit_code == 0 { "✓" } else { "✗" });
            }
            AgentEvent::ContextCompacted {
                before_messages,
                after_messages,
                percent_before,
                ..
            } => {
                println!(
                    "\n  📦 Auto-compacted: {} → {} messages (was {:.0}% full)\n",
                    before_messages,
                    after_messages,
                    percent_before * 100.0
                );
            }
            AgentEvent::TurnEnd { turn, usage } => {
                println!("  └─ Turn {turn} ({} tokens)", usage.total_tokens);
            }
            AgentEvent::End { usage, .. } => {
                println!("\n■ Done ({} tokens total)", usage.total_tokens);
                break;
            }
            AgentEvent::Error { message } => {
                eprintln!("✗ Error: {message}");
                break;
            }
            _ => {}
        }
    }

    // Verify outputs
    println!();
    for name in ["data.csv", "report.md", "analyze.sh"] {
        let path = workspace.path().join(name);
        if path.exists() {
            println!("✓ {name} ({} bytes)", std::fs::metadata(&path)?.len());
        } else {
            println!("⚠ {name} not found");
        }
    }

    Ok(())
}
