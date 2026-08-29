use a3s_code_core::{
    DurableMemoryActivation, DurableMemoryBindingV1, DurableMemoryMode, DurableMemoryRecallChannel,
    DurableMemoryRecallHit, DurableMemoryRecallPolicy, DurableMemoryRecallPreview,
    DurableMemorySession, DurableMemoryUse, DURABLE_MEMORY_BINDING_SCHEMA_VERSION,
    DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2, DURABLE_MEMORY_RETRIEVAL_PROFILE_V1,
};
use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, InMemoryRepository, MemoryChangeSet,
    MemoryNamespace, MemoryNodeDraft, MemoryOperation, MemoryRepository, MemoryStatus,
};
use chrono::{TimeZone, Utc};
use std::sync::Arc;

fn time(second: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 29, 16, 0, second)
        .single()
        .unwrap()
}

fn evidence(uri: &str, kind: EvidenceKind, second: u32) -> EvidenceRef {
    EvidenceRef::try_new(uri, format!("sha256:{:0>64}", uri), kind, time(second)).unwrap()
}

#[test]
fn active_memory_policy_is_bounded_and_public_types_are_thread_safe() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DurableMemoryActivation>();
    assert_send_sync::<DurableMemoryBindingV1>();
    assert_send_sync::<DurableMemoryRecallChannel>();
    assert_send_sync::<DurableMemoryRecallHit>();
    assert_send_sync::<DurableMemoryRecallPolicy>();
    assert_send_sync::<DurableMemoryRecallPreview>();
    assert_send_sync::<DurableMemorySession>();
    assert_send_sync::<DurableMemoryUse>();

    assert!(DurableMemoryRecallPolicy::try_new(0, 0.25).is_err());
    assert!(DurableMemoryRecallPolicy::try_new(1, f32::NAN).is_err());
    assert!(DurableMemoryRecallPolicy::try_new(1, 1.01).is_err());
    assert!(DurableMemoryRecallPolicy::try_new(1, 0.0).is_ok());
    assert!(DurableMemoryRecallPolicy::try_new(1, 0.0)
        .unwrap()
        .try_with_related_lookups(101)
        .is_err());
    assert_eq!(
        DurableMemoryRecallPolicy::try_new(1, 0.0)
            .unwrap()
            .try_with_related_lookups(4)
            .unwrap()
            .max_related_lookups(),
        4
    );

    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let policy = DurableMemoryRecallPolicy::try_new(3, 0.25)
        .unwrap()
        .try_with_related_lookups(2)
        .unwrap();
    let session = DurableMemorySession::active_recall(
        Arc::new(InMemoryRepository::new()),
        namespace.clone(),
        policy,
    );
    let binding = session.binding();
    assert_eq!(
        binding.schema_version(),
        DURABLE_MEMORY_BINDING_SCHEMA_VERSION
    );
    assert_eq!(binding.namespace(), &namespace);
    assert_eq!(binding.mode(), DurableMemoryMode::ActiveRecall);
    assert_eq!(binding.recall_policy(), Some(policy));
    assert_eq!(
        binding.retrieval_profile(),
        DURABLE_MEMORY_RETRIEVAL_PROFILE_V1
    );
    assert_eq!(
        binding.context_id_profile(),
        DURABLE_MEMORY_CONTEXT_ID_PROFILE_V2
    );
    let roundtrip: DurableMemoryBindingV1 =
        serde_json::from_str(&serde_json::to_string(&binding).unwrap()).unwrap();
    assert_eq!(roundtrip, binding);
}

#[tokio::test]
async fn active_recall_requires_explicit_evidence_backed_activation() {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let policy = DurableMemoryRecallPolicy::try_new(3, 0.25).unwrap();
    let binding =
        DurableMemorySession::active_recall(repository.clone(), namespace.clone(), policy);
    assert_eq!(binding.mode(), DurableMemoryMode::ActiveRecall);
    assert_eq!(binding.recall_policy().unwrap().max_results(), 3);

    repository
        .apply(MemoryChangeSet::new(
            "create-candidate",
            namespace.clone(),
            time(1),
            vec![MemoryOperation::Create {
                node: MemoryNodeDraft::new(
                    "candidate",
                    namespace.clone(),
                    DurableMemoryKind::Procedural,
                    MemoryStatus::Candidate,
                    "Run focused tests after changing durable memory",
                    vec![evidence(
                        "a3s://session/one/turn/one",
                        EvidenceKind::SessionTurn,
                        1,
                    )],
                    time(1),
                ),
            }],
        ))
        .await
        .unwrap();

    let invalid = DurableMemoryActivation::try_new(
        "activate-invalid",
        "candidate",
        1,
        evidence("a3s://session/one/turn/two", EvidenceKind::SessionTurn, 2),
        time(2),
    );
    assert!(
        invalid.is_err(),
        "self-reported turn evidence is not approval"
    );

    let activated = binding
        .activate_candidate(
            DurableMemoryActivation::try_new(
                "activate-candidate",
                "candidate",
                1,
                evidence(
                    "a3s://verification/memory/candidate",
                    EvidenceKind::Verification,
                    2,
                ),
                time(2),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(activated.status, MemoryStatus::Active);
    assert_eq!(activated.revision, 2);
    assert_eq!(activated.evidence.len(), 2);

    binding
        .record_use(
            DurableMemoryUse::try_new("use-candidate", "candidate", 2, time(3))
                .unwrap()
                .with_context_id("turn-context-1"),
        )
        .await
        .unwrap();
    assert_eq!(
        repository
            .usage_summary(&namespace, "candidate")
            .await
            .unwrap()
            .uses,
        1
    );
}
