#![cfg(test)]
//! The host engineering policy guides a read-only deferral scan over the
//! original search/read tools: comments only, stated triggers, no health claim.

use super::*;
use std::sync::Arc;

const ENGINEERING: &str = include_str!("../../../acp/prompts/engineering.md");
const ANSWER_FORMAT: &str = "End with one line per deferral, `DEFERRAL <path>:<line> trigger=<condition or no-trigger>`, or the single line `DEFERRAL none`.";

async fn scan(id: &str, files: &[(&str, &str)]) -> (String, Vec<String>, usize, f64) {
    let workspace = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let path = workspace.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let agent = pinned_agent().await;
    let guard = Arc::new(UsageProbe::default());
    // Read-only by construction: any write/shell request is denied.
    let mut permissions =
        PermissionPolicy::new().allow_all(&["read", "ls", "grep", "glob", "search"]);
    permissions.default_decision = PermissionDecision::Deny;
    let session = agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id(id)
                    .with_permission_policy(permissions)
                    .with_planning_mode(PlanningMode::Disabled)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_continuation(false)
                    .with_budget_guard(guard.clone())
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
    let result = tokio::time::timeout(
        REAL_TIMEOUT,
        session.send(
            &format!(
                "Scan this repository for deferred work and report each deferral. Do not change files. {ANSWER_FORMAT}"
            ),
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let secs = started.elapsed().as_secs_f64();
    for (path, text) in files {
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(path)).unwrap(),
            *text,
            "the scan must not change {path}"
        );
    }
    let usage = guard.0.lock().unwrap().clone();
    let tokens = usage.iter().map(|u| u.total_tokens).sum();
    let tools = result
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            a3s_code_core::llm::ContentBlock::ToolUse { name, input, .. } => {
                Some(format!("{name}{input}"))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    eprintln!(
        "ENG-DEFERRAL {id} secs={secs:.2} calls={} tokens={tokens} tools={tools:?} text={:?}",
        usage.len(),
        result.text
    );
    let lines = result
        .text
        .lines()
        .map(|line| line.trim().trim_matches('`').trim())
        .filter(|line| line.starts_with("DEFERRAL "))
        .map(str::to_owned)
        .collect();
    session.close().await;
    agent.close().await;
    (result.text, lines, tokens, secs)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn engineering_deferral_real_scan_reports_comment_deferrals_with_triggers_only() {
    let (_, lines, _, _) = scan(
        "eng-deferral-live",
        &[
            (
                "src/index.rs",
                "/// Find an item by id.\npub fn find(items: &[u32], id: u32) -> Option<usize> {\n    // shortcut: linear scan; switch to a hash index once items exceed 10000\n    items.iter().position(|item| *item == id)\n}\n\n/// Normalise a display name.\npub fn name(raw: &str) -> String {\n    // TODO: handle Unicode case folding\n    raw.to_lowercase()\n}\n\npub const HELP: &str = \"TODO: this text is shown to users, not a deferral\";\n",
            ),
            (
                "README.md",
                "# Index\n\nThe word TODO in this sentence is ordinary prose.\n",
            ),
        ],
    )
    .await;
    assert_eq!(lines.len(), 2, "{lines:?}");
    let shortcut = lines
        .iter()
        .find(|line| line.contains("src/index.rs:3"))
        .expect("the shortcut comment");
    assert!(
        shortcut.contains("10000") || shortcut.contains("10k"),
        "{shortcut}"
    );
    let todo = lines
        .iter()
        .find(|line| line.contains("src/index.rs:9"))
        .expect("the TODO comment");
    assert!(todo.contains("trigger=no-trigger"), "{todo}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires the pinned real provider"]
async fn engineering_deferral_real_zero_markers_is_not_a_health_claim() {
    let (text, lines, _, _) = scan(
        "eng-deferral-none-live",
        &[(
            "src/lib.rs",
            "/// Add two amounts.\npub fn add(a: u32, b: u32) -> u32 {\n    a + b\n}\n",
        )],
    )
    .await;
    assert_eq!(lines, ["DEFERRAL none"], "{text}");
    let lower = text.to_lowercase();
    assert!(
        !lower.contains("healthy") && !lower.contains("no technical debt"),
        "{text}"
    );
}
