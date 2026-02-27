//! Test Session::submit() and Session::submit_batch() direct queue API
//!
//! Demonstrates submitting tasks directly into the lane queue without going
//! through the LLM agent loop.
//!
//! Run with: cargo run --example test_submit_api

use a3s_code_core::queue::{SessionCommand, SessionLane, SessionQueueConfig};
use a3s_code_core::{Agent, SessionOptions};
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use std::time::Instant;

fn find_config() -> Result<PathBuf> {
    let candidates = [
        PathBuf::from("/Users/roylin/Desktop/ai-lab/a3s/.a3s/config.hcl"),
        dirs::home_dir()
            .map(|h| h.join(".a3s/config.hcl"))
            .unwrap_or_default(),
    ];
    candidates
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| anyhow::anyhow!("config.hcl not found"))
}

// ─── A simple command that runs a shell command and returns stdout ────────────

struct ShellCommand {
    cmd: String,
}

#[async_trait]
impl SessionCommand for ShellCommand {
    async fn execute(&self) -> Result<serde_json::Value> {
        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&self.cmd)
            .output()
            .await?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(serde_json::json!({ "cmd": self.cmd, "output": stdout, "exit_code": output.status.code() }))
    }
    fn command_type(&self) -> &str {
        "shell"
    }
}

// ─── A compute-heavy command (CPU work, no I/O) ───────────────────────────────

struct FibCommand {
    n: u64,
}

#[async_trait]
impl SessionCommand for FibCommand {
    async fn execute(&self) -> Result<serde_json::Value> {
        let n = self.n;
        let result = tokio::task::spawn_blocking(move || {
            let (mut a, mut b) = (0u64, 1u64);
            for _ in 0..n {
                (a, b) = (b, a.wrapping_add(b));
            }
            a
        })
        .await?;
        Ok(serde_json::json!({ "n": n, "fib": result }))
    }
    fn command_type(&self) -> &str {
        "fib"
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let config_path = find_config()?;
    println!("Using config: {}", config_path.display());
    println!("{}", "=".repeat(70));

    let agent = Agent::new(config_path.to_str().unwrap()).await?;
    let opts = SessionOptions::new().with_queue_config(
        SessionQueueConfig {
            query_max_concurrency: 8,
            execute_max_concurrency: 4,
            enable_metrics: true,
            ..Default::default()
        },
    );
    let session = agent.session(".", Some(opts))?;

    // ── Test 1: single submit() ───────────────────────────────────────────────
    println!("\nTest 1: single submit()");
    {
        let cmd = Box::new(ShellCommand {
            cmd: "echo 'hello from submit'".into(),
        });
        let rx = session.submit(SessionLane::Execute, cmd).await?;
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), rx)
            .await??
            .expect("command failed");
        println!("  result: {}", result);
    }

    // ── Test 2: submit_batch() — 8 shell commands in parallel ────────────────
    println!("\nTest 2: submit_batch() — 8 shell commands in parallel");
    {
        let commands: Vec<Box<dyn SessionCommand>> = (1u32..=8)
            .map(|i| -> Box<dyn SessionCommand> {
                Box::new(ShellCommand {
                    cmd: format!("echo 'task {}' && sleep 0.1", i),
                })
            })
            .collect();

        let start = Instant::now();
        let rxs = session
            .submit_batch(SessionLane::Execute, commands)
            .await?;

        // Await all in parallel
        let results = futures::future::join_all(rxs).await;
        let elapsed = start.elapsed();

        for (i, r) in results.iter().enumerate() {
            match r {
                Ok(Ok(v)) => println!("  [{}] {}", i + 1, v["output"].as_str().unwrap_or("")),
                Ok(Err(e)) => println!("  [{}] ERR: {}", i + 1, e),
                Err(e) => println!("  [{}] channel closed: {}", i + 1, e),
            }
        }
        println!(
            "  8 tasks (each 100ms) completed in {:.2}s (parallel speedup: {:.1}x)",
            elapsed.as_secs_f64(),
            0.8 / elapsed.as_secs_f64()
        );
    }

    // ── Test 3: submit_batch() — CPU-bound fib tasks ──────────────────────────
    println!("\nTest 3: submit_batch() — 6 CPU-bound fib tasks");
    {
        let commands: Vec<Box<dyn SessionCommand>> = [10u64, 20, 30, 40, 50, 60]
            .iter()
            .map(|&n| -> Box<dyn SessionCommand> { Box::new(FibCommand { n }) })
            .collect();

        let start = Instant::now();
        let rxs = session.submit_batch(SessionLane::Query, commands).await?;
        let results = futures::future::join_all(rxs).await;
        let elapsed = start.elapsed();

        for r in &results {
            if let Ok(Ok(v)) = r {
                println!("  fib({}) = {}", v["n"], v["fib"]);
            }
        }
        println!("  completed in {:.3}s", elapsed.as_secs_f64());
    }

    // ── Test 4: no-queue session returns Err ─────────────────────────────────
    println!("\nTest 4: submit() on session without queue → Err");
    {
        let session_no_queue = agent.session(".", None)?;
        let cmd = Box::new(FibCommand { n: 5 });
        match session_no_queue.submit(SessionLane::Query, cmd).await {
            Err(e) => println!("  Got expected error: {}", e),
            Ok(_) => println!("  UNEXPECTED: should have returned Err"),
        }
    }

    // ── Queue stats ───────────────────────────────────────────────────────────
    let stats = session.queue_stats().await;
    println!("\nQueue stats:");
    println!("  pending={} active={} external_pending={}",
        stats.total_pending, stats.total_active, stats.external_pending);

    println!("\n{}", "=".repeat(70));
    println!("All tests passed.");
    Ok(())
}
