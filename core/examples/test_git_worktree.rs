//! # Git Worktree — Parallel workspace isolation with real LLM
//!
//! Demonstrates the `git_worktree` builtin tool:
//! 1. Initialize a git repo in a temp directory
//! 2. Ask the LLM to create a worktree for a feature branch
//! 3. Ask the LLM to list worktrees
//! 4. Ask the LLM to check worktree status
//! 5. Ask the LLM to remove the worktree
//!
//! ```bash
//! cd crates/code
//! cargo run --example test_git_worktree
//! ```

use a3s_code_core::Agent;
use std::path::PathBuf;
use std::process::Command;

fn find_config() -> PathBuf {
    if let Ok(p) = std::env::var("A3S_CONFIG") {
        return PathBuf::from(p);
    }
    let home = dirs::home_dir().expect("no home dir");
    let home_cfg = home.join(".a3s/config.hcl");
    if home_cfg.exists() {
        return home_cfg;
    }
    let project_cfg = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.a3s/config.hcl");
    if project_cfg.exists() {
        return project_cfg;
    }
    panic!("Config not found. Create ~/.a3s/config.hcl or set A3S_CONFIG");
}

/// Initialize a git repo with one commit in the given directory.
fn init_git_repo(path: &std::path::Path) {
    Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .expect("git init failed");
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .output()
        .expect("git config email failed");
    Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(path)
        .output()
        .expect("git config name failed");
    // Create a file so the repo isn't empty
    std::fs::write(path.join("README.md"), "# Test Repo\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .output()
        .expect("git add failed");
    Command::new("git")
        .args(["commit", "-m", "Initial commit"])
        .current_dir(path)
        .output()
        .expect("git commit failed");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("a3s_code_core=info")
        .init();

    let config = find_config();
    println!("Config: {}", config.display());

    let agent = Agent::new(config.to_str().unwrap()).await?;
    println!("Agent created ✓");

    // Create a temp workspace with a git repo
    let workspace = tempfile::tempdir()?;
    init_git_repo(workspace.path());
    println!("Git repo initialized at: {}\n", workspace.path().display());

    let session = agent.session(workspace.path().to_str().unwrap(), None)?;

    // --- Test 1: Direct tool call — git_worktree status ---
    println!("═══ Test 1: git_worktree status (direct tool call) ═══");
    let result = session
        .tool("git_worktree", serde_json::json!({"command": "status"}))
        .await?;
    println!("{}\n", result.output);
    assert!(result.exit_code == 0, "status should succeed");

    // --- Test 2: Direct tool call — create worktree ---
    println!("═══ Test 2: git_worktree create (direct tool call) ═══");
    let wt_path = workspace.path().join("wt-feature-auth");
    let result = session
        .tool(
            "git_worktree",
            serde_json::json!({
                "command": "create",
                "branch": "feature-auth",
                "path": wt_path.to_str().unwrap(),
            }),
        )
        .await?;
    println!("{}\n", result.output);
    assert!(result.exit_code == 0, "create should succeed");
    assert!(wt_path.exists(), "worktree directory should exist");

    // --- Test 3: Direct tool call — list worktrees ---
    println!("═══ Test 3: git_worktree list (direct tool call) ═══");
    let result = session
        .tool("git_worktree", serde_json::json!({"command": "list"}))
        .await?;
    println!("{}\n", result.output);
    assert!(result.exit_code == 0);
    assert!(
        result.output.contains("feature-auth"),
        "list should contain the new branch"
    );

    // --- Test 4: LLM-driven — ask the agent to check worktree status ---
    println!("═══ Test 4: LLM-driven worktree query ═══");
    let result = session
        .send(
            "Use the git_worktree tool with command 'list' to show me all worktrees in this repo. Just show the tool output.",
            None,
        )
        .await?;
    println!("LLM response:\n{}\n", result.text);
    assert!(
        result.tool_calls_count > 0,
        "LLM should have called git_worktree"
    );

    // --- Test 5: Direct tool call — remove worktree ---
    println!("═══ Test 5: git_worktree remove (direct tool call) ═══");
    let result = session
        .tool(
            "git_worktree",
            serde_json::json!({
                "command": "remove",
                "path": wt_path.to_str().unwrap(),
            }),
        )
        .await?;
    println!("{}\n", result.output);
    assert!(result.exit_code == 0, "remove should succeed");
    assert!(!wt_path.exists(), "worktree directory should be gone");

    // --- Test 6: Verify list shows only main worktree ---
    println!("═══ Test 6: Verify cleanup ═══");
    let result = session
        .tool("git_worktree", serde_json::json!({"command": "list"}))
        .await?;
    println!("{}\n", result.output);
    assert!(!result.output.contains("feature-auth"));

    println!("═══ All git_worktree tests passed ✓ ═══");
    Ok(())
}
