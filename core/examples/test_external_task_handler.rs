//! External Task Handler Integration Test
//!
//! Demonstrates the Multi-Machine External Task pattern:
//! 1. Coordinator creates a session with Execute lane set to External mode
//! 2. Agent streams a prompt that triggers bash/write/edit tool calls
//! 3. ExternalTaskPending events fire — coordinator polls pending tasks
//! 4. Coordinator processes tasks locally (simulating a remote worker)
//! 5. Coordinator completes tasks via complete_external_task()
//!
//! Run with: cargo run --example test_external_task_handler

use a3s_code_core::agent::AgentEvent;
use a3s_code_core::permissions::PermissionPolicy;
use a3s_code_core::queue::{
    ExternalTaskResult, LaneHandlerConfig, SessionLane, TaskHandlerMode,
};
use a3s_code_core::{Agent, SessionOptions, SessionQueueConfig};
use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

fn find_config() -> Result<PathBuf> {
    let home_config = dirs::home_dir()
        .map(|h| h.join(".a3s/config.hcl"))
        .filter(|p| p.exists());

    if let Some(config) = home_config {
        return Ok(config);
    }

    let project_config = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.join(".a3s/config.hcl"))
        .filter(|p| p.exists());

    project_config.ok_or_else(|| anyhow::anyhow!("Config file not found"))
}

/// Simulate a remote worker executing a bash command
fn worker_execute_bash(command: &str, working_dir: &str) -> (bool, String, i32, Option<String>) {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(working_dir)
        .output();

    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let success = out.status.success();
            let exit_code = out.status.code().unwrap_or(1);
            let error = if success { None } else { Some(stderr) };
            (success, stdout, exit_code, error)
        }
        Err(e) => (false, String::new(), 1, Some(e.to_string())),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,a3s_code_core=info")
        .init();

    println!("🚀 A3S Code - External Task Handler Integration Test\n");
    println!("{}", "=".repeat(80));

    let config_path = find_config()?;
    println!("📄 Using config: {}", config_path.display());
    println!("{}", "=".repeat(80));

    let agent = Agent::new(config_path.to_str().unwrap()).await?;

    // =========================================================================
    // Test 1: External mode — Execute lane tasks routed to external handler
    // =========================================================================
    println!("\n📦 Test 1: External Task Handler (Execute Lane → External)");
    println!("{}", "-".repeat(80));

    // 1. Create session with queue enabled
    let queue_config = SessionQueueConfig::default()
        .with_lane_features()
        .with_timeout(60_000);

    let mut opts = SessionOptions::default().with_queue_config(queue_config);
    opts.permission_checker = Some(Arc::new(PermissionPolicy::permissive()));
    let session = agent.session(".", Some(opts))?;

    // 2. Route Execute lane to External mode
    session
        .set_lane_handler(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::External,
                timeout_ms: 60_000,
            },
        )
        .await;

    println!("✓ Session created with Execute lane → External mode");
    println!("  Query lane: Internal (read, glob, grep run locally)");
    println!("  Execute lane: External (bash, write, edit → ExternalTask)");
    println!();

    // 3. Stream a prompt that will trigger Execute-lane tools (bash)
    let start = Instant::now();

    let (mut rx, _handle) = session
        .stream(
            "Run these bash commands and tell me the results:\n\
             1. echo 'Hello from external worker'\n\
             2. date '+%Y-%m-%d %H:%M:%S'\n\
             3. uname -s",
            None,
        )
        .await?;

    // 4. Event loop: handle external tasks as they arrive
    let mut external_tasks_processed = 0u32;
    let mut text_output = String::new();

    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::ExternalTaskPending { task_id, command_type, .. } => {
                println!("  📥 ExternalTaskPending: {} ({})", &task_id[..8], command_type);

                // Poll all pending tasks
                let tasks = session.pending_external_tasks().await;
                for task in tasks {
                    println!("  🔧 Worker processing: {} → {}", task.command_type, &task.task_id[..8]);

                    // Execute the task (simulating a remote worker)
                    let (success, output, exit_code, error) = if task.command_type == "bash" {
                        let cmd = task.payload["command"].as_str().unwrap_or("echo 'no command'");
                        let dir = task.payload["working_dir"].as_str().unwrap_or(".");
                        worker_execute_bash(cmd, dir)
                    } else {
                        // For non-bash tasks, return a placeholder
                        (true, format!("External handler processed: {}", task.command_type), 0, None)
                    };

                    println!("  ✅ Worker result: success={}, exit_code={}", success, exit_code);
                    if !output.is_empty() {
                        let preview = output.trim();
                        let preview = if preview.len() > 60 { &preview[..60] } else { preview };
                        println!("     Output: {}", preview);
                    }

                    // Complete the external task
                    let completed = session
                        .complete_external_task(
                            &task.task_id,
                            ExternalTaskResult {
                                success,
                                result: serde_json::json!({
                                    "output": output,
                                    "exit_code": exit_code,
                                }),
                                error,
                            },
                        )
                        .await;

                    if completed {
                        external_tasks_processed += 1;
                        println!("  📤 Task {} completed and returned to agent", &task.task_id[..8]);
                    }
                }
            }
            AgentEvent::TextDelta { text } => {
                text_output.push_str(&text);
                print!("{text}");
            }
            AgentEvent::ToolStart { name, .. } => {
                println!("\n  🔨 Tool: {name}");
            }
            AgentEvent::End { .. } => {
                println!();
                break;
            }
            AgentEvent::Error { message } => {
                println!("\n  ❌ Error: {message}");
                break;
            }
            _ => {}
        }
    }

    let duration = start.elapsed();

    println!("\n{}", "-".repeat(80));
    println!("📊 Results:");
    println!("  Duration: {:.2}s", duration.as_secs_f64());
    println!("  External tasks processed: {}", external_tasks_processed);
    println!("  Response length: {} chars", text_output.len());

    // =========================================================================
    // Test 2: Hybrid mode — local execution + external notification
    // =========================================================================
    println!("\n\n📦 Test 2: Hybrid Mode (Execute Lane → Hybrid)");
    println!("{}", "-".repeat(80));

    // Switch to Hybrid mode
    session
        .set_lane_handler(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::Hybrid,
                timeout_ms: 60_000,
            },
        )
        .await;

    println!("✓ Execute lane switched to Hybrid mode");
    println!("  Tools execute locally AND emit ExternalTaskPending events");
    println!();

    let start = Instant::now();
    let result = session
        .send("Run: echo 'hybrid mode test'", None)
        .await?;

    let duration = start.elapsed();
    println!("✓ Completed in {:.2}s", duration.as_secs_f64());
    println!("  Response: {}", &result.text[..result.text.len().min(120)]);
    println!("  Tool calls: {}", result.tool_calls_count);

    // =========================================================================
    // Test 3: Dynamic lane switching
    // =========================================================================
    println!("\n\n📦 Test 3: Dynamic Lane Switching");
    println!("{}", "-".repeat(80));

    // Switch back to Internal
    session
        .set_lane_handler(
            SessionLane::Execute,
            LaneHandlerConfig {
                mode: TaskHandlerMode::Internal,
                timeout_ms: 60_000,
            },
        )
        .await;

    println!("✓ Execute lane switched back to Internal mode");

    let start = Instant::now();
    let result = session
        .send("Run: echo 'back to internal mode'", None)
        .await?;

    let duration = start.elapsed();
    println!("✓ Completed in {:.2}s", duration.as_secs_f64());
    println!("  Response: {}", &result.text[..result.text.len().min(120)]);

    println!("\n{}", "=".repeat(80));
    println!("✅ All external task handler tests completed!");
    println!("{}", "=".repeat(80));

    Ok(())
}
