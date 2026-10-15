#![cfg(test)]
//! The host engineering policy alone routes on-demand review and whole-repository
//! audit to the original `review` worker, which stays read-only.

use super::*;
use a3s_code_core::llm::TokenUsage;
use a3s_code_core::AgentEvent;
use std::sync::Arc;

const ENGINEERING: &str = include_str!("../../../acp/prompts/engineering.md");

#[derive(Default)]
struct Turn {
    task_args: Vec<serde_json::Value>,
    /// Core-resolved worker names and outcomes.
    workers: Vec<(String, bool)>,
    tools: Vec<String>,
    text: String,
}

impl Turn {
    fn reviewed(&self) -> bool {
        self.workers
            .iter()
            .any(|(agent, success)| agent == "review" && *success)
    }
}

fn write_fixture(root: &std::path::Path) {
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub mod budget;\npub mod report;\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/report.rs"),
        "/// Join invoice ids for display.\npub fn ids(ids: &[u32]) -> String {\n    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(\",\")\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/budget.rs"),
        "pub const MAX_LINES: usize = 10;\n\n/// Sum the billed amounts of at most MAX_LINES invoice lines.\npub fn billed(lines: &[u8]) -> usize {\n    let mut total = 0;\n    for index in 0..=MAX_LINES {\n        if let Some(line) = lines.get(index) {\n            total += usize::from(*line);\n        }\n    }\n    total\n}\n",
    )
    .unwrap();
}

fn snapshot(root: &std::path::Path) -> Vec<(String, String)> {
    ["src/lib.rs", "src/report.rs", "src/budget.rs"]
        .iter()
        .map(|path| {
            (
                path.to_string(),
                std::fs::read_to_string(root.join(path)).unwrap(),
            )
        })
        .collect()
}

async fn run(id: &str, prompt: &str) -> (Turn, Vec<TokenUsage>, f64) {
    let workspace = tempfile::tempdir().unwrap();
    write_fixture(workspace.path());
    let before = snapshot(workspace.path());
    let agent = pinned_agent().await;
    let guard = Arc::new(UsageProbe::default());
    let mut permissions =
        PermissionPolicy::new().allow_all(&["task", "read", "ls", "grep", "glob", "search"]);
    permissions.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id(id)
                    .with_permission_policy(permissions)
                    .with_confirmation_manager(Arc::new(
                        a3s_code_core::hitl::AutoApproveConfirmation,
                    ))
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_manual_delegation_enabled(true)
                    .with_auto_delegation_enabled(false)
                    .with_continuation(false)
                    .with_budget_guard(guard.clone())
                    .with_max_tool_rounds(8)
                    .with_llm_api_timeout(180_000)
                    .with_temperature(0.0)
                    .with_work_policy(
                        a3s_code_core::WorkPolicyV1::new("engineering-v1", 1, ENGINEERING).unwrap(),
                    ),
            ),
        )
        .await
        .unwrap();
    let started = Instant::now();
    let (mut events, worker) = session.stream(prompt, None).await.unwrap();
    let mut turn = Turn::default();
    turn.text = tokio::time::timeout(REAL_TIMEOUT, async {
        loop {
            match events.recv().await.expect("event stream open") {
                AgentEvent::ToolExecutionStart { name, args, .. } => {
                    if name == "task" {
                        turn.task_args.push(args);
                    }
                    turn.tools.push(name);
                }
                AgentEvent::SubagentEnd { agent, success, .. } => {
                    turn.workers.push((agent, success));
                }
                AgentEvent::End { text, .. } => break text,
                AgentEvent::Error { message } => panic!("{message}"),
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    worker.await.unwrap();
    let secs = started.elapsed().as_secs_f64();
    assert_eq!(
        snapshot(workspace.path()),
        before,
        "review must not change files"
    );
    let usage = guard.0.lock().unwrap().clone();
    session.close().await;
    agent.close().await;
    (turn, usage, secs)
}

fn report(label: &str, turn: &Turn, usage: &[TokenUsage], secs: f64) {
    eprintln!(
        "ENG-REVIEW {label} secs={secs:.2} calls={} tokens={} tools={:?} workers={:?} task_args={:?} text={:?}",
        usage.len(),
        usage.iter().map(|u| u.total_tokens).sum::<usize>(),
        turn.tools,
        turn.workers,
        turn.task_args,
        turn.text
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn engineering_review_real_policy_routes_to_the_read_only_review_worker() {
    let (turn, usage, secs) = run(
        "eng-review-live",
        "Do an on-demand code review of src/budget.rs and report the findings. Do not change files.",
    )
    .await;
    report("review", &turn, &usage, secs);
    assert!(turn.reviewed(), "the review worker is used");
    assert!(turn.text.contains("MAX_LINES") || turn.text.contains("0..="));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn engineering_audit_real_whole_repository_finds_the_unnamed_defect() {
    let (turn, usage, secs) = run(
        "eng-audit-live",
        "Audit the whole repository for defects and report the findings. Do not change files.",
    )
    .await;
    report("audit", &turn, &usage, secs);
    assert!(turn.reviewed(), "the review worker is used");
    assert!(turn.text.contains("budget"), "the audit locates the defect");
}
