use super::*;

fn cognitive_binding() -> crate::cognitive_context::CognitivePackageBindingV1 {
    let generation_digest =
        "sha256:aa0beeb62f1b7b21bf70f21e6f0e858a1e4b720d313f0907209b5b9dad2eeb20";
    let knowledge = crate::cognitive_context::CognitiveKnowledgeBindingV1::new(
        "domain-knowledge",
        "0.2",
        "sha256:1def786da6d190b7b3ce0176e71d99ff1cac3f8c8cc7c0f8b76a893c544e7a90",
        7,
        generation_digest,
    )
    .unwrap();
    crate::cognitive_context::CognitivePackageBindingV1::new(
        "contra-sense/handbook",
        "0.1.0",
        7,
        generation_digest,
        "sha256:1e0f0a0162f5b290887ade8886af69fbba4548c863df026178e3550c77813455",
        knowledge,
        crate::cognitive_context::CognitiveContextLimits::default(),
    )
    .unwrap()
}

#[tokio::test]
async fn run_store_tracks_status_and_events() {
    let store = InMemoryRunStore::new();
    let run = store.create_run("session-1", "fix tests").await;

    store
        .record_event(
            &run.id,
            AgentEvent::Start {
                prompt: "fix tests".to_string(),
            },
        )
        .await;
    store
        .record_event(
            &run.id,
            AgentEvent::End {
                text: "done".to_string(),
                usage: Default::default(),
                verification_summary: Box::new(
                    crate::verification::VerificationSummary::from_reports(&[]),
                ),
                completion_terminal: Default::default(),
                forced_stop: None,
                meta: None,
            },
        )
        .await;

    let snapshot = store.snapshot(&run.id).await.unwrap();
    assert_eq!(snapshot.status, RunStatus::Completed);
    assert_eq!(snapshot.result_text.as_deref(), Some("done"));
    assert_eq!(snapshot.event_count, 2);
    assert_eq!(store.events(&run.id).await.len(), 2);
}

#[tokio::test]
async fn cognitive_binding_is_exact_idempotent_and_pre_observation_only() {
    let store = InMemoryRunStore::new();
    let run = store.create_run("session-1", "query knowledge").await;
    let binding = cognitive_binding();

    let bound = store
        .bind_cognitive_package(&run.id, binding.clone())
        .await
        .unwrap();
    assert_eq!(bound.cognitive_package_binding.as_ref(), Some(&binding));
    store
        .bind_cognitive_package(&run.id, binding.clone())
        .await
        .expect("exact binding replay is idempotent");

    let mut conflict = binding.clone();
    conflict.limits.max_results -= 1;
    conflict.validate().unwrap();
    assert!(matches!(
        store.bind_cognitive_package(&run.id, conflict).await,
        Err(RunCognitiveBindingError::Conflict)
    ));

    let late = store.create_run("session-1", "late binding").await;
    store
        .record_event(
            &late.id,
            AgentEvent::Start {
                prompt: "late binding".to_owned(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        store.bind_cognitive_package(&late.id, binding).await,
        Err(RunCognitiveBindingError::AlreadyObserved)
    ));
}

#[tokio::test]
async fn event_observation_keeps_snapshot_and_page_in_one_generation() {
    let store = InMemoryRunStore::new();
    let run = store.create_run("session-1", "observe exactly").await;
    store
        .record_event(
            &run.id,
            AgentEvent::End {
                text: "done".to_string(),
                usage: Default::default(),
                verification_summary: Box::new(
                    crate::verification::VerificationSummary::from_reports(&[]),
                ),
                completion_terminal: Default::default(),
                forced_stop: None,
                meta: None,
            },
        )
        .await;

    let observation = store
        .event_observation(&run.id, None, 64)
        .await
        .expect("known run observation");

    assert_eq!(observation.snapshot.status, RunStatus::Completed);
    assert_eq!(
        observation.snapshot.event_count,
        observation.page.latest_sequence_exclusive
    );
    assert!(observation
        .page
        .events
        .iter()
        .all(|event| event.timestamp_ms <= observation.snapshot.updated_at_ms));
    assert!(store
        .event_observation("missing-run", None, 64)
        .await
        .is_none());
}

#[tokio::test]
async fn restored_logical_time_cannot_regress_new_event_observations() {
    let source = InMemoryRunStore::new();
    let run = source.create_run("session-1", "resume exactly").await;
    let failed_run = source.create_run("session-1", "fail exactly").await;
    let mut records = source.records().await;
    let persisted_time = now_ms().saturating_add(60_000);
    for record in &mut records {
        record.snapshot.updated_at_ms = persisted_time;
    }

    let restored = InMemoryRunStore::new();
    restored.replace_records(records).await;
    restored
        .record_event(
            &run.id,
            AgentEvent::TextDelta {
                text: "after recovery".to_string(),
            },
        )
        .await;

    let observation = restored
        .event_observation(&run.id, None, 64)
        .await
        .expect("restored run observation");
    assert!(observation.snapshot.updated_at_ms >= persisted_time);
    assert!(observation
        .page
        .events
        .iter()
        .all(|event| event.timestamp_ms >= persisted_time));

    let cancelled = restored
        .mark_cancelled(&run.id)
        .await
        .expect("restored run cancellation");
    assert!(cancelled.updated_at_ms >= persisted_time);
    let failed = restored
        .mark_failed(&failed_run.id, "provider failed")
        .await
        .expect("restored run failure");
    assert!(failed.updated_at_ms >= persisted_time);
}

#[tokio::test]
async fn run_store_replaces_persisted_records() {
    let source = InMemoryRunStore::new();
    let run = source.create_run("session-1", "persist").await;
    source
        .record_event(
            &run.id,
            AgentEvent::Start {
                prompt: "persist".to_string(),
            },
        )
        .await;

    let target = InMemoryRunStore::new();
    target.replace_records(source.records().await).await;

    assert_eq!(target.list().await.len(), 1);
    assert_eq!(target.events(&run.id).await.len(), 1);
    assert_eq!(target.snapshot(&run.id).await.unwrap().event_count, 1);
}

#[tokio::test]
async fn run_handle_only_cancels_current_run() {
    // Real public canonical entry mints the original handle; no test pin/context constructor.
    let fixture = super::owner_cancel_tests::public_fixture().await;
    let stream = super::owner_cancel_tests::start(&fixture).await;
    let handle = stream.run.as_ref().unwrap().clone();
    assert!(handle.cancel().await);
    super::owner_cancel_tests::finish(&fixture, stream).await;
    assert_eq!(handle.status().await, Some(RunStatus::Cancelled));

    let next = super::owner_cancel_tests::start(&fixture).await;
    assert_ne!(next.run.as_ref().unwrap().id(), handle.id());
    assert!(!handle.cancel().await);
    super::owner_cancel_tests::finish(&fixture, next).await;
}

#[tokio::test]
async fn late_events_cannot_regress_a_terminal_run_status() {
    let store = InMemoryRunStore::new();
    let cancelled = store.create_run("session-1", "cancelled").await;
    store.mark_cancelled(&cancelled.id).await;
    store
        .record_event(&cancelled.id, AgentEvent::TurnStart { turn: 2 })
        .await;
    assert_eq!(
        store.snapshot(&cancelled.id).await.unwrap().status,
        RunStatus::Cancelled
    );

    let completed = store.create_run("session-1", "completed").await;
    store
        .record_event(
            &completed.id,
            AgentEvent::End {
                text: "done".to_string(),
                usage: Default::default(),
                verification_summary: Box::new(
                    crate::verification::VerificationSummary::from_reports(&[]),
                ),
                completion_terminal: Default::default(),
                forced_stop: None,
                meta: None,
            },
        )
        .await;
    store
        .record_event(
            &completed.id,
            AgentEvent::ToolExecutionStart {
                id: "late-tool".to_string(),
                name: "bash".to_string(),
                args: serde_json::json!({}),
            },
        )
        .await;
    assert_eq!(
        store.snapshot(&completed.id).await.unwrap().status,
        RunStatus::Completed
    );
}

#[tokio::test]
async fn late_terminal_markers_cannot_rewrite_the_first_terminal_outcome() {
    let store = InMemoryRunStore::new();
    let completed = store.create_run("session-1", "done").await;
    store
        .record_event(
            &completed.id,
            AgentEvent::End {
                text: "done".to_string(),
                usage: Default::default(),
                verification_summary: Box::new(
                    crate::verification::VerificationSummary::from_reports(&[]),
                ),
                completion_terminal: Default::default(),
                forced_stop: None,
                meta: None,
            },
        )
        .await;
    assert_eq!(
        store.mark_cancelled(&completed.id).await.unwrap().status,
        RunStatus::Completed
    );
    assert_eq!(
        store
            .mark_failed(&completed.id, "late failure")
            .await
            .unwrap()
            .status,
        RunStatus::Completed
    );

    let failed = store.create_run("session-1", "failed").await;
    store.mark_failed(&failed.id, "provider failed").await;
    assert_eq!(
        store.mark_cancelled(&failed.id).await.unwrap().status,
        RunStatus::Failed
    );
}

#[tokio::test]
async fn terminal_sink_preserves_event_completion_and_rejects_late_outcomes() {
    let store = InMemoryRunStore::new();
    let completed = store.create_run("session-1", "done").await;
    store
        .record_event(
            &completed.id,
            AgentEvent::End {
                text: "done".to_string(),
                usage: Default::default(),
                verification_summary: Box::new(
                    crate::verification::VerificationSummary::from_reports(&[]),
                ),
                completion_terminal: Default::default(),
                forced_stop: None,
                meta: None,
            },
        )
        .await;

    let acknowledged = store
        .settle_terminal(&completed.id, RunTerminalTransition::Completed)
        .await
        .expect("completed Run remains observable");
    assert_eq!(acknowledged.status, RunStatus::Completed);

    let failed = store.create_run("session-1", "fails").await;
    let failed = store
        .settle_terminal(
            &failed.id,
            RunTerminalTransition::Failed("provider failed".to_string()),
        )
        .await
        .expect("failed Run remains observable");
    assert_eq!(failed.status, RunStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some("provider failed"));

    let unchanged = store
        .settle_terminal(&failed.id, RunTerminalTransition::Cancelled)
        .await
        .expect("late cancellation remains observable");
    assert_eq!(unchanged.status, RunStatus::Failed);
    assert_eq!(unchanged.error.as_deref(), Some("provider failed"));
}

// Bare declarations resolve to run/tests/<name>.rs — the slot the soak
// files already occupy (the old `#[path]` form resolved differently once
// this module left run.rs's inline block; that is the 10-08 hijack lesson).
mod run_cognitive_binding_soak;

mod run_protocol_soak;
