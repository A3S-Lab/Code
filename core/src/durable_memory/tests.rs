use super::*;
use a3s_memory::repository::{
    EvidenceKind, EvidenceRef, InMemoryRepository, MemoryChangeSet, MemoryNodeDraft,
    MemoryOperation, MemoryQuery,
};

fn time(offset_seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_777_000_000 + offset_seconds, 0).unwrap()
}

fn evidence(name: &str, kind: EvidenceKind, offset_seconds: i64) -> EvidenceRef {
    EvidenceRef::try_new(
        format!("a3s://evidence/{name}"),
        format!("sha256:{name:0>64}"),
        kind,
        time(offset_seconds),
    )
    .unwrap()
}

#[tokio::test]
async fn shadow_write_is_evidence_backed_candidate_and_never_active() {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let binding = DurableMemorySession::shadow(repository.clone(), namespace.clone());
    let occurred_at = DateTime::from_timestamp_millis(1_777_000_000_000).unwrap();
    let turn_evidence = DurableTurnEvidence::try_new(
        "session/one",
        "turn one",
        "remember this",
        "done",
        "user: remember this",
        occurred_at,
    )
    .unwrap();
    let item = MemoryItem::new("The repository requires focused crate tests")
        .with_type(MemoryType::Procedural)
        .with_importance(0.9)
        .with_metadata("confidence", "0.88")
        .with_metadata("source", "workflow")
        .with_metadata("scope", "workspace")
        .with_metadata("reason", "This prevents invalid root workspace builds")
        .with_metadata("schema", "a3s.memory.durable.v1");

    let node = binding
        .store_shadow_candidate(&item, &turn_evidence)
        .await
        .unwrap();
    assert_eq!(node.status, MemoryStatus::Candidate);
    assert_eq!(node.evidence.len(), 1);
    assert!(node.evidence[0].uri.contains("session%2Fone"));
    assert!(!node.evidence[0].uri.contains("remember this"));
    assert_eq!(node.confidence, 0.88);
    assert!(repository
        .query(MemoryQuery::new(namespace.clone()))
        .await
        .unwrap()
        .hits
        .is_empty());
    assert_eq!(
        repository
            .query(
                MemoryQuery::new(namespace.clone())
                    .with_statuses([MemoryStatus::Candidate])
                    .with_text("focused crate"),
            )
            .await
            .unwrap()
            .hits
            .len(),
        1
    );

    let replay = binding
        .store_shadow_candidate(&item, &turn_evidence)
        .await
        .unwrap();
    assert_eq!(replay, node);
    assert_eq!(
        repository
            .query(
                MemoryQuery::new(namespace.clone())
                    .with_statuses([MemoryStatus::Candidate])
                    .with_text("focused crate"),
            )
            .await
            .unwrap()
            .hits
            .len(),
        1
    );

    binding
        .activate_candidate(
            DurableMemoryActivation::try_new(
                "activate-shadow-candidate",
                &node.id,
                1,
                evidence("shadow-approval", EvidenceKind::Verification, 1),
                time(1),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .query(MemoryQuery::new(namespace).with_text("focused crate"))
            .await
            .unwrap()
            .hits
            .len(),
        1
    );
    assert!(binding
        .query_active_context("focused crate")
        .await
        .unwrap()
        .result
        .is_empty());
}

#[tokio::test]
async fn active_context_is_admitted_only_for_the_selected_current_revision() {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let binding = DurableMemorySession::active_recall(
        repository.clone(),
        namespace.clone(),
        DurableMemoryRecallPolicy::try_new(3, 0.2).unwrap(),
    );
    repository
        .apply(MemoryChangeSet::new(
            "create-active-candidate",
            namespace.clone(),
            time(1),
            vec![MemoryOperation::Create {
                node: MemoryNodeDraft::new(
                    "active-node",
                    namespace.clone(),
                    DurableMemoryKind::Procedural,
                    MemoryStatus::Candidate,
                    "Run focused durable memory tests after changing admission",
                    vec![evidence("proposal-active", EvidenceKind::SessionTurn, 1)],
                    time(1),
                ),
            }],
        ))
        .await
        .unwrap();
    repository
        .apply(MemoryChangeSet::new(
            "create-shadow-candidate",
            namespace.clone(),
            time(1),
            vec![MemoryOperation::Create {
                node: MemoryNodeDraft::new(
                    "candidate-node",
                    namespace.clone(),
                    DurableMemoryKind::Procedural,
                    MemoryStatus::Candidate,
                    "Run focused durable memory tests after changing candidates",
                    vec![evidence("proposal-shadow", EvidenceKind::SessionTurn, 1)],
                    time(1),
                ),
            }],
        ))
        .await
        .unwrap();
    binding
        .activate_candidate(
            DurableMemoryActivation::try_new(
                "activate-active-candidate",
                "active-node",
                1,
                evidence("approval", EvidenceKind::Verification, 2),
                time(2),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let batch = binding
        .query_active_context("focused durable memory tests")
        .await
        .unwrap();
    assert_eq!(batch.result.items.len(), 1);
    assert_eq!(batch.identities[0].node_id, "active-node");
    let mut unselected = crate::context::ContextAssembly {
        items: vec![crate::context::ContextItem::new(
            "ordinary-only",
            crate::context::ContextType::Resource,
            "ordinary context",
        )
        .with_token_count(2)],
        total_tokens: 2,
        truncated: true,
    };
    assert_eq!(
        binding
            .admit_selected_context(
                &mut unselected,
                &batch.identities,
                "context-unselected",
                Some(time(3)),
            )
            .await,
        0,
        "query hits dropped by final assembly must not count as admissions"
    );
    let mut assembly = crate::context::ContextAssembly {
        items: vec![
            batch.result.items[0].clone(),
            crate::context::ContextItem::new(
                "ordinary",
                crate::context::ContextType::Resource,
                "ordinary context",
            )
            .with_token_count(2),
        ],
        total_tokens: batch.result.items[0].token_count + 2,
        truncated: false,
    };

    assert_eq!(
        binding
            .admit_selected_context(
                &mut assembly,
                &batch.identities,
                "context-one",
                Some(time(3)),
            )
            .await,
        1
    );
    assert_eq!(
        repository
            .usage_summary(&namespace, "active-node")
            .await
            .unwrap()
            .admissions,
        1
    );

    repository
        .apply(MemoryChangeSet::new(
            "tombstone-active-node",
            namespace.clone(),
            time(4),
            vec![MemoryOperation::SetStatus {
                node_id: "active-node".into(),
                expected_revision: 2,
                status: MemoryStatus::Tombstoned,
            }],
        ))
        .await
        .unwrap();
    assert_eq!(
        binding
            .admit_selected_context(
                &mut assembly,
                &batch.identities,
                "context-two",
                Some(time(5)),
            )
            .await,
        0
    );
    assert_eq!(assembly.items.len(), 1);
    assert_eq!(assembly.items[0].id, "ordinary");
}
