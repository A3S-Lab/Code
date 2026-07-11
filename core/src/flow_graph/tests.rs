use super::*;
use a3s_flow::{RetryPolicy, WorkflowSpec};
use chrono::Utc;
use uuid::Uuid;

fn envelope(run_id: &str, sequence: u64, event: FlowEvent) -> FlowEventEnvelope {
    FlowEventEnvelope {
        run_id: run_id.to_string(),
        sequence,
        event_id: Uuid::new_v4(),
        timestamp: Utc::now(),
        event,
    }
}

#[tokio::test]
async fn projects_run_and_step_without_becoming_the_scheduler() {
    let runtime = Arc::new(Mutex::new(GraphRuntime::new()));
    let observer = FlowGraphObserver::new(Arc::clone(&runtime));
    observer
        .project(envelope(
            "run-1",
            1,
            FlowEvent::RunCreated {
                spec: WorkflowSpec::rust_embedded("test", "1", "test", "run"),
                input: json!({"goal": "ship"}),
            },
        ))
        .await
        .unwrap();
    observer
        .project(envelope("run-1", 2, FlowEvent::RunStarted))
        .await
        .unwrap();
    observer
        .project(envelope(
            "run-1",
            3,
            FlowEvent::StepCreated {
                step_id: "build".to_string(),
                step_name: "shell".to_string(),
                input: json!({"command": "cargo test"}),
                retry: RetryPolicy::default(),
            },
        ))
        .await
        .unwrap();
    observer
        .project(envelope(
            "run-1",
            4,
            FlowEvent::StepStarted {
                step_id: "build".to_string(),
                attempt: 1,
            },
        ))
        .await
        .unwrap();

    let runtime = runtime.lock().await;
    let run = runtime.graph().object(&run_object_id("run-1")).unwrap();
    let step = runtime
        .graph()
        .object(&step_object_id("run-1", "build"))
        .unwrap();
    assert_eq!(run.data["last_sequence"], 4);
    assert_eq!(step.data["status"], "running");
    assert_eq!(step.data["attempt"], 1);
    assert_eq!(runtime.graph().relations_from(&run.id).count(), 1);
}

#[tokio::test]
async fn duplicate_is_idempotent_after_restore_and_gaps_are_rejected() {
    let runtime = Arc::new(Mutex::new(GraphRuntime::new()));
    let observer = FlowGraphObserver::new(Arc::clone(&runtime));
    let created = envelope(
        "run-2",
        1,
        FlowEvent::RunCreated {
            spec: WorkflowSpec::rust_embedded("test", "1", "test", "run"),
            input: json!({}),
        },
    );
    assert_eq!(
        observer.project(created.clone()).await.unwrap(),
        ExternalProjectionOutcome::Applied
    );
    let records = runtime.lock().await.events().to_vec();
    let restored = Arc::new(Mutex::new(GraphRuntime::restore(records).unwrap()));
    let observer = FlowGraphObserver::new(restored);
    assert_eq!(
        observer.project(created).await.unwrap(),
        ExternalProjectionOutcome::Duplicate
    );
    let error = observer
        .project(envelope("run-2", 3, FlowEvent::RunStarted))
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        RuntimeError::ExternalSequenceDiverged {
            expected: 2,
            actual: 3,
            ..
        }
    ));
}

#[tokio::test]
async fn observer_reports_projection_failure_without_panicking() {
    let observer = FlowGraphObserver::new(Arc::new(Mutex::new(GraphRuntime::new())));
    observer
        .observe(envelope("missing", 2, FlowEvent::RunStarted))
        .await;
    assert!(observer.last_error().await.is_some());
}

#[tokio::test]
async fn projects_wait_hook_retry_failure_and_terminal_run_state() {
    let runtime = Arc::new(Mutex::new(GraphRuntime::new()));
    let observer = FlowGraphObserver::new(Arc::clone(&runtime));
    let events = vec![
        FlowEvent::RunCreated {
            spec: WorkflowSpec::rust_embedded("test", "1", "test", "run"),
            input: json!({}),
        },
        FlowEvent::RunStarted,
        FlowEvent::StepCreated {
            step_id: "step".into(),
            step_name: "tool".into(),
            input: json!({}),
            retry: RetryPolicy::default(),
        },
        FlowEvent::StepStarted {
            step_id: "step".into(),
            attempt: 1,
        },
        FlowEvent::StepRetrying {
            step_id: "step".into(),
            attempt: 1,
            error: "transient".into(),
            retry_after: None,
        },
        FlowEvent::StepFailed {
            step_id: "step".into(),
            attempt: 2,
            error: "permanent".into(),
        },
        FlowEvent::WaitCreated {
            wait_id: "wait".into(),
            resume_at: Utc::now(),
        },
        FlowEvent::WaitCompleted {
            wait_id: "wait".into(),
        },
        FlowEvent::HookCreated {
            hook_id: "hook".into(),
            token: "secret-must-not-project".into(),
            metadata: json!({"kind": "approval"}),
        },
        FlowEvent::HookReceived {
            hook_id: "hook".into(),
            payload: json!({"approved": true}),
        },
        FlowEvent::HookDisposed {
            hook_id: "hook".into(),
        },
        FlowEvent::RunFailed {
            error: "step failed".into(),
        },
    ];
    for (index, event) in events.into_iter().enumerate() {
        observer
            .project(envelope("run-all", index as u64 + 1, event))
            .await
            .unwrap();
    }
    let runtime = runtime.lock().await;
    assert_eq!(
        runtime
            .graph()
            .object(&run_object_id("run-all"))
            .unwrap()
            .data["status"],
        "failed"
    );
    assert_eq!(
        runtime
            .graph()
            .object(&step_object_id("run-all", "step"))
            .unwrap()
            .data["attempt"],
        2
    );
    let hook = runtime
        .graph()
        .object(&subject_object_id("run-all", "hook", "hook"))
        .unwrap();
    assert_eq!(hook.data["status"], "disposed");
    assert!(!hook.data.to_string().contains("secret-must-not-project"));
    assert!(!runtime
        .events()
        .iter()
        .any(|record| serde_json::to_string(record)
            .unwrap()
            .contains("secret-must-not-project")));
    assert_eq!(
        runtime
            .graph()
            .relations_from(&run_object_id("run-all"))
            .count(),
        3
    );
}

#[tokio::test]
async fn catch_up_orders_multiple_runs_and_is_idempotent() {
    let observer = FlowGraphObserver::new(Arc::new(Mutex::new(GraphRuntime::new())));
    let run_a = envelope(
        "a",
        1,
        FlowEvent::RunCreated {
            spec: WorkflowSpec::rust_embedded("a", "1", "test", "run"),
            input: json!({}),
        },
    );
    let run_b = envelope(
        "b",
        1,
        FlowEvent::RunCreated {
            spec: WorkflowSpec::rust_embedded("b", "1", "test", "run"),
            input: json!({}),
        },
    );
    let started_a = envelope("a", 2, FlowEvent::RunStarted);
    assert_eq!(
        observer
            .catch_up(vec![started_a.clone(), run_b.clone(), run_a.clone()])
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        observer
            .catch_up(vec![run_a, started_a, run_b])
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn concurrent_independent_flow_streams_are_serialized_safely() {
    let observer = FlowGraphObserver::new(Arc::new(Mutex::new(GraphRuntime::new())));
    let left = observer.clone();
    let right = observer.clone();
    let (left_result, right_result) = tokio::join!(
        left.project(envelope(
            "left",
            1,
            FlowEvent::RunCreated {
                spec: WorkflowSpec::rust_embedded("left", "1", "test", "run"),
                input: json!({})
            }
        )),
        right.project(envelope(
            "right",
            1,
            FlowEvent::RunCreated {
                spec: WorkflowSpec::rust_embedded("right", "1", "test", "run"),
                input: json!({})
            }
        ))
    );
    assert_eq!(left_result.unwrap(), ExternalProjectionOutcome::Applied);
    assert_eq!(right_result.unwrap(), ExternalProjectionOutcome::Applied);
}
