use super::*;
use a3s_memory::repository::{
    EvidenceKind, EvidenceRef, InMemoryRepository, MemoryChangeSet, MemoryNodeDraft,
    MemoryOperation, MemoryQuery, MemoryRelation, MemoryRelationKind,
};
use tokio_util::sync::CancellationToken;

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
async fn candidate_write_stays_inactive_until_explicit_activation() {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let binding = DurableMemorySession::active_recall(
        repository.clone(),
        namespace.clone(),
        DurableMemoryRecallPolicy::try_new(8, 0.0).unwrap(),
    );
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
    assert!(
        binding
            .query_active_context("focused crate")
            .await
            .unwrap()
            .result
            .is_empty(),
        "candidates must not enter Active recall before activation"
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
    assert!(
        !binding
            .query_active_context("focused crate")
            .await
            .unwrap()
            .result
            .is_empty(),
        "activated nodes must be eligible for Active recall"
    );
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
    let mut unbound = crate::context::ContextAssembly {
        items: batch.result.items.clone(),
        total_tokens: batch.result.items[0].token_count,
        truncated: false,
    };
    assert_eq!(
        binding
            .admit_selected_context(&mut unbound, &batch.identities, None, Some(time(3)))
            .await,
        0,
        "memory without an exact invocation identity must fail closed"
    );
    assert!(unbound.items.is_empty());
    assert_eq!(unbound.total_tokens, 0);
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
                Some("context-unselected"),
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
                Some("context-one"),
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
                Some("context-two"),
                Some(time(5)),
            )
            .await,
        0
    );
    assert_eq!(assembly.items.len(), 1);
    assert_eq!(assembly.items[0].id, "ordinary");
}

#[tokio::test]
async fn related_recall_is_bounded_active_only_and_excludes_conflicts() {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    repository
        .apply(MemoryChangeSet::new(
            "seed-related-recall",
            namespace.clone(),
            time(1),
            vec![
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "rollback-index",
                        namespace.clone(),
                        DurableMemoryKind::Semantic,
                        MemoryStatus::Active,
                        "Deployment rollback playbook index",
                        vec![evidence(
                            "index-verification",
                            EvidenceKind::Verification,
                            1,
                        )],
                        time(1),
                    )
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::RelatedTo,
                        "canary-procedure",
                    ))
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::RelatedTo,
                        "zebra-procedure",
                    ))
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::ConflictsWith,
                        "unsafe-procedure",
                    )),
                },
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "zebra-procedure",
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Active,
                        "Restart every production shard at the same time",
                        vec![evidence(
                            "zebra-verification",
                            EvidenceKind::Verification,
                            1,
                        )],
                        time(1),
                    )
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::RelatedTo,
                        "rollback-index",
                    )),
                },
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "canary-procedure",
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Active,
                        "Drain the first ring before shifting production traffic",
                        vec![evidence(
                            "canary-verification",
                            EvidenceKind::Verification,
                            1,
                        )],
                        time(1),
                    )
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::RelatedTo,
                        "rollback-index",
                    )),
                },
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "unsafe-procedure",
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Active,
                        "Shift all traffic without observing the first ring",
                        vec![evidence(
                            "unsafe-verification",
                            EvidenceKind::Verification,
                            1,
                        )],
                        time(1),
                    )
                    .with_relation(MemoryRelation::new(
                        MemoryRelationKind::ConflictsWith,
                        "rollback-index",
                    )),
                },
                MemoryOperation::Create {
                    node: MemoryNodeDraft::new(
                        "candidate-related",
                        namespace.clone(),
                        DurableMemoryKind::Procedural,
                        MemoryStatus::Candidate,
                        "Unverified recovery shortcut",
                        vec![evidence("candidate-proposal", EvidenceKind::SessionTurn, 1)],
                        time(1),
                    ),
                },
            ],
        ))
        .await
        .unwrap();
    repository
        .apply(MemoryChangeSet::new(
            "attach-candidate-relation",
            namespace.clone(),
            time(2),
            vec![
                MemoryOperation::AddRelation {
                    node_id: "rollback-index".into(),
                    expected_revision: 1,
                    relation: MemoryRelation::new(
                        MemoryRelationKind::RelatedTo,
                        "candidate-related",
                    ),
                },
                MemoryOperation::AddRelation {
                    node_id: "candidate-related".into(),
                    expected_revision: 1,
                    relation: MemoryRelation::new(MemoryRelationKind::RelatedTo, "rollback-index"),
                },
            ],
        ))
        .await
        .unwrap();

    let lexical = DurableMemorySession::active_recall(
        repository.clone(),
        namespace.clone(),
        DurableMemoryRecallPolicy::try_new(4, 0.2).unwrap(),
    )
    .preview_recall("deployment rollback")
    .await
    .unwrap();
    assert_eq!(lexical.hits.len(), 1);
    assert_eq!(lexical.hits[0].node_id, "rollback-index");

    let related = DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(4, 0.2)
            .unwrap()
            .try_with_related_lookups(2)
            .unwrap(),
    )
    .preview_recall("deployment rollback")
    .await
    .unwrap();
    assert_eq!(related.hits.len(), 2);
    assert_eq!(related.hits[0].node_id, "rollback-index");
    assert_eq!(related.hits[0].channel, DurableMemoryRecallChannel::Lexical);
    assert_eq!(related.hits[1].node_id, "canary-procedure");
    assert_eq!(related.hits[1].channel, DurableMemoryRecallChannel::Related);
    assert_eq!(
        related.hits[1].related_from.as_deref(),
        Some("rollback-index")
    );
    assert!(related
        .hits
        .iter()
        .all(|hit| hit.node_id != "unsafe-procedure"));
    assert!(related
        .hits
        .iter()
        .all(|hit| hit.node_id != "candidate-related"));
    assert!(related
        .hits
        .iter()
        .all(|hit| hit.node_id != "zebra-procedure"));
}

#[test]
fn activation_try_new_rejects_invalid_revision_and_evidence() {
    let occurred_at = time(10);
    let late_evidence = evidence("late", EvidenceKind::Verification, 11);

    assert!(DurableMemoryActivation::try_new(
        "activate",
        "node-1",
        0,
        evidence("approval", EvidenceKind::Verification, 9),
        occurred_at,
    )
    .is_err());

    assert!(DurableMemoryActivation::try_new(
        "activate",
        "node-1",
        1,
        evidence("turn", EvidenceKind::SessionTurn, 9),
        occurred_at,
    )
    .is_err());

    assert!(
        DurableMemoryActivation::try_new("activate", "node-1", 1, late_evidence, occurred_at,)
            .is_err()
    );
}

#[test]
fn durable_memory_use_rejects_revision_zero() {
    assert!(DurableMemoryUse::try_new("use-1", "node-1", 0, time(1)).is_err());
}

#[tokio::test]
async fn refresh_semantic_recall_requires_attached_semantic_generation() {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let binding = DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(4, 0.2).unwrap(),
    );

    let err = binding
        .refresh_semantic_recall(CancellationToken::new())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("semantic recall"));
}

#[tokio::test]
async fn store_shadow_candidate_rejects_working_memory() {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let binding = DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(4, 0.2).unwrap(),
    );
    let item = MemoryItem::new("temporary note").with_type(MemoryType::Working);
    let turn_evidence = DurableTurnEvidence::try_new(
        "session/working",
        "turn",
        "prompt",
        "response",
        "transcript",
        time(1),
    )
    .unwrap();

    let err = binding
        .store_shadow_candidate(&item, &turn_evidence)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("working memory is not durable"));
}

#[test]
fn durable_memory_activation_accepts_manual_evidence_and_rejects_empty_ids() {
    assert!(DurableMemoryActivation::try_new(
        "activate",
        "node-1",
        1,
        evidence("manual", EvidenceKind::Manual, 1),
        time(2),
    )
    .is_ok());
    assert!(DurableMemoryActivation::try_new(
        "   ",
        "node-1",
        1,
        evidence("m", EvidenceKind::Manual, 1),
        time(2),
    )
    .is_err());
    assert!(DurableMemoryUse::try_new("use", "   ", 1, time(1)).is_err());
}

#[tokio::test]
async fn record_use_with_context_id_and_session_accessors() {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let binding = DurableMemorySession::active_recall(
        repository.clone(),
        namespace.clone(),
        DurableMemoryRecallPolicy::try_new(4, 0.0).unwrap(),
    );
    assert!(matches!(binding.mode(), DurableMemoryMode::ActiveRecall));
    assert!(binding.recall_policy().is_some());
    assert!(binding.semantic_recall().is_none());
    assert_eq!(binding.namespace(), &namespace);
    let _ = binding.repository();
    let debug = format!("{binding:?}");
    assert!(debug.contains("DurableMemorySession"));

    repository
        .apply(MemoryChangeSet::new(
            "create-for-use",
            namespace.clone(),
            time(1),
            vec![MemoryOperation::Create {
                node: MemoryNodeDraft::new(
                    "used-node",
                    namespace.clone(),
                    DurableMemoryKind::Procedural,
                    MemoryStatus::Candidate,
                    "Use recording requires an exact active revision",
                    vec![evidence("proposal", EvidenceKind::SessionTurn, 1)],
                    time(1),
                ),
            }],
        ))
        .await
        .unwrap();
    binding
        .activate_candidate(
            DurableMemoryActivation::try_new(
                "activate-used-node",
                "used-node",
                1,
                evidence("approval", EvidenceKind::Verification, 2),
                time(2),
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let usage = DurableMemoryUse::try_new("use-event", "used-node", 1, time(3))
        .unwrap()
        .with_context_id("ctx-1");
    binding.record_use(usage).await.unwrap();
    assert_eq!(
        binding.binding().schema_version(),
        DURABLE_MEMORY_BINDING_SCHEMA_VERSION
    );
}

#[test]
fn durable_turn_evidence_binds_percent_encoded_session_and_turn() {
    let occurred_at = time(12);
    let evidence = DurableTurnEvidence::try_new(
        "session/one",
        "turn one",
        "remember this",
        "done",
        "user: remember this",
        occurred_at,
    )
    .unwrap();
    assert!(evidence.reference.uri.contains("session%2Fone"));
    assert!(evidence.reference.uri.contains("turn%20one"));
    assert_eq!(evidence.reference.kind, EvidenceKind::SessionTurn);
}

#[tokio::test]
async fn store_shadow_candidate_accepts_semantic_memory_with_tags() {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let binding = DurableMemorySession::active_recall(
        repository.clone(),
        namespace,
        DurableMemoryRecallPolicy::try_new(4, 0.0).unwrap(),
    );
    let item = MemoryItem::new("Prefer crate-local tests")
        .with_type(MemoryType::Semantic)
        .with_metadata("source", "workflow")
        .with_tags(vec!["testing".into()]);
    let turn_evidence = DurableTurnEvidence::try_new(
        "session/tags",
        "turn",
        "prompt",
        "response",
        "transcript",
        time(1),
    )
    .unwrap();
    let node = binding
        .store_shadow_candidate(&item, &turn_evidence)
        .await
        .unwrap();
    assert_eq!(node.kind, DurableMemoryKind::Semantic);
    assert!(node.labels.contains_key("a3s.extraction.tags"));
}

#[tokio::test]
async fn store_shadow_candidate_accepts_episodic_memory() {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let binding = DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(4, 0.0).unwrap(),
    );
    let item = MemoryItem::new("Yesterday the crate tests failed on main")
        .with_type(MemoryType::Episodic)
        .with_metadata("confidence", "0.7");
    let turn_evidence = DurableTurnEvidence::try_new(
        "session/episodic",
        "turn",
        "prompt",
        "response",
        "transcript",
        time(1),
    )
    .unwrap();
    let node = binding
        .store_shadow_candidate(&item, &turn_evidence)
        .await
        .unwrap();
    assert_eq!(node.kind, DurableMemoryKind::Episodic);
    assert!((node.confidence - 0.7).abs() < f32::EPSILON);
}

#[test]
fn durable_memory_use_rejects_oversized_identifiers() {
    let oversized = "x".repeat(MAX_IDENTIFIER_BYTES + 1);
    assert!(DurableMemoryUse::try_new(oversized.as_str(), "node-1", 1, time(1)).is_err());
    assert!(DurableMemoryUse::try_new("use-1", oversized.as_str(), 1, time(1)).is_err());
}

#[test]
fn recall_policy_rejects_invalid_bounds() {
    assert!(DurableMemoryRecallPolicy::try_new(0, 0.0).is_err());
    assert!(DurableMemoryRecallPolicy::try_new(4, f32::NAN).is_err());
    assert!(DurableMemoryRecallPolicy::try_new(4, 1.5).is_err());
    let policy = DurableMemoryRecallPolicy::try_new(4, 0.0).unwrap();
    assert!(policy.try_with_related_lookups(usize::MAX).is_err());
}

#[test]
fn fuse_lexical_semantic_covers_empty_semantic_lexical_and_hybrid() {
    use super::context::RecallCandidate;
    use super::fusion::fuse_lexical_semantic;
    use super::semantic::SemanticRecallCandidate;
    use a3s_memory::repository::MemoryRevisionKind;
    use std::collections::BTreeMap;

    fn node(id: &str, updated_offset: i64) -> MemoryNode {
        MemoryNode {
            id: id.into(),
            namespace: MemoryNamespace::try_new("tenant", "principal", "scope").unwrap(),
            revision: 1,
            kind: DurableMemoryKind::Procedural,
            status: MemoryStatus::Active,
            content: format!("content-{id}"),
            confidence: 0.5,
            importance: 0.5,
            evidence: Vec::new(),
            relations: Vec::new(),
            labels: BTreeMap::new(),
            created_at: time(0),
            updated_at: time(updated_offset),
            revision_kind: MemoryRevisionKind::Created,
            history: Vec::new(),
        }
    }

    let lexical_only = fuse_lexical_semantic(
        vec![RecallCandidate {
            node: node("lex-1", 1),
            score: 0.9,
            channel: DurableMemoryRecallChannel::Lexical,
            related_from: None,
        }],
        Vec::new(),
    );
    assert_eq!(lexical_only.len(), 1);
    assert_eq!(lexical_only[0].channel, DurableMemoryRecallChannel::Lexical);

    let semantic_only = fuse_lexical_semantic(
        Vec::new(),
        vec![SemanticRecallCandidate {
            node: node("sem-1", 2),
            score: 0.8,
        }],
    );
    assert_eq!(semantic_only.len(), 1);
    assert_eq!(
        semantic_only[0].channel,
        DurableMemoryRecallChannel::Semantic
    );

    let hybrid = fuse_lexical_semantic(
        vec![RecallCandidate {
            node: node("shared", 1),
            score: 0.4,
            channel: DurableMemoryRecallChannel::Lexical,
            related_from: None,
        }],
        vec![SemanticRecallCandidate {
            node: node("shared", 3),
            score: 0.95,
        }],
    );
    assert_eq!(hybrid.len(), 1);
    assert_eq!(hybrid[0].channel, DurableMemoryRecallChannel::Hybrid);
    assert_eq!(hybrid[0].node.updated_at, time(3));
}

#[test]
fn oversized_identifier_is_rejected_for_activation() {
    let too_long = "a".repeat(a3s_memory::repository::MAX_IDENTIFIER_BYTES + 1);
    let err = DurableMemoryActivation::try_new(
        too_long,
        "node-1",
        1,
        evidence("decision", EvidenceKind::Manual, 0),
        time(1),
    )
    .unwrap_err();
    assert!(err.to_string().contains("must not exceed"), "{err}");
}

#[tokio::test]
async fn store_shadow_candidate_accepts_tags_and_clamps_invalid_confidence() {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("tenant", "principal", "scope").unwrap();
    let binding = DurableMemorySession::active_recall(
        repository,
        namespace,
        DurableMemoryRecallPolicy::try_new(8, 0.0).unwrap(),
    );
    let occurred_at = time(0);
    let turn_evidence = DurableTurnEvidence::try_new(
        "session-tags",
        "turn-1",
        "remember",
        "done",
        "user: remember",
        occurred_at,
    )
    .unwrap();
    let item = MemoryItem::new("candidate with tags")
        .with_type(MemoryType::Episodic)
        .with_tags(vec!["alpha".into(), "beta".into()])
        .with_metadata("confidence", "not-a-number");
    let node = binding
        .store_shadow_candidate(&item, &turn_evidence)
        .await
        .unwrap();
    assert_eq!(node.kind, DurableMemoryKind::Episodic);
    assert_eq!(node.confidence, 0.0);
}
