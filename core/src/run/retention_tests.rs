use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exact_run_reservation_is_atomic_and_never_replaces_the_winner() {
    let store = Arc::new(InMemoryRunStore::new());
    let mut reservations = Vec::new();
    for index in 0..32 {
        let store = Arc::clone(&store);
        reservations.push(tokio::spawn(async move {
            store
                .reserve_run_with_id(
                    "run-cloud-1".to_string(),
                    "session-cloud-1",
                    &format!("prompt-{index}"),
                )
                .await
        }));
    }

    let mut created = 0;
    for reservation in reservations {
        if !reservation.await.unwrap().replayed() {
            created += 1;
        }
    }
    assert_eq!(created, 1);

    let winner = store.snapshot("run-cloud-1").await.unwrap();
    let replay = store
        .reserve_run_with_id(
            "run-cloud-1".to_string(),
            "another-session",
            "replacement prompt",
        )
        .await;
    assert!(replay.replayed());
    assert_eq!(replay.snapshot().session_id, winner.session_id);
    assert_eq!(replay.snapshot().prompt, winner.prompt);
    assert_eq!(store.list().await.len(), 1);
}

#[tokio::test]
async fn compatibility_create_replacement_keeps_one_fifo_entry() {
    let store = InMemoryRunStore::with_retention(Some(2), None);
    let original = store
        .create_run_with_id("run-replaced".to_owned(), "session-1", "first")
        .await;
    let replacement = store
        .create_run_with_id("run-replaced".to_owned(), "session-1", "second")
        .await;

    assert_eq!(replacement.id, original.id);
    assert_eq!(store.list().await.len(), 1);
    assert_eq!(store.records().await.len(), 1);
    assert_eq!(
        store.snapshot("run-replaced").await.unwrap().prompt,
        "second"
    );
}

#[tokio::test]
async fn event_sequence_overflow_does_not_publish_a_partial_event() {
    let store = InMemoryRunStore::new();
    let run = store.create_run("session-1", "overflow").await;
    store
        .runs
        .write()
        .await
        .get_mut(&run.id)
        .unwrap()
        .event_count = usize::MAX;

    let recorded = store
        .record_event(
            &run.id,
            AgentEvent::TextDelta {
                text: "must not publish".to_owned(),
            },
        )
        .await;

    assert!(recorded.is_none());
    assert!(store.events(&run.id).await.is_empty());
    assert_eq!(
        store.snapshot(&run.id).await.unwrap().event_count,
        usize::MAX
    );
}

#[tokio::test]
async fn workspace_change_set_is_terminal_and_immutable() {
    let store = InMemoryRunStore::new();
    let run = store.create_run("session-1", "change the workspace").await;
    let evidence = RunWorkspaceChangeSet {
        base_tree: format!("git-tree:{}", "1".repeat(40)),
        result_tree: format!("git-tree:{}", "2".repeat(40)),
        patch_digest: format!("sha256:{}", "3".repeat(64)),
        patch_bytes: 0,
        patch_base64: String::new(),
        observed_at_ms: 1,
    };

    assert!(matches!(
        store
            .record_workspace_change_set(&run.id, evidence.clone())
            .await,
        Err(RunWorkspaceChangeSetError::RunNotTerminal)
    ));
    store.mark_failed(&run.id, "fixture failure").await.unwrap();
    assert_eq!(
        store
            .record_workspace_change_set(&run.id, evidence.clone())
            .await
            .unwrap()
            .workspace_change_set,
        Some(evidence.clone())
    );
    store
        .record_workspace_change_set(&run.id, evidence.clone())
        .await
        .expect("exact evidence replay is idempotent");

    let mut conflict = evidence;
    conflict.result_tree = format!("git-tree:{}", "4".repeat(40));
    assert!(matches!(
        store.record_workspace_change_set(&run.id, conflict).await,
        Err(RunWorkspaceChangeSetError::Conflict)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_create_and_record_under_cap_does_not_deadlock() {
    // Guards the canonical lock-ordering change in create_run_with_id
    // (order -> events -> runs held together). A bad ordering would
    // ABBA-deadlock against concurrent record_event and hang this test.
    let store = std::sync::Arc::new(InMemoryRunStore::with_retention(Some(10), None));
    let mut handles = Vec::new();
    for i in 0..100 {
        let s = std::sync::Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            let r = s.create_run("sess", &format!("p{i}")).await;
            for _ in 0..5 {
                s.record_event(
                    &r.id,
                    AgentEvent::TextDelta {
                        text: "x".to_string(),
                    },
                )
                .await;
            }
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    // Cap honored under concurrent load, and the store is still usable
    // (no deadlock, no poisoned locks).
    assert!(store.list().await.len() <= 10);
}

#[tokio::test]
async fn replace_records_preserves_cumulative_event_count_after_trim() {
    // Source store with a small per-run event cap.
    let src = InMemoryRunStore::with_retention(None, Some(3));
    let run = src.create_run("s", "p").await;
    for _ in 0..10 {
        src.record_event(
            &run.id,
            AgentEvent::TextDelta {
                text: "x".to_string(),
            },
        )
        .await;
    }
    let records = src.records().await;
    // Buffer trimmed to cap, but cumulative event_count is the total.
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].events.len(), 3, "buffer trimmed to cap");
    assert_eq!(records[0].snapshot.event_count, 10, "cumulative preserved");

    // Round-trip into a fresh store via replace_records.
    let dst = InMemoryRunStore::new();
    dst.replace_records(records).await;
    let restored = dst.snapshot(&run.id).await.unwrap();
    assert_eq!(
        restored.event_count, 10,
        "replace_records must NOT reset event_count to the trimmed buffer length"
    );
    // The (trimmed) event buffer still round-trips at cap size.
    assert_eq!(dst.events(&run.id).await.len(), 3);
}

#[tokio::test]
async fn replace_records_enforces_run_and_event_caps() {
    let source = InMemoryRunStore::new();
    for run_index in 0..4 {
        let run = source
            .create_run_with_id(
                format!("run-{run_index}"),
                "session-1",
                &format!("prompt-{run_index}"),
            )
            .await;
        for event_index in 0..5 {
            source
                .record_event(
                    &run.id,
                    AgentEvent::TextDelta {
                        text: format!("{run_index}:{event_index}"),
                    },
                )
                .await;
        }
    }

    let restored = InMemoryRunStore::with_retention(Some(2), Some(2));
    restored.replace_records(source.records().await).await;

    let records = restored.records().await;
    assert_eq!(
        records
            .iter()
            .map(|record| record.snapshot.id.as_str())
            .collect::<Vec<_>>(),
        vec!["run-2", "run-3"],
        "restore must keep the newest runs under the same FIFO policy as live writes"
    );
    for (run_index, record) in records.iter().enumerate() {
        assert_eq!(record.snapshot.event_count, 5);
        assert_eq!(record.events.len(), 2);
        assert_eq!(record.events[0].sequence, 3);
        assert_eq!(record.events[1].sequence, 4);
        assert_eq!(record.snapshot.id, format!("run-{}", run_index + 2));
    }
}

#[tokio::test]
async fn replace_records_honors_zero_caps() {
    let source = InMemoryRunStore::new();
    let run = source.create_run("session-1", "prompt").await;
    source
        .record_event(
            &run.id,
            AgentEvent::TextDelta {
                text: "event".to_string(),
            },
        )
        .await;

    let no_runs = InMemoryRunStore::with_retention(Some(0), Some(0));
    no_runs.replace_records(source.records().await).await;
    assert!(no_runs.records().await.is_empty());

    let no_events = InMemoryRunStore::with_retention(None, Some(0));
    no_events.replace_records(source.records().await).await;
    let records = no_events.records().await;
    assert_eq!(records.len(), 1);
    assert!(records[0].events.is_empty());
    assert_eq!(records[0].snapshot.event_count, 1);
}

#[tokio::test]
async fn max_runs_evicts_oldest() {
    let store = InMemoryRunStore::with_retention(Some(2), None);
    let _ = store.create_run("session-1", "prompt-1").await;
    let r2 = store.create_run("session-1", "prompt-2").await;
    let r3 = store.create_run("session-1", "prompt-3").await;

    // Oldest run (prompt-1) must have been evicted.
    assert_eq!(store.list().await.len(), 2);
    let ids: Vec<String> = store.list().await.into_iter().map(|r| r.id).collect();
    assert!(ids.contains(&r2.id));
    assert!(ids.contains(&r3.id));
    assert!(store.events(&r2.id).await.is_empty());
    // The evicted run's events are gone too.
    let surviving_event_count: usize =
        store.events(&r2.id).await.len() + store.events(&r3.id).await.len();
    assert_eq!(surviving_event_count, 0);
}

#[tokio::test]
async fn max_events_per_run_caps_event_buffer() {
    let store = InMemoryRunStore::with_retention(None, Some(3));
    let run = store.create_run("session-1", "prompt").await;
    for _ in 0..10 {
        store
            .record_event(
                &run.id,
                AgentEvent::TextDelta {
                    text: "x".to_string(),
                },
            )
            .await;
    }
    let events = store.events(&run.id).await;
    assert_eq!(
        events.len(),
        3,
        "buffer must be capped at max_events_per_run"
    );
    // Snapshot `event_count` reflects the cumulative total, not the
    // surviving buffer length.
    let snap = store.snapshot(&run.id).await.unwrap();
    assert_eq!(snap.event_count, 10);
}

#[tokio::test]
async fn max_event_bytes_per_run_drops_oversized_live_event_but_advances_cursor() {
    let store = InMemoryRunStore::with_retention_limits(None, None, Some(0));
    let run = store.create_run("session-1", "prompt").await;

    store
        .record_event(
            &run.id,
            AgentEvent::TextDelta {
                text: "oversized".to_string(),
            },
        )
        .await;

    assert!(store.events(&run.id).await.is_empty());
    let snapshot = store.snapshot(&run.id).await.unwrap();
    assert_eq!(snapshot.event_count, 1);
}

#[tokio::test]
async fn replace_records_enforces_serialized_event_byte_cap_fifo() {
    let source = InMemoryRunStore::new();
    let run = source.create_run("session-1", "prompt").await;
    for text in ["old", "middle", "new"] {
        source
            .record_event(
                &run.id,
                AgentEvent::TextDelta {
                    text: text.to_string(),
                },
            )
            .await;
    }
    let source_records = source.records().await;
    let retained_bytes = source_records[0].events[1..]
        .iter()
        .map(serialized_event_record_len)
        .sum();

    let restored = InMemoryRunStore::with_retention_limits(None, None, Some(retained_bytes));
    restored.replace_records(source_records).await;

    let records = restored.records().await;
    assert_eq!(records[0].snapshot.event_count, 3);
    assert_eq!(
        records[0]
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[tokio::test]
async fn retained_event_sequences_remain_monotonic_after_fifo_trim() {
    let store = InMemoryRunStore::with_retention(None, Some(3));
    let run = store.create_run("session-1", "prompt").await;

    for index in 0..10 {
        store
            .record_event(
                &run.id,
                AgentEvent::TextDelta {
                    text: index.to_string(),
                },
            )
            .await;
    }

    let sequences = store
        .events(&run.id)
        .await
        .into_iter()
        .map(|record| record.sequence)
        .collect::<Vec<_>>();
    assert_eq!(sequences, vec![7, 8, 9]);
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
}

#[tokio::test]
async fn restored_run_continues_sequence_from_cumulative_event_count() {
    let source = InMemoryRunStore::with_retention(None, Some(3));
    let run = source.create_run("session-1", "prompt").await;
    for index in 0..10 {
        source
            .record_event(
                &run.id,
                AgentEvent::TextDelta {
                    text: index.to_string(),
                },
            )
            .await;
    }

    let restored = InMemoryRunStore::with_retention(None, Some(3));
    restored.replace_records(source.records().await).await;
    restored
        .record_event(
            &run.id,
            AgentEvent::TextDelta {
                text: "after restore".to_string(),
            },
        )
        .await;

    let sequences = restored
        .events(&run.id)
        .await
        .into_iter()
        .map(|record| record.sequence)
        .collect::<Vec<_>>();
    assert_eq!(sequences, vec![8, 9, 10]);
    assert_eq!(restored.snapshot(&run.id).await.unwrap().event_count, 11);
}

#[tokio::test]
async fn event_page_reports_retention_gap_and_paginates_from_cursor() {
    let store = InMemoryRunStore::with_retention(None, Some(3));
    let run = store.create_run("session-1", "prompt").await;
    for index in 0..6 {
        store
            .record_event(
                &run.id,
                AgentEvent::TextDelta {
                    text: index.to_string(),
                },
            )
            .await;
    }

    let first = store.event_page(&run.id, None, 2).await.unwrap();
    assert_eq!(first.first_available_sequence, Some(3));
    assert_eq!(first.latest_sequence_exclusive, 6);
    assert!(first.retention_gap);
    assert!(first.has_more);
    assert_eq!(first.next_after_sequence, Some(4));
    assert_eq!(
        first
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![3, 4]
    );

    let second = store
        .event_page(&run.id, first.next_after_sequence, 2)
        .await
        .unwrap();
    assert!(!second.retention_gap);
    assert!(!second.has_more);
    assert_eq!(second.next_after_sequence, Some(5));
    assert_eq!(second.events[0].sequence, 5);
    assert!(store.event_page("missing", None, 10).await.is_none());
}

#[tokio::test]
async fn event_page_reports_gap_when_retention_keeps_no_events() {
    let store = InMemoryRunStore::with_retention(None, Some(0));
    let run = store.create_run("session-1", "prompt").await;
    store
        .record_event(
            &run.id,
            AgentEvent::TextDelta {
                text: "gone".to_string(),
            },
        )
        .await;

    let page = store.event_page(&run.id, None, 10).await.unwrap();
    assert!(page.events.is_empty());
    assert_eq!(page.first_available_sequence, None);
    assert_eq!(page.latest_sequence_exclusive, 1);
    assert!(page.retention_gap);
    assert!(!page.has_more);
}

#[tokio::test]
async fn unlimited_retention_is_the_default() {
    let store = InMemoryRunStore::new();
    for i in 0..50 {
        let r = store.create_run("s", &format!("p{i}")).await;
        for _ in 0..20 {
            store
                .record_event(
                    &r.id,
                    AgentEvent::TextDelta {
                        text: "y".to_string(),
                    },
                )
                .await;
        }
    }
    assert_eq!(store.list().await.len(), 50);
}
