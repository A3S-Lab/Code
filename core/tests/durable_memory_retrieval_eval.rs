use a3s_code_core::{DurableMemoryRecallPolicy, DurableMemorySession};
use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, InMemoryRepository, MemoryChangeSet,
    MemoryNamespace, MemoryNodeDraft, MemoryOperation, MemoryRelation, MemoryRelationKind,
    MemoryRepository, MemoryStatus,
};
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::Arc;

const FIXTURE: &str = include_str!("fixtures/durable-memory-retrieval-v1/corpus.json");

#[derive(Debug, Deserialize)]
struct RetrievalFixture {
    schema_version: u32,
    policy: PolicyFixture,
    nodes: Vec<NodeFixture>,
    queries: Vec<QueryFixture>,
    negative_queries: Vec<NegativeQueryFixture>,
    expected_summary: ExpectedSummary,
    vector_gate: VectorGate,
}

#[derive(Debug, Deserialize)]
struct PolicyFixture {
    max_results: usize,
    min_lexical_score: f32,
    max_related_lookups: usize,
}

#[derive(Debug, Deserialize)]
struct NodeFixture {
    id: String,
    kind: DurableMemoryKind,
    status: MemoryStatus,
    content: String,
    related_to: Vec<String>,
    conflicts_with: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct QueryFixture {
    id: String,
    #[serde(rename = "category")]
    _category: String,
    query: String,
    relevant_node_ids: Vec<String>,
    expected_lexical_ids: Vec<String>,
    expected_relation_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NegativeQueryFixture {
    id: String,
    query: String,
    forbidden_node_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedSummary {
    no_memory: Metrics,
    lexical: Metrics,
    relation: Metrics,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
struct Metrics {
    recall_at_5: f64,
    mean_reciprocal_rank: f64,
}

#[derive(Debug, Deserialize)]
struct VectorGate {
    minimum_relation_recall_at_5: f64,
    required: bool,
}

fn fixture_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 29, 17, 0, 0)
        .single()
        .expect("fixture timestamp must be valid")
}

fn evidence(node: &NodeFixture) -> EvidenceRef {
    let kind = match node.status {
        MemoryStatus::Active => EvidenceKind::Verification,
        MemoryStatus::Candidate => EvidenceKind::SessionTurn,
        status => panic!("fixture does not support {status:?} nodes"),
    };
    EvidenceRef::try_new(
        format!("a3s://fixture/durable-memory-retrieval-v1/{}", node.id),
        format!("sha256:{:x}", Sha256::digest(node.content.as_bytes())),
        kind,
        fixture_time(),
    )
    .expect("fixture evidence must be valid")
}

fn draft(node: &NodeFixture, namespace: &MemoryNamespace) -> MemoryNodeDraft {
    let mut draft = MemoryNodeDraft::new(
        &node.id,
        namespace.clone(),
        node.kind,
        node.status,
        &node.content,
        vec![evidence(node)],
        fixture_time(),
    );
    for target_id in &node.related_to {
        draft = draft.with_relation(MemoryRelation::new(
            MemoryRelationKind::RelatedTo,
            target_id,
        ));
    }
    for target_id in &node.conflicts_with {
        draft = draft.with_relation(MemoryRelation::new(
            MemoryRelationKind::ConflictsWith,
            target_id,
        ));
    }
    draft
}

fn metrics(queries: &[QueryFixture], ranked_ids: &BTreeMap<String, Vec<String>>) -> Metrics {
    let mut recall = 0.0;
    let mut reciprocal_rank = 0.0;
    for query in queries {
        let empty = Vec::new();
        let ranked = ranked_ids.get(&query.id).unwrap_or(&empty);
        let first_relevant = ranked
            .iter()
            .take(5)
            .position(|id| query.relevant_node_ids.contains(id));
        if let Some(index) = first_relevant {
            recall += 1.0;
            reciprocal_rank += 1.0 / (index + 1) as f64;
        }
    }
    let count = queries.len() as f64;
    Metrics {
        recall_at_5: recall / count,
        mean_reciprocal_rank: reciprocal_rank / count,
    }
}

fn assert_metrics(label: &str, actual: Metrics, expected: Metrics) {
    const EPSILON: f64 = 1e-9;
    assert!(
        (actual.recall_at_5 - expected.recall_at_5).abs() < EPSILON,
        "{label} Recall@5 changed: actual={}, expected={}",
        actual.recall_at_5,
        expected.recall_at_5
    );
    assert!(
        (actual.mean_reciprocal_rank - expected.mean_reciprocal_rank).abs() < EPSILON,
        "{label} MRR changed: actual={}, expected={}",
        actual.mean_reciprocal_rank,
        expected.mean_reciprocal_rank
    );
}

#[tokio::test]
async fn locked_retrieval_quality_and_safety_gate() {
    let fixture: RetrievalFixture =
        serde_json::from_str(FIXTURE).expect("retrieval fixture must remain valid JSON");
    assert_eq!(fixture.schema_version, 1);

    let repository = Arc::new(InMemoryRepository::new());
    let namespace =
        MemoryNamespace::try_new("fixture-tenant", "fixture-principal", "fixture-scope")
            .expect("fixture namespace must be valid");
    let operations = fixture
        .nodes
        .iter()
        .map(|node| MemoryOperation::Create {
            node: draft(node, &namespace),
        })
        .collect();
    repository
        .apply(MemoryChangeSet::new(
            "seed-durable-memory-retrieval-v1",
            namespace.clone(),
            fixture_time(),
            operations,
        ))
        .await
        .expect("fixture corpus must satisfy repository invariants");

    let foreign_namespace =
        MemoryNamespace::try_new("foreign-tenant", "fixture-principal", "fixture-scope")
            .expect("foreign namespace must be valid");
    let foreign = NodeFixture {
        id: "foreign-rust-format".into(),
        kind: DurableMemoryKind::Procedural,
        status: MemoryStatus::Active,
        content: "rust formatting verification".into(),
        related_to: Vec::new(),
        conflicts_with: Vec::new(),
    };
    repository
        .apply(MemoryChangeSet::new(
            "seed-foreign-namespace",
            foreign_namespace.clone(),
            fixture_time(),
            vec![MemoryOperation::Create {
                node: draft(&foreign, &foreign_namespace),
            }],
        ))
        .await
        .expect("foreign fixture node must be valid");

    let lexical = DurableMemorySession::active_recall(
        repository.clone(),
        namespace.clone(),
        DurableMemoryRecallPolicy::try_new(
            fixture.policy.max_results,
            fixture.policy.min_lexical_score,
        )
        .expect("policy must be valid"),
    );
    let relation = DurableMemorySession::active_recall(
        repository.clone(),
        namespace.clone(),
        DurableMemoryRecallPolicy::try_new(
            fixture.policy.max_results,
            fixture.policy.min_lexical_score,
        )
        .expect("policy must be valid")
        .try_with_related_lookups(fixture.policy.max_related_lookups)
        .expect("relation bound must be valid"),
    );

    let mut lexical_results = BTreeMap::new();
    let mut relation_results = BTreeMap::new();
    for query in &fixture.queries {
        let lexical_ids = lexical
            .preview_recall(&query.query)
            .await
            .expect("lexical preview must succeed")
            .hits
            .into_iter()
            .map(|hit| hit.node_id)
            .collect::<Vec<_>>();
        let relation_ids = relation
            .preview_recall(&query.query)
            .await
            .expect("relation preview must succeed")
            .hits
            .into_iter()
            .map(|hit| hit.node_id)
            .collect::<Vec<_>>();
        assert_eq!(
            lexical_ids, query.expected_lexical_ids,
            "{} lexical ranking changed",
            query.id
        );
        assert_eq!(
            relation_ids, query.expected_relation_ids,
            "{} relation ranking changed",
            query.id
        );
        assert!(!lexical_ids.iter().any(|id| id == &foreign.id));
        assert!(!relation_ids.iter().any(|id| id == &foreign.id));
        lexical_results.insert(query.id.clone(), lexical_ids);
        relation_results.insert(query.id.clone(), relation_ids);
    }

    for query in &fixture.negative_queries {
        let returned = relation
            .preview_recall(&query.query)
            .await
            .expect("negative preview must succeed")
            .hits
            .into_iter()
            .map(|hit| hit.node_id)
            .collect::<Vec<_>>();
        for forbidden in &query.forbidden_node_ids {
            assert!(
                !returned.contains(forbidden),
                "{} leaked forbidden node {forbidden}",
                query.id
            );
        }
    }

    let no_memory_results = fixture
        .queries
        .iter()
        .map(|query| (query.id.clone(), Vec::new()))
        .collect::<BTreeMap<_, _>>();
    let no_memory_metrics = metrics(&fixture.queries, &no_memory_results);
    let lexical_metrics = metrics(&fixture.queries, &lexical_results);
    let relation_metrics = metrics(&fixture.queries, &relation_results);
    assert_metrics(
        "no-memory",
        no_memory_metrics,
        fixture.expected_summary.no_memory,
    );
    assert_metrics("lexical", lexical_metrics, fixture.expected_summary.lexical);
    assert_metrics(
        "relation",
        relation_metrics,
        fixture.expected_summary.relation,
    );

    assert!(
        relation_metrics.recall_at_5 >= fixture.vector_gate.minimum_relation_recall_at_5,
        "relation retrieval fell below the predeclared vector gate"
    );
    assert!(!fixture.vector_gate.required);

    for node in &fixture.nodes {
        let summary = repository
            .usage_summary(&namespace, &node.id)
            .await
            .expect("usage summary must remain readable");
        assert_eq!(summary.admissions, 0, "preview admitted {}", node.id);
        assert_eq!(summary.uses, 0, "preview used {}", node.id);
    }

    println!(
        "A3S_DURABLE_MEMORY_RETRIEVAL_EVAL={}",
        serde_json::json!({
            "fixtureVersion": fixture.schema_version,
            "noMemory": no_memory_metrics,
            "lexical": lexical_metrics,
            "relation": relation_metrics,
            "vectorRequired": fixture.vector_gate.required,
        })
    );
}
