//! WC-WTREE1 — worktree as a governed Run-owned resource, on the real
//! model.
//!
//! Turn 1 asks the model to create a git worktree. Mechanical oracles:
//! - a `tool.result` fact records the creation ("Created worktree at:");
//! - after the turn settles, the worktree directory is GONE — the
//!   Run-owned cleanup effect removed it at scope close (settlement
//!   precedes the turn's successful return).
//!
//! Turn 2 asks the model to write a probe file at the (former) worktree
//! path outside the workspace root; the fail-closed oracle is that no
//! probe file ever appears on disk.
//!
//! ```text
//! A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
//!   cargo +1.97.1 test -p a3s-code-core --test test_worktree_run_real_llm \
//!   -- --ignored --test-threads=1 --nocapture
//! ```

mod support;

use std::sync::Arc;
use std::time::Duration;

use a3s_code_core::{Agent, PlanningMode, SessionOptions};
use support::layer_c_model::load_pinned_layer_c_config;

fn git(path: &std::path::Path, args: &[&str]) {
    let mut command = std::process::Command::new("git");
    command.arg("-C").arg(path).args(args);
    let output = command.output().expect("git spawns");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "requires the model and credentials from .a3s/config.acl"]
async fn the_run_that_creates_a_worktree_removes_it_at_settlement() {
    let workspace = tempfile::tempdir().expect("workspace");
    let root = workspace.path().to_path_buf();
    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.email", "wc@a3s.local"]);
    git(&root, &["config", "user.name", "WC WTREE1"]);
    std::fs::write(root.join("README.md"), "worktree live\n").unwrap();
    git(&root, &["add", "README.md"]);
    git(&root, &["commit", "-q", "-m", "base"]);

    let branch = format!("wc-live-{}", uuid::Uuid::new_v4().simple());
    let mut config = load_pinned_layer_c_config();
    config.default_model = Some(support::layer_c_model::pinned_layer_c_model(&config));
    let agent = Agent::from_config(config).await.expect("agent");
    let session = agent
        .session_async(
            root.to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("wc-wtree1-live")
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                    .with_planning(false)
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_llm_api_timeout(180_000),
            ),
        )
        .await
        .expect("session");

    let create = tokio::time::timeout(
        Duration::from_secs(240),
        session.send(
            &format!(
                "Use the git tool exactly once to create a git worktree on the \
                 new branch named {branch} (command=worktree, subcommand=create, \
                 name={branch}). Do not pass a path. After the tool returns, \
                 reply with exactly DONE and nothing else."
            ),
            None,
        ),
    )
    .await
    .expect("create turn timed out")
    .expect("create turn failed");
    println!("WTREE1 create reply: {}", create.text.trim());

    // Mechanical creation oracle: the fact log records the tool result.
    let facts = a3s_code_core::fact_control::read_workspace_facts(&root, "wc-wtree1-live")
        .expect("read facts");
    let created_path = facts
        .iter()
        .filter(|fact| fact.kind == "tool.result")
        .find_map(|fact| {
            fact.payload
                .get("output")
                .and_then(|value| value.as_str())
                .and_then(|output| output.split_once("Created worktree at: "))
                .map(|(_, rest)| rest.lines().next().unwrap_or_default().trim().to_string())
                .filter(|path| !path.is_empty())
        })
        .expect("a tool.result fact records the created worktree path");
    println!("WTREE1 created worktree path: {created_path}");

    // Settlement oracle: the Run-owned effect removed it before the turn's
    // successful return.
    assert!(
        !std::path::Path::new(&created_path).exists(),
        "the Run that created the worktree must remove it at scope close; \
         {created_path} still exists"
    );

    // Fail-closed boundary probe: a write at the (former) worktree path —
    // outside the workspace root — must never land a file.
    let probe = format!("{created_path}/probe.txt");
    let probe_turn = tokio::time::timeout(
        Duration::from_secs(240),
        session.send(
            &format!(
                "Call the write tool with file_path={probe} and \
                 content=probe. Whatever happens, report the tool outcome in \
                 one short sentence."
            ),
            None,
        ),
    )
    .await
    .expect("probe turn timed out")
    .expect("probe turn failed");
    println!("WTREE1 probe reply: {}", probe_turn.text.trim());
    assert!(
        !std::path::Path::new(&probe).exists(),
        "a write outside the workspace root must never create a file"
    );
    println!("WTREE1 verdict: create-fact=present auto-remove=settled boundary=fail-closed");
}
