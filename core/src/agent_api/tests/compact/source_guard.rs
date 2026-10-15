#![cfg(test)]

use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Default)]
struct Gate {
    deny: AtomicBool,
    checks: AtomicUsize,
    pause: AtomicBool,
    entered: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl crate::budget::BudgetGuard for Gate {
    async fn check_before_llm(&self, _: &str, _: usize) -> crate::budget::BudgetDecision {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if self.pause.load(Ordering::SeqCst) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        if self.deny.load(Ordering::SeqCst) {
            crate::budget::BudgetDecision::Deny {
                resource: "summary".into(),
                reason: "summary budget refused".into(),
            }
        } else {
            crate::budget::BudgetDecision::Allow
        }
    }
}

async fn fixture(root: &std::path::Path) -> (AgentSession, Arc<RecordingCompactClient>, Arc<Gate>) {
    let client = Arc::new(RecordingCompactClient {
        reply: "ready".into(),
        summary: "## Current State\nA compact summary.".into(),
        prompts: Mutex::new(vec![]),
        streams: Mutex::new(vec![]),
    });
    let gate = Arc::new(Gate::default());
    let mut config = test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();
    let session = agent
        .build_session(
            root.to_string_lossy().to_string(),
            client.clone(),
            &SessionOptions::new()
                .with_session_id("compact-source")
                .with_planning_mode(crate::PlanningMode::Disabled)
                .with_auto_delegation_enabled(false)
                .with_continuation(false)
                .with_work_policy(
                    crate::WorkPolicyV1::new("compact-policy", 1, "Preserve exact source policy.")
                        .unwrap(),
                )
                .with_budget_guard(gate.clone()),
        )
        .unwrap();
    finish_stream(&session, "Remember the current scoped task.").await;
    finish_stream(&session, "Keep the task open.").await;
    (session, client, gate)
}

#[tokio::test]
async fn compact_bad_policy_source_refuses_before_summary_provider() {
    let root = tempfile::tempdir().unwrap();
    let (session, client, _) = fixture(root.path()).await;
    let mut facts =
        crate::fact_control::read_workspace_facts(root.path(), "compact-source").unwrap();
    facts
        .iter_mut()
        .find(|f| f.kind == "a3s.work-policy.admitted")
        .unwrap()
        .payload["digest"] = "sha256:invalid".into();
    let bytes = facts
        .iter()
        .map(|f| serde_json::to_string(f).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let path = crate::session_folder::effect_log_dir(root.path(), "compact-source")
        .join("compact-source.jsonl");
    std::fs::write(&path, &bytes).unwrap();
    let calls = client.prompts.lock().unwrap().len();
    let history = serde_json::to_value(session.history()).unwrap();
    let error = session
        .compact_conversation(None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("work_policy.corrupt"), "{error}");
    assert_eq!(client.prompts.lock().unwrap().len(), calls);
    assert_eq!(serde_json::to_value(session.history()).unwrap(), history);
    assert_eq!(std::fs::read_to_string(path).unwrap(), bytes);
    session.close().await;
}

#[tokio::test]
async fn compact_budget_refusal_does_not_call_summary_provider_or_append() {
    let root = tempfile::tempdir().unwrap();
    let (session, client, gate) = fixture(root.path()).await;
    gate.deny.store(true, Ordering::SeqCst);
    gate.checks.store(0, Ordering::SeqCst);
    let calls = client.prompts.lock().unwrap().len();
    let facts = crate::fact_control::read_workspace_facts(root.path(), "compact-source").unwrap();
    let history = serde_json::to_value(session.history()).unwrap();
    let error = session
        .compact_conversation(None)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("summary budget refused"), "{error}");
    assert_eq!(client.prompts.lock().unwrap().len(), calls);
    assert_eq!(gate.checks.load(Ordering::SeqCst), 1);
    assert_eq!(serde_json::to_value(session.history()).unwrap(), history);
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "compact-source").unwrap(),
        facts
    );
    session.close().await;
}

#[tokio::test]
async fn compact_close_cancels_budget_wait_and_retains_original_writer() {
    let root = tempfile::tempdir().unwrap();
    let (session, client, gate) = fixture(root.path()).await;
    gate.pause.store(true, Ordering::SeqCst);
    let calls = client.prompts.lock().unwrap().len();
    let facts = crate::fact_control::read_workspace_facts(root.path(), "compact-source").unwrap();
    let history = serde_json::to_value(session.history()).unwrap();
    let (compacted, ()) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(session.compact_conversation(None), async {
            gate.entered.notified().await;
            let other = a3s_effect::FileLog::open(crate::session_folder::effect_log_dir(
                root.path(),
                "compact-source",
            ))
            .unwrap();
            assert!(
                other.try_writer_fence().is_err(),
                "original writer held throughout summary"
            );
            session.close().await;
        })
    })
    .await
    .unwrap();
    assert!(compacted.is_err());
    assert_eq!(client.prompts.lock().unwrap().len(), calls);
    assert_eq!(serde_json::to_value(session.history()).unwrap(), history);
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "compact-source").unwrap(),
        facts
    );
}

#[tokio::test]
async fn compact_append_refuses_foreign_writer_and_changed_source() {
    let root = tempfile::tempdir().unwrap();
    let (session, _, _) = fixture(root.path()).await;
    let log = session
        .run_admission
        .fact_log_slot()
        .lock()
        .unwrap()
        .clone()
        .unwrap();
    let other_root = tempfile::tempdir().unwrap();
    let other = a3s_effect::FileLog::open(other_root.path()).unwrap();
    let other_writer = other.try_writer_fence().unwrap();
    let before = crate::fact_control::read_workspace_facts(root.path(), "compact-source").unwrap();
    let error = crate::fact_control::append_compaction_done(
        &log,
        &other_writer,
        "compact-source",
        "summary",
        None,
    )
    .unwrap_err();
    assert!(error.to_string().contains("original log writer"));
    {
        let writer = log.try_writer_fence().unwrap();
        let error = crate::fact_control::append_compaction_done(
            &log,
            &writer,
            "compact-source",
            "summary",
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("durable source changed"));
    }
    assert_eq!(
        crate::fact_control::read_workspace_facts(root.path(), "compact-source").unwrap(),
        before
    );
    session.close().await;
}
