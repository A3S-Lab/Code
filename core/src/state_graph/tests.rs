use super::*;
use serde_json::json;
use std::sync::Arc;

fn add_object(version: u64, id: &str, object_type: &str, data: serde_json::Value) -> GraphPatch {
    GraphPatch::new(
        version,
        vec![PatchOperation::AddObject {
            id: id.to_string(),
            object_type: object_type.to_string(),
            data,
        }],
    )
}

#[test]
fn event_log_is_the_source_of_truth_and_strict_replay_rebuilds_projection() {
    let mut runtime = GraphRuntime::new().with_correlation_id("trace-1");
    assert!(runtime
        .propose_patch(
            add_object(0, "task-1", "task", json!({"status": "open"})),
            None
        )
        .unwrap());

    let replayed = GraphRuntime::strict_replay(runtime.events()).unwrap();
    assert_eq!(&replayed, runtime.graph());
    assert_eq!(replayed.version(), 1);
    assert_eq!(replayed.object("task-1").unwrap().version, 1);
    assert!(runtime
        .events()
        .iter()
        .all(|record| record.correlation_id.as_deref() == Some("trace-1")));
}

#[test]
fn stale_patch_is_rejected_atomically_without_partial_mutation() {
    let mut runtime = GraphRuntime::new();
    runtime
        .propose_patch(add_object(0, "a", "task", json!({})), None)
        .unwrap();
    let stale = GraphPatch::new(
        0,
        vec![
            PatchOperation::AddObject {
                id: "b".into(),
                object_type: "task".into(),
                data: json!({}),
            },
            PatchOperation::AddObject {
                id: "c".into(),
                object_type: "task".into(),
                data: json!({}),
            },
        ],
    );

    assert!(!runtime.propose_patch(stale, None).unwrap());
    assert!(runtime.graph().object("b").is_none());
    assert!(runtime.graph().object("c").is_none());
    assert!(matches!(
        runtime.events().last().unwrap().event,
        GraphEvent::PatchRejected { .. }
    ));
}

#[test]
fn object_level_version_conflict_is_a_first_class_rejection() {
    let mut runtime = GraphRuntime::new();
    runtime
        .propose_patch(add_object(0, "a", "task", json!({"n": 1})), None)
        .unwrap();
    let patch = GraphPatch::new(
        1,
        vec![PatchOperation::UpdateObject {
            id: "a".into(),
            expected_version: 0,
            data: json!({"n": 2}),
        }],
    );
    assert!(!runtime.propose_patch(patch, None).unwrap());
    assert_eq!(runtime.graph().object("a").unwrap().data, json!({"n": 1}));
}

#[test]
fn relation_requires_existing_endpoints_and_object_removal_requires_detach() {
    let mut runtime = GraphRuntime::new();
    runtime
        .propose_patch(add_object(0, "a", "task", json!({})), None)
        .unwrap();
    let missing_endpoint = GraphPatch::new(
        1,
        vec![PatchOperation::AddRelation {
            id: "edge".into(),
            relation_type: "blocks".into(),
            source: "a".into(),
            target: "missing".into(),
            data: json!({}),
        }],
    );
    assert!(!runtime.propose_patch(missing_endpoint, None).unwrap());
    assert!(runtime.graph().relation("edge").is_none());
}

#[test]
fn behavior_reacts_to_graph_event_and_preserves_causation_chain() {
    let behavior = FnBehavior::new(
        "create-evidence",
        EventFilter::new(["object.created"]),
        |context| {
            let GraphEvent::ObjectCreated { id, .. } = &context.event.event else {
                return Ok(Vec::new());
            };
            if id != "claim-1" {
                return Ok(Vec::new());
            }
            Ok(vec![GraphPatch::new(
                context.graph.version(),
                vec![PatchOperation::AddObject {
                    id: "evidence-1".into(),
                    object_type: "evidence".into(),
                    data: json!({"claim": id}),
                }],
            )])
        },
    );
    let mut runtime = GraphRuntime::new();
    runtime.register(Arc::new(behavior)).unwrap();
    runtime
        .propose_patch(
            add_object(0, "claim-1", "claim", json!({"text": "test"})),
            None,
        )
        .unwrap();

    assert!(runtime.graph().object("evidence-1").is_some());
    let created = runtime
        .events()
        .iter()
        .find(|record| {
            matches!(&record.event, GraphEvent::ObjectCreated { id, .. } if id == "evidence-1")
        })
        .unwrap();
    assert!(created.causation_id.is_some());
    GraphRuntime::strict_replay(runtime.events()).unwrap();
}

#[test]
fn behavior_failure_is_recorded_and_does_not_escape_runtime_entrypoint() {
    let behavior = FnBehavior::new("failing", EventFilter::new(["goal.created"]), |_| {
        Err(BehaviorError::new("fixture failed"))
    });
    let mut runtime = GraphRuntime::new();
    runtime.register(Arc::new(behavior)).unwrap();
    runtime.run_goal("test").unwrap();
    assert!(runtime.events().iter().any(|record| {
        matches!(&record.event, GraphEvent::BehaviorFailed { name, error }
            if name == "failing" && error == "fixture failed")
    }));
}

#[test]
fn typed_and_graph_shape_predicates_scope_behavior_context() {
    let behavior = FnBehavior::new(
        "only-open-tasks",
        EventFilter::new(["object.created"])
            .with_object_types(["task"])
            .where_predicate(|event, _| {
                matches!(&event.event, GraphEvent::ObjectCreated { data, .. }
                    if data.get("status") == Some(&json!("open")))
            }),
        |context| {
            Ok(vec![add_object(
                context.graph.version(),
                "matched",
                "marker",
                json!({}),
            )])
        },
    );
    let mut runtime = GraphRuntime::new();
    runtime.register(Arc::new(behavior)).unwrap();
    runtime
        .propose_patch(
            add_object(0, "claim", "claim", json!({"status": "open"})),
            None,
        )
        .unwrap();
    runtime
        .propose_patch(
            add_object(1, "closed", "task", json!({"status": "closed"})),
            None,
        )
        .unwrap();
    runtime
        .propose_patch(
            add_object(2, "open", "task", json!({"status": "open"})),
            None,
        )
        .unwrap();
    assert!(runtime.graph().object("matched").is_some());
    let starts = runtime
        .events()
        .iter()
        .filter(|record| matches!(&record.event, GraphEvent::BehaviorStarted { name } if name == "only-open-tasks"))
        .count();
    assert_eq!(starts, 1);
}

#[test]
fn relation_shape_behavior_coordinates_endpoints_on_custom_event() {
    let unblock = FnBehavior::new(
        "unblock-dependent",
        EventFilter::new(["task.completed"]).where_predicate(|event, graph| {
            let GraphEvent::Custom { payload, .. } = &event.event else {
                return false;
            };
            let Some(completed) = payload.get("task_id").and_then(|id| id.as_str()) else {
                return false;
            };
            graph
                .relations_from(completed)
                .any(|relation| relation.relation_type == "depends_on")
        }),
        |context| {
            let GraphEvent::Custom { payload, .. } = &context.event.event else {
                return Ok(Vec::new());
            };
            let completed = payload["task_id"].as_str().unwrap_or_default();
            let operations = context
                .graph
                .relations_from(completed)
                .filter(|relation| relation.relation_type == "depends_on")
                .filter_map(|relation| context.graph.object(&relation.target))
                .map(|target| PatchOperation::UpdateObject {
                    id: target.id.clone(),
                    expected_version: target.version,
                    data: json!({"status": "open"}),
                })
                .collect::<Vec<_>>();
            Ok(vec![GraphPatch::new(context.graph.version(), operations)])
        },
    );
    let mut runtime = GraphRuntime::new();
    runtime.register(Arc::new(unblock)).unwrap();
    runtime
        .propose_patch(
            GraphPatch::new(
                0,
                vec![
                    PatchOperation::AddObject {
                        id: "research".into(),
                        object_type: "task".into(),
                        data: json!({"status": "open"}),
                    },
                    PatchOperation::AddObject {
                        id: "memo".into(),
                        object_type: "task".into(),
                        data: json!({"status": "blocked"}),
                    },
                    PatchOperation::AddRelation {
                        id: "dependency".into(),
                        relation_type: "depends_on".into(),
                        source: "research".into(),
                        target: "memo".into(),
                        data: json!({}),
                    },
                ],
            ),
            None,
        )
        .unwrap();
    runtime
        .emit(GraphEvent::Custom {
            name: "task.completed".into(),
            payload: json!({"task_id": "research"}),
        })
        .unwrap();
    assert_eq!(
        runtime.graph().object("memo").unwrap().data["status"],
        "open"
    );
}

#[test]
fn fork_at_event_is_independent_and_structurally_diffable() {
    let mut parent = GraphRuntime::new();
    parent
        .propose_patch(add_object(0, "a", "task", json!({"choice": 1})), None)
        .unwrap();
    let fork_point = parent.events().len() as u64;
    let mut fork = parent.fork_at(fork_point).unwrap();
    assert!(matches!(
        fork.events().last().unwrap().event,
        GraphEvent::BranchForked { .. }
    ));

    parent
        .propose_patch(add_object(1, "parent-only", "result", json!({})), None)
        .unwrap();
    fork.propose_patch(add_object(1, "fork-only", "result", json!({})), None)
        .unwrap();

    assert_ne!(parent.branch_id(), fork.branch_id());
    assert!(parent.graph().object("fork-only").is_none());
    let diff = parent.diff(&fork);
    assert_eq!(diff.objects_added[0].id, "fork-only");
    assert_eq!(diff.objects_removed[0].id, "parent-only");
}

#[test]
fn strict_replay_detects_sequence_version_and_causation_divergence() {
    let mut runtime = GraphRuntime::new();
    runtime
        .propose_patch(add_object(0, "a", "task", json!({})), None)
        .unwrap();

    let mut bad_sequence = runtime.events().to_vec();
    bad_sequence[1].sequence = 99;
    assert!(matches!(
        GraphRuntime::strict_replay(&bad_sequence),
        Err(ReplayError::SequenceDiverged { .. })
    ));

    let mut bad_version = runtime.events().to_vec();
    bad_version[1].state_version_after = 99;
    assert!(matches!(
        GraphRuntime::strict_replay(&bad_version),
        Err(ReplayError::RecordHashDiverged { .. })
    ));

    let mut bad_cause = runtime.events().to_vec();
    bad_cause[1].causation_id = Some("future-event".into());
    assert!(matches!(
        GraphRuntime::strict_replay(&bad_cause),
        Err(ReplayError::InvalidCausation { .. })
    ));
}

#[test]
fn strict_replay_detects_semantic_event_tampering() {
    let mut runtime = GraphRuntime::new();
    runtime
        .propose_patch(add_object(0, "a", "task", json!({"trusted": true})), None)
        .unwrap();
    let mut tampered = runtime.events().to_vec();
    let created = tampered
        .iter_mut()
        .find(|record| matches!(record.event, GraphEvent::ObjectCreated { .. }))
        .unwrap();
    if let GraphEvent::ObjectCreated { data, .. } = &mut created.event {
        *data = json!({"trusted": false});
    }
    assert!(matches!(
        GraphRuntime::strict_replay(&tampered),
        Err(ReplayError::RecordHashDiverged { .. })
    ));
}

#[test]
fn strict_replay_rejects_future_event_schema() {
    let mut runtime = GraphRuntime::new();
    runtime.run_goal("schema").unwrap();
    let mut future = runtime.events().to_vec();
    future[0].schema_version = GRAPH_EVENT_SCHEMA_VERSION + 1;
    assert!(matches!(
        GraphRuntime::strict_replay(&future),
        Err(ReplayError::UnsupportedSchema { .. })
    ));
}

#[test]
fn event_records_round_trip_without_losing_causal_metadata() {
    let mut runtime = GraphRuntime::new();
    runtime
        .propose_patch(add_object(0, "a", "task", json!({})), None)
        .unwrap();
    let json = serde_json::to_string(runtime.events()).unwrap();
    let restored: Vec<GraphEventRecord> = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, runtime.events());
    assert_eq!(
        GraphRuntime::strict_replay(&restored).unwrap(),
        *runtime.graph()
    );
}

#[test]
fn hard_event_limit_stops_reactive_run() {
    let mut runtime = GraphRuntime::with_limits(RuntimeLimits {
        max_events: 1,
        max_behavior_depth: 4,
    });
    let error = runtime
        .propose_patch(add_object(0, "a", "task", json!({})), None)
        .unwrap_err();
    assert!(matches!(error, RuntimeError::EventLimitExceeded(1)));
    assert!(runtime.events().is_empty());
    assert_eq!(runtime.graph().version(), 0);
}

#[test]
fn long_event_log_replays_deterministically_across_many_versions() {
    let mut runtime = GraphRuntime::with_limits(RuntimeLimits {
        max_events: 1_000,
        max_behavior_depth: 64,
    });
    for index in 0..100 {
        let version = runtime.graph().version();
        runtime
            .propose_patch(
                add_object(
                    version,
                    &format!("task-{index:03}"),
                    "task",
                    json!({"index": index}),
                ),
                None,
            )
            .unwrap();
    }
    assert_eq!(runtime.graph().version(), 100);
    assert_eq!(runtime.events().len(), 300);
    let replayed = GraphRuntime::strict_replay(runtime.events()).unwrap();
    assert_eq!(replayed, *runtime.graph());

    for fork_sequence in [0, 3, 75, 150, 300] {
        let fork = runtime.fork_at(fork_sequence).unwrap();
        assert_eq!(fork.events().len(), fork_sequence as usize + 1);
        GraphRuntime::strict_replay(fork.events()).unwrap();
    }
}

#[test]
fn duplicate_event_ids_are_rejected_even_when_hashes_are_recomputed() {
    let mut runtime = GraphRuntime::new();
    runtime
        .propose_patch(add_object(0, "a", "task", json!({})), None)
        .unwrap();
    let mut records = runtime.events().to_vec();
    records[1].id = records[0].id.clone();
    records[1].record_hash = super::graph::record_hash(&records[1]).unwrap();
    assert!(matches!(
        GraphRuntime::strict_replay(&records),
        Err(ReplayError::DuplicateEventId(_))
    ));
}

#[tokio::test]
async fn memory_and_file_event_stores_round_trip_a_restorable_branch() {
    let mut runtime = GraphRuntime::new();
    runtime
        .propose_patch(add_object(0, "a", "task", json!({})), None)
        .unwrap();
    let memory = MemoryGraphEventStore::new();
    memory
        .save(runtime.branch_id(), runtime.events())
        .await
        .unwrap();
    let restored =
        GraphRuntime::restore(memory.load(runtime.branch_id()).await.unwrap().unwrap()).unwrap();
    assert_eq!(restored.graph(), runtime.graph());

    let temp = tempfile::tempdir().unwrap();
    let file = FileGraphEventStore::new(temp.path());
    file.save(runtime.branch_id(), runtime.events())
        .await
        .unwrap();
    file.save(runtime.branch_id(), runtime.events())
        .await
        .unwrap();
    let restored =
        GraphRuntime::restore(file.load(runtime.branch_id()).await.unwrap().unwrap()).unwrap();
    assert_eq!(restored.events(), runtime.events());
    file.delete(runtime.branch_id()).await.unwrap();
    assert!(file.load(runtime.branch_id()).await.unwrap().is_none());
    assert!(temp.path().read_dir().unwrap().next().is_none());
}
