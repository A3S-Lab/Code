use a3s_code_core::{
    EventFilter, FnBehavior, GraphEvent, GraphEventStore, GraphPatch, GraphRuntime,
    MemoryGraphEventStore, PatchOperation,
};
use serde_json::json;
use std::sync::Arc;

fn add_object(version: u64, id: &str, object_type: &str, data: serde_json::Value) -> GraphPatch {
    GraphPatch::new(
        version,
        vec![PatchOperation::AddObject {
            id: id.into(),
            object_type: object_type.into(),
            data,
        }],
    )
}

#[tokio::test]
async fn reactive_branch_persists_restores_and_diverges_structurally() {
    let derive_risk = FnBehavior::new(
        "derive-risk",
        EventFilter::new(["object.created"]).with_object_types(["claim"]),
        |context| {
            let GraphEvent::ObjectCreated { id, .. } = &context.event.event else {
                return Ok(Vec::new());
            };
            Ok(vec![add_object(
                context.graph.version(),
                "risk-1",
                "risk",
                json!({"claim_id": id, "severity": "unknown"}),
            )])
        },
    );
    let mut parent = GraphRuntime::new().with_correlation_id("integration-run");
    parent.register(Arc::new(derive_risk)).unwrap();
    parent
        .propose_patch(
            add_object(0, "claim-1", "claim", json!({"text": "Revenue grew"})),
            None,
        )
        .unwrap();
    assert!(parent.graph().object("risk-1").is_some());

    let store = MemoryGraphEventStore::new();
    store
        .save(parent.branch_id(), parent.events())
        .await
        .unwrap();
    let persisted = store.load(parent.branch_id()).await.unwrap().unwrap();
    let restored = GraphRuntime::restore(persisted).unwrap();
    assert_eq!(restored.graph(), parent.graph());

    let mut alternative = restored.fork_at(restored.events().len() as u64).unwrap();
    alternative
        .propose_patch(
            add_object(
                alternative.graph().version(),
                "memo-1",
                "memo",
                json!({"conclusion": "investigate"}),
            ),
            None,
        )
        .unwrap();
    let diff = parent.diff(&alternative);
    assert_eq!(diff.objects_added.len(), 1);
    assert_eq!(diff.objects_added[0].id, "memo-1");
    GraphRuntime::strict_replay(alternative.events()).unwrap();
}
