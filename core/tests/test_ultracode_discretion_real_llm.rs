//! Real-LLM integration test for ultracode discretion.
//!
//! Proves the post-fix ultracode configuration exercises judgment in BOTH
//! directions instead of unconditionally planning + fanning out every turn:
//!
//!   * a trivial greeting ("hi")        -> NO planning, NO parallel fan-out
//!   * a genuinely parallel task        -> still fans out via unified `task`
//!
//! This mirrors what the cli now sends in ultracode mode
//! (`crates/cli/src/tui/panels/model.rs::effort_session_opts`): message-gated
//! planning (`PlanningMode::Auto`) + auto-parallel delegation + goal tracking +
//! the *conditional* ULTRACODE guideline — NOT the old `PlanningMode::Enabled`
//! (plan-every-turn) plus a per-turn imperative prompt suffix, which is what made
//! ultracode explore the workspace on a bare "hi".
//!
//! `#[ignore]` — requires a live provider in `.a3s/config.acl`. Run:
//!
//!   A3S_CONFIG_FILE=/Users/roylin/code/a3s/.a3s/config.acl \
//!     cargo test -p a3s-code-core --test test_ultracode_discretion_real_llm \
//!     -- --ignored --nocapture

use std::path::PathBuf;
use std::time::Duration;

use a3s_code_core::{
    hitl::AutoApproveConfirmation, Agent, AgentEvent, CodeConfig, PlanningMode, SessionOptions,
    SystemPromptSlots,
};

/// Conditional guideline injected in ultracode (kept in sync with the cli's
/// `ULTRACODE_GUIDELINES`). The point under test is that it *grants* the workflow
/// rather than *mandating* it.
const ULTRACODE_GUIDELINES: &str = "\
[ultracode] Dynamic-workflow mode is available — you decide whether a turn needs \
it. Match the effort to the task: answer trivial or conversational input (a \
greeting, a single question, a one-step edit) directly, with no plan and no \
fan-out. When a task genuinely splits into independent branches, decompose it, \
run those branches as one `task` call with multiple independent `tasks` items \
(keep each child prompt bounded and evidence-oriented), then synthesize their \
results before continuing dependent work.";

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

async fn real_agent() -> Agent {
    let config_path = repo_config_path();
    let config = CodeConfig::from_file(&config_path)
        .unwrap_or_else(|err| panic!("failed to load {}: {err}", config_path.display()));
    Agent::from_config(config)
        .await
        .expect("agent from real config")
}

/// The ultracode SessionOptions the cli now builds (planning is message-gated).
fn ultracode_opts() -> SessionOptions {
    SessionOptions::new()
        // The production TUI supplies a confirmation provider for delegated
        // task calls.  Use the explicit test-only equivalent here so the
        // fan-out assertions exercise child execution rather than the
        // fail-closed MissingConfirmationManager path.
        .with_confirmation_manager(std::sync::Arc::new(AutoApproveConfirmation))
        .with_max_parallel_tasks(8)
        .with_auto_delegation_enabled(true)
        .with_auto_parallel_delegation(true)
        .with_manual_delegation_enabled(true)
        .with_planning_mode(PlanningMode::Auto)
        .with_goal_tracking(true)
        .with_max_tool_rounds(40)
        .with_prompt_slots(SystemPromptSlots::default().with_guidelines(ULTRACODE_GUIDELINES))
}

/// A trivial greeting must NOT trigger planning or subagent fan-out — it should
/// just be answered. This is the "explores on hi" regression.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn ultracode_trivial_greeting_does_not_plan_or_fan_out() {
    let agent = real_agent().await;
    let workspace = tempfile::tempdir().expect("temp workspace");
    // A non-trivial workspace so "exploration", if it happened, would be visible.
    std::fs::write(
        workspace.path().join("README.md"),
        "# Demo\n\nMultiple modules live here.\n",
    )
    .unwrap();
    std::fs::write(workspace.path().join("auth.rs"), "// auth module\n").unwrap();
    std::fs::write(workspace.path().join("billing.rs"), "// billing module\n").unwrap();

    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(ultracode_opts()),
        )
        .await
        .expect("session");

    let (mut rx, handle) = session.stream("hi", None).await.expect("stream starts");

    let mut planned = false;
    let mut fanned_out = false;
    let mut got_text = false;

    let _ = tokio::time::timeout(Duration::from_secs(180), async {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::PlanningStart { .. } => planned = true,
                AgentEvent::SubagentStart { .. } => fanned_out = true,
                AgentEvent::ToolExecutionStart { name, .. }
                    if name == "parallel_task" || name == "task" =>
                {
                    fanned_out = true;
                }
                AgentEvent::TextDelta { text } if !text.trim().is_empty() => got_text = true,
                AgentEvent::End { .. } => return,
                AgentEvent::Error { message } => panic!("stream error on 'hi': {message}"),
                _ => {}
            }
        }
    })
    .await;

    let _ = session.cancel().await;
    handle.abort();
    let _ = handle.await;

    assert!(
        !planned,
        "ultracode ran the planner for a bare greeting — Auto gating failed"
    );
    assert!(
        !fanned_out,
        "ultracode fanned out subagents for a bare greeting — the 'explores on hi' bug"
    );
    assert!(got_text, "expected a direct text reply to 'hi'");
}

/// A genuinely parallelizable task must STILL fan out, proving that removing the
/// imperative prompt did not disable delegation — the tool description + Auto
/// planning carry it on their own.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn ultracode_parallel_task_still_fans_out() {
    let agent = real_agent().await;
    let workspace = tempfile::tempdir().expect("temp workspace");
    std::fs::write(
        workspace.path().join("README.md"),
        "# Demo service\n\nThree unrelated modules: auth, billing, search.\n",
    )
    .unwrap();
    for (name, body) in [
        ("auth.rs", "// authentication module\npub fn login() {}\n"),
        ("billing.rs", "// billing module\npub fn charge() {}\n"),
        ("search.rs", "// search module\npub fn query() {}\n"),
    ] {
        std::fs::write(workspace.path().join(name), body).unwrap();
    }

    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(ultracode_opts()),
        )
        .await
        .expect("session");

    let prompt = "These three modules are independent. In parallel, separately inspect \
        auth.rs, billing.rs, and search.rs and give a one-line summary of each. They do \
        not depend on each other, so investigate them concurrently rather than one by one. \
        Keep each summary compact and do not modify any files.";
    let (mut rx, handle) = session.stream(prompt, None).await.expect("stream starts");

    let fanned_out = tokio::time::timeout(Duration::from_secs(300), async {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::ToolExecutionStart { name, args, .. } if name == "task" => {
                    if args
                        .get("tasks")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|tasks| tasks.len() > 1)
                    {
                        return true;
                    }
                }
                AgentEvent::SubagentStart { .. } => return true,
                AgentEvent::End { .. } => return false,
                AgentEvent::Error { message } => panic!("stream error: {message}"),
                _ => {}
            }
        }
        false
    })
    .await
    .expect("timed out waiting for the parallel task to fan out");

    let _ = session.cancel().await;
    handle.abort();
    let _ = handle.await;

    assert!(
        fanned_out,
        "ultracode did not fan out for an explicitly parallel, independent task"
    );
}

// Live end-to-end proof that the unified task delegation path works:
// the model delegates one or more children, the child result is returned to
// the parent, and the turn completes.  The preceding test is the stricter
// multi-item fan-out check.  Keeping this assertion independent of the exact
// number of items makes it robust to a provider choosing one bounded task for
// a prompt that still asks for an explicitly parallel workflow.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires real provider credentials and network access"]
async fn ultracode_delegated_task_runs_and_completes() {
    let agent = real_agent().await;
    let ws = tempfile::tempdir().unwrap();
    for (n, b) in [
        ("auth.rs", "// auth module\npub fn login() {}\n"),
        ("billing.rs", "// billing module\npub fn charge() {}\n"),
        ("search.rs", "// search module\npub fn query() {}\n"),
    ] {
        std::fs::write(ws.path().join(n), b).unwrap();
    }
    let session = agent
        .session_async(ws.path().display().to_string(), Some(ultracode_opts()))
        .await
        .unwrap();
    let prompt = "These three files are independent: auth.rs, billing.rs, search.rs. In parallel, \
        inspect each separately and give a one-line summary of each. Investigate them concurrently, \
        not one by one. Do not modify any files.";
    let (mut rx, handle) = session.stream(prompt, None).await.unwrap();

    // The reliable signals: the model invokes the unified task tool, its
    // successful ToolEnd carries the child result, and the turn completes.
    // The executor's actual multi-child concurrency is proven deterministically
    // by `parallel_task_executor_runs_children_concurrently_and_preserves_input_order`.
    let mut task_calls = 0usize;
    let mut successful_task_ends = 0usize;
    let mut merged = String::new();
    let mut reached_end = false;

    let _ = tokio::time::timeout(std::time::Duration::from_secs(360), async {
        while let Some(ev) = rx.recv().await {
            match ev {
                AgentEvent::ToolExecutionStart { name, args, .. } if name == "task" => {
                    assert!(
                        args.get("tasks")
                            .and_then(serde_json::Value::as_array)
                            .is_some_and(|tasks| !tasks.is_empty()),
                        "unified task call must contain at least one task: {args}"
                    );
                    task_calls += 1;
                }
                AgentEvent::ToolEnd {
                    name,
                    output,
                    exit_code,
                    ..
                } if name == "task" => {
                    if exit_code == 0 {
                        successful_task_ends += 1;
                    }
                    merged.push_str(&output);
                    merged.push('\n');
                }
                AgentEvent::End { .. } => {
                    reached_end = true;
                    return;
                }
                AgentEvent::Error { message } => panic!("stream error: {message}"),
                _ => {}
            }
        }
    })
    .await;

    let _ = session.cancel().await;
    handle.abort();
    let _ = handle.await;

    let first_line = merged.lines().next().unwrap_or("");
    eprintln!("TASK_DELEGATION_TRACE calls={task_calls} successful_ends={successful_task_ends} reached_end={reached_end} merged_first_line={first_line:?}");

    assert!(task_calls >= 1, "model never called the unified task tool");
    assert!(
        successful_task_ends >= 1,
        "delegated task did not complete successfully: {merged:?}"
    );
    assert!(
        !merged.trim().is_empty(),
        "delegated task returned no result"
    );
    assert!(reached_end, "turn never completed after delegation");
}
