//! # Planning Mode — Task Decomposition with Goal Tracking
//!
//! Enables the planner so the agent decomposes complex tasks into steps,
//! executes them in order (or in parallel), and tracks goal progress.
//!
//! ```bash
//! cargo run --example 05_planning
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

    let opts = SessionOptions::new()
        .with_planning(true)
        .with_goal_tracking(true)
        .with_permissive_policy();

    let session = agent.session(workspace.path().to_str().unwrap(), Some(opts))?;
    println!(
        "Planning example — workspace: {}\n",
        workspace.path().display()
    );

    // Stream to observe planning events
    let (mut rx, _handle) = session
        .stream(
            "Create a small Rust project:\n\
             1. `src/lib.rs` — a `greet(name: &str) -> String` function\n\
             2. `src/main.rs` — calls greet(\"World\") and prints the result\n\
             3. `Cargo.toml` — minimal manifest for the project\n\
             4. `README.md` — brief docs explaining the project\n\
             Then read all files to verify correctness.",
            None,
        )
        .await?;

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::PlanningStart { .. } => {
                println!("📋 Planning started...");
            }
            AgentEvent::PlanningEnd { plan, .. } => {
                println!("📋 Plan created: {} steps", plan.steps.len());
                for (i, step) in plan.steps.iter().enumerate() {
                    println!("   {}. {}", i + 1, step.content);
                }
                println!();
            }
            AgentEvent::StepStart {
                description,
                step_number,
                total_steps,
                ..
            } => {
                println!("▶ Step {step_number}/{total_steps}: {description}");
            }
            AgentEvent::StepEnd {
                step_number,
                total_steps,
                status,
                ..
            } => {
                println!("  Step {step_number}/{total_steps}: {status:?}");
            }
            AgentEvent::GoalExtracted { goal } => {
                println!("🎯 Goal: {}", goal.description);
            }
            AgentEvent::GoalAchieved {
                goal,
                total_steps,
                duration_ms,
            } => {
                println!("✅ Goal achieved: {goal} ({total_steps} steps, {duration_ms}ms)");
            }
            AgentEvent::ToolStart { name, .. } => {
                print!("   🔧 {name}...");
            }
            AgentEvent::ToolEnd { exit_code, .. } => {
                println!(" {}", if exit_code == 0 { "✓" } else { "✗" });
            }
            AgentEvent::End { usage, .. } => {
                println!("\n■ Done ({} tokens)", usage.total_tokens);
                break;
            }
            AgentEvent::Error { message } => {
                eprintln!("✗ Error: {message}");
                break;
            }
            _ => {}
        }
    }

    // Verify files
    println!();
    for name in ["src/lib.rs", "src/main.rs", "Cargo.toml", "README.md"] {
        let path = workspace.path().join(name);
        if path.exists() {
            println!("✓ {name} ({} bytes)", std::fs::metadata(&path)?.len());
        } else {
            println!("⚠ {name} not found");
        }
    }

    Ok(())
}
