//! S-AP-01: a cancelled run stays terminal when a later start is recorded.

use super::{InMemoryRunStore, RunStatus};
use crate::agent::AgentEvent;

const CYCLES: usize = 30;

#[tokio::test]
#[ignore = "S-AP-01 soak: 30 start/tool/cancel/recover cycles"]
async fn soak_cancelled_runs_stay_terminal_and_do_not_leak_sessions() {
    let store = InMemoryRunStore::new();

    for cycle in 0..CYCLES {
        let run = store
            .create_run("protocol-soak", &format!("cycle-{cycle}"))
            .await;
        let started = store
            .record_event(
                &run.id,
                AgentEvent::Start {
                    prompt: format!("start-{cycle}"),
                },
            )
            .await
            .expect("start is recorded");
        assert_eq!(started.status, RunStatus::Executing);
        let tooled = store
            .record_event(
                &run.id,
                AgentEvent::ToolExecutionStart {
                    id: format!("tool-{cycle}"),
                    name: "read".into(),
                    args: serde_json::json!({ "path": "fixture.txt" }),
                },
            )
            .await
            .expect("tool start is recorded");
        assert!(
            tooled.event_count > started.event_count,
            "cycle {cycle} tool event did not advance the sequence"
        );

        let cancelled = store.mark_cancelled(&run.id).await.expect("cancel");
        assert_eq!(cancelled.status, RunStatus::Cancelled);

        let recovered = store
            .record_event(
                &run.id,
                AgentEvent::Start {
                    prompt: format!("recover-{cycle}"),
                },
            )
            .await
            .expect("recovery event is retained for replay");
        assert_eq!(
            recovered.status,
            RunStatus::Cancelled,
            "cycle {cycle} returned a cancelled run to a live status"
        );
        assert!(recovered.status.is_terminal());

        let events = store.events(&run.id).await;
        assert!(events.len() >= 3, "cycle {cycle} dropped an event");
        for pair in events.windows(2) {
            assert!(
                pair[1].sequence > pair[0].sequence,
                "cycle {cycle} event sequence regressed: {} then {}",
                pair[0].sequence,
                pair[1].sequence
            );
        }
    }
}
