//! S-RO-01: a run that fills twenty event pages must keep each projected page
//! inside the page bound, and retention must delete until the retained log is
//! inside the byte cap. The cursor walk reconstructs only the retained window.

use a3s_code_core::{AgentEvent, InMemoryRunStore};

#[tokio::test]
#[ignore = "S-RO-01 soak: twenty event pages stay bounded after retention"]
async fn soak_event_log_retention_deletes_after_twenty_pages() {
    const PAGE_LIMIT: usize = 5;
    const PAGES: usize = 20;
    let probe = InMemoryRunStore::new();
    let probe_run = probe.create_run("s-ro-01", "probe").await;
    probe
        .record_event(
            &probe_run.id,
            AgentEvent::TextDelta {
                text: "event-0000".to_string(),
            },
        )
        .await;
    let one = serde_json::to_vec(&probe.events(&probe_run.id).await[0]).expect("encode");
    let cap = one.len() * PAGE_LIMIT * 2;

    let store = InMemoryRunStore::with_retention_limits(None, None, Some(cap));
    let run = store.create_run("s-ro-01", "project").await;
    for index in 0..(PAGES * PAGE_LIMIT) {
        store
            .record_event(
                &run.id,
                AgentEvent::TextDelta {
                    text: format!("event-{index:04}"),
                },
            )
            .await;
    }

    let snapshot = store.snapshot(&run.id).await.expect("run");
    assert_eq!(snapshot.event_count, PAGES * PAGE_LIMIT);

    let retained = store.events(&run.id).await;
    assert!(
        retained.len() < snapshot.event_count,
        "retention did not delete"
    );
    let retained_bytes: usize = retained
        .iter()
        .map(|event| serde_json::to_vec(event).expect("encode").len())
        .sum();
    assert!(
        retained_bytes <= cap,
        "retained log {retained_bytes} exceeds cap {cap}"
    );

    let mut cursor = None;
    let mut walked = Vec::new();
    let mut pages = 0usize;
    loop {
        let page = store
            .event_page(&run.id, cursor, PAGE_LIMIT)
            .await
            .expect("known run");
        assert!(
            page.events.len() <= PAGE_LIMIT,
            "page {} has {} events",
            pages,
            page.events.len()
        );
        if pages == 0 {
            assert!(page.retention_gap, "deleted prefix was not reported");
        }
        walked.extend(page.events.iter().map(|event| event.sequence));
        pages += 1;
        if !page.has_more {
            break;
        }
        cursor = page.next_after_sequence;
        assert!(
            pages <= PAGES,
            "projection exceeded the appended page count"
        );
    }

    assert!(walked.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        walked,
        retained
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>()
    );
    assert!(pages < PAGES, "retention left the full twenty pages");
}
