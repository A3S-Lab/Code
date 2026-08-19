use super::*;
use a3s_flow::{
    CancellationRequest, ChildOperationReference, RetryPolicy, WorkflowProgress, WorkflowSpec,
};
use chrono::Utc;
use uuid::Uuid;

fn envelope(run_id: &str, sequence: u64, event: FlowEvent) -> FlowEventEnvelope {
    FlowEventEnvelope::new(run_id, sequence, Uuid::new_v4(), Utc::now(), event)
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
async fn cancellation_request_projects_request_and_closes_open_work() {
    let runtime = Arc::new(Mutex::new(GraphRuntime::new()));
    let observer = FlowGraphObserver::new(Arc::clone(&runtime));
    let events = vec![
        FlowEvent::RunCreated {
            spec: WorkflowSpec::rust_embedded("cancel", "1", "test", "run"),
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
        FlowEvent::WaitCreated {
            wait_id: "wait".into(),
            resume_at: Utc::now(),
        },
        FlowEvent::HookCreated {
            hook_id: "hook".into(),
            token: "secret-must-not-project".into(),
            metadata: json!({"kind": "approval"}),
        },
        FlowEvent::RunCancellationRequested {
            request: CancellationRequest::new(Some("operator request".into())),
        },
    ];
    for (index, event) in events.into_iter().enumerate() {
        observer
            .project(envelope("cancel", index as u64 + 1, event))
            .await
            .unwrap();
    }

    let runtime = runtime.lock().await;
    let graph = runtime.graph();
    let run = graph.object(&run_object_id("cancel")).unwrap();
    assert_eq!(run.data["status"], "cancelling");
    assert_eq!(
        run.data["cancellation"]["request"]["reason"],
        "operator request"
    );
    assert_eq!(run.data["cancellation"]["sequence"], 7);
    assert!(run.data["cancellation"]["requested_at"].is_string());
    assert_eq!(
        graph
            .object(&step_object_id("cancel", "step"))
            .unwrap()
            .data["status"],
        "cancelled"
    );
    assert_eq!(
        graph
            .object(&subject_object_id("cancel", "wait", "wait"))
            .unwrap()
            .data["status"],
        "cancelled"
    );
    assert_eq!(
        graph
            .object(&subject_object_id("cancel", "hook", "hook"))
            .unwrap()
            .data["status"],
        "cancelled"
    );
}

#[tokio::test]
async fn exceptional_terminal_events_project_typed_outcomes() {
    let deadline = Utc::now();
    let cases = [
        (
            "timeout-with-reason",
            FlowEvent::RunTimedOut {
                deadline,
                reason: Some("deadline exceeded".into()),
            },
            json!({
                "type": "timed_out",
                "deadline": deadline,
                "reason": "deadline exceeded",
            }),
            "deadline exceeded".to_string(),
        ),
        (
            "timeout-without-reason",
            FlowEvent::RunTimedOut {
                deadline,
                reason: None,
            },
            json!({"type": "timed_out", "deadline": deadline, "reason": null}),
            format!("workflow timed out at {deadline}"),
        ),
        (
            "shutdown",
            FlowEvent::RunHostShutdown { reason: None },
            json!({"type": "host_shutdown", "reason": null}),
            "workflow terminated by host shutdown".to_string(),
        ),
    ];

    for (run_id, terminal_event, expected_outcome, expected_error) in cases {
        let runtime = Arc::new(Mutex::new(GraphRuntime::new()));
        let observer = FlowGraphObserver::new(Arc::clone(&runtime));
        observer
            .project(envelope(
                run_id,
                1,
                FlowEvent::RunCreated {
                    spec: WorkflowSpec::rust_embedded(run_id, "1", "test", "run"),
                    input: json!({}),
                },
            ))
            .await
            .unwrap();
        observer
            .project(envelope(run_id, 2, FlowEvent::RunStarted))
            .await
            .unwrap();
        observer
            .project(envelope(run_id, 3, terminal_event))
            .await
            .unwrap();

        let runtime = runtime.lock().await;
        let run = runtime.graph().object(&run_object_id(run_id)).unwrap();
        assert_eq!(run.data["status"], "failed");
        assert_eq!(run.data["error"], expected_error);
        assert_eq!(run.data["terminal_outcome"], expected_outcome);
    }

    let runtime = Arc::new(Mutex::new(GraphRuntime::new()));
    let observer = FlowGraphObserver::new(Arc::clone(&runtime));
    let events = vec![
        FlowEvent::RunCreated {
            spec: WorkflowSpec::rust_embedded("retry", "1", "test", "run"),
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
        FlowEvent::StepFailed {
            step_id: "step".into(),
            attempt: 1,
            error: "permanent".into(),
        },
        FlowEvent::RunRetryExhausted {
            step_id: "step".into(),
            attempt: 1,
            error: "permanent".into(),
        },
    ];
    for (index, event) in events.into_iter().enumerate() {
        observer
            .project(envelope("retry", index as u64 + 1, event))
            .await
            .unwrap();
    }
    let runtime = runtime.lock().await;
    let run = runtime.graph().object(&run_object_id("retry")).unwrap();
    assert_eq!(run.data["status"], "failed");
    assert_eq!(run.data["error"], "permanent");
    assert_eq!(
        run.data["terminal_outcome"],
        json!({
            "type": "retry_exhausted",
            "step_id": "step",
            "attempt": 1,
            "error": "permanent",
        })
    );
}

#[tokio::test]
async fn progress_and_child_operations_are_first_class_projection_subjects() {
    let runtime = Arc::new(Mutex::new(GraphRuntime::new()));
    let observer = FlowGraphObserver::new(Arc::clone(&runtime));
    let events = vec![
        FlowEvent::RunCreated {
            spec: WorkflowSpec::rust_embedded("subjects", "1", "test", "run"),
            input: json!({}),
        },
        FlowEvent::RunStarted,
        FlowEvent::RunProgressRecorded {
            progress: WorkflowProgress::new("compile", 2)
                .with_total(5)
                .with_message("compiling")
                .with_details(json!({"crate": "a3s-code-core"})),
        },
        FlowEvent::ChildOperationLinked {
            child: ChildOperationReference::new("child-1", "flow", "operation-1")
                .with_flow_run_id("child-run-1")
                .with_metadata(json!({"purpose": "delegation"})),
        },
    ];
    for (index, event) in events.into_iter().enumerate() {
        observer
            .project(envelope("subjects", index as u64 + 1, event))
            .await
            .unwrap();
    }

    let runtime = runtime.lock().await;
    let graph = runtime.graph();
    let progress = graph
        .object(&subject_object_id("subjects", "progress", "compile"))
        .unwrap();
    assert_eq!(progress.object_type, "workflow_progress");
    assert_eq!(progress.data["completed"], 2);
    assert_eq!(progress.data["total"], 5);
    assert_eq!(progress.data["message"], "compiling");
    assert_eq!(progress.data["details"]["crate"], "a3s-code-core");

    let child = graph
        .object(&subject_object_id("subjects", "child_operation", "child-1"))
        .unwrap();
    assert_eq!(child.object_type, "workflow_child_operation");
    assert_eq!(child.data["reference_id"], "child-1");
    assert_eq!(child.data["operation_id"], "operation-1");
    assert_eq!(child.data["flow_run_id"], "child-run-1");
    assert_eq!(graph.relations_from(&run_object_id("subjects")).count(), 2);
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

#[tokio::test]
async fn health_snapshot_exposes_low_cardinality_projection_outcomes() {
    let observer = FlowGraphObserver::new(Arc::new(Mutex::new(GraphRuntime::new())));
    let created = envelope(
        "health",
        1,
        FlowEvent::RunCreated {
            spec: WorkflowSpec::rust_embedded("health", "1", "test", "run"),
            input: json!({}),
        },
    );
    observer.project(created.clone()).await.unwrap();
    observer.project(created).await.unwrap();
    observer
        .observe(envelope("health", 3, FlowEvent::RunStarted))
        .await;

    let health = observer.health().await;
    assert_eq!(health.status, FlowGraphHealthStatus::Degraded);
    assert_eq!(health.attempted, 3);
    assert_eq!(health.applied, 1);
    assert_eq!(health.duplicates, 1);
    assert_eq!(health.failures, 1);
    assert_eq!(health.sequence_gaps, 1);
    assert_eq!(health.event_conflicts, 0);
    assert_eq!(health.cancellations, 0);
    assert_eq!(health.in_flight, 0);
    assert!(health.last_success_at_ms.is_some());
    assert!(health.last_failure_at_ms.is_some());
    assert!(health.last_error.unwrap().contains("expected 2"));
}

#[tokio::test]
async fn cancelled_projection_does_not_leak_in_flight_health() {
    let runtime = Arc::new(Mutex::new(GraphRuntime::new()));
    let observer = FlowGraphObserver::new(Arc::clone(&runtime));
    let lock = runtime.lock().await;
    let pending = tokio::spawn({
        let observer = observer.clone();
        async move {
            observer
                .project(envelope(
                    "cancelled",
                    1,
                    FlowEvent::RunCreated {
                        spec: WorkflowSpec::rust_embedded("cancelled", "1", "test", "run"),
                        input: json!({}),
                    },
                ))
                .await
        }
    });
    tokio::task::yield_now().await;
    assert_eq!(observer.health().await.in_flight, 1);
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    drop(lock);
    let health = observer.health().await;
    assert_eq!(health.in_flight, 0);
    assert_eq!(health.cancellations, 1);
    assert_eq!(health.failures, 1);
    assert_eq!(health.status, FlowGraphHealthStatus::Degraded);
}
