use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::host_env::{FixedClock, HostEnv, SequentialIdGenerator};
use a3s_code_core::memory::MemoryConfig;
use a3s_code_core::{
    Agent, DurableMemoryRecallPolicy, DurableMemorySession, PlanningMode, SessionOptions,
    DURABLE_MEMORY_RETRIEVAL_PROFILE_V1,
};
use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, InMemoryRepository, MemoryChangeSet,
    MemoryNamespace, MemoryNodeDraft, MemoryOperation, MemoryRepository, MemoryStatus,
};
use a3s_memory::InMemoryStore;
use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

const FIXTURE: &str = include_str!("fixtures/durable-memory-multilingual-v1/evaluation.json");

#[path = "durable_memory_multilingual_eval/model.rs"]
mod model;
use model::InspectingClient;

#[derive(Debug, Deserialize)]
struct Fixture {
    schema_version: u32,
    retrieval_profile: String,
    policy: Policy,
    nodes: Vec<Node>,
    foreign_node: Node,
    queries: Vec<Query>,
    negative_queries: Vec<NegativeQuery>,
    thresholds: Thresholds,
}

#[derive(Debug, Deserialize)]
struct Policy {
    max_results: usize,
    min_lexical_score: f32,
    max_related_lookups: usize,
}

#[derive(Debug, Deserialize)]
struct Node {
    id: String,
    kind: DurableMemoryKind,
    status: MemoryStatus,
    content: String,
}

#[derive(Debug, Deserialize)]
struct Query {
    id: String,
    #[serde(rename = "language")]
    _language: String,
    query: String,
    relevant_node_id: String,
}

#[derive(Debug, Deserialize)]
struct NegativeQuery {
    id: String,
    query: String,
}

#[derive(Debug, Deserialize)]
struct Thresholds {
    minimum_recall_at_3: f64,
    minimum_mean_reciprocal_rank: f64,
    maximum_context_nodes_per_query: usize,
    maximum_model_calls_per_query: usize,
    expected_admissions: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Report<'a> {
    schema_version: u32,
    retrieval_profile: &'a str,
    queries: usize,
    recall_at_3: f64,
    mean_reciprocal_rank: f64,
    model_calls: usize,
    admissions: u64,
    maximum_context_nodes: usize,
    negative_queries_with_hits: usize,
    candidate_or_foreign_leaks: usize,
}

fn fixture_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 29, 20, 0, 0)
        .single()
        .expect("valid multilingual fixture time")
}

fn evidence(node: &Node) -> EvidenceRef {
    let kind = if node.status == MemoryStatus::Active {
        EvidenceKind::Verification
    } else {
        EvidenceKind::SessionTurn
    };
    EvidenceRef::try_new(
        format!("a3s://fixture/durable-memory-multilingual-v1/{}", node.id),
        format!("sha256:{:x}", Sha256::digest(node.content.as_bytes())),
        kind,
        fixture_time(),
    )
    .expect("valid multilingual fixture evidence")
}

fn draft(node: &Node, namespace: &MemoryNamespace) -> MemoryNodeDraft {
    MemoryNodeDraft::new(
        &node.id,
        namespace.clone(),
        node.kind,
        node.status,
        &node.content,
        vec![evidence(node)],
        fixture_time(),
    )
}

fn offline_config() -> CodeConfig {
    CodeConfig {
        default_model: Some("anthropic/claude-sonnet-4-20250514".to_string()),
        providers: vec![ProviderConfig {
            name: "anthropic".to_string(),
            api_key: Some("offline-key".to_string()),
            base_url: None,
            headers: HashMap::new(),
            session_id_header: None,
            models: vec![ModelConfig {
                id: "claude-sonnet-4-20250514".to_string(),
                name: "Claude Sonnet 4".to_string(),
                family: "claude-sonnet".to_string(),
                api_key: None,
                base_url: None,
                headers: HashMap::new(),
                session_id_header: None,
                attachment: false,
                reasoning: false,
                tool_call: true,
                temperature: true,
                release_date: None,
                modalities: ModelModalities::default(),
                cost: Default::default(),
                limit: Default::default(),
            }],
        }],
        memory: Some(MemoryConfig {
            llm_extraction: false,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn host_env() -> Arc<HostEnv> {
    Arc::new(HostEnv::new(
        Arc::new(SequentialIdGenerator::new("multilingual")),
        Arc::new(FixedClock::new(
            u64::try_from(fixture_time().timestamp_millis()).unwrap(),
        )),
    ))
}

async fn seed(fixture: &Fixture) -> (Arc<InMemoryRepository>, MemoryNamespace) {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("fixture-tenant", "fixture-principal", "workspace")
        .expect("valid multilingual namespace");
    repository
        .apply(MemoryChangeSet::new(
            "seed-durable-memory-multilingual-v1",
            namespace.clone(),
            fixture_time(),
            fixture
                .nodes
                .iter()
                .map(|node| MemoryOperation::Create {
                    node: draft(node, &namespace),
                })
                .collect(),
        ))
        .await
        .expect("seed multilingual corpus");
    let foreign_namespace =
        MemoryNamespace::try_new("foreign-tenant", "fixture-principal", "workspace")
            .expect("valid foreign namespace");
    repository
        .apply(MemoryChangeSet::new(
            "seed-foreign-multilingual-node",
            foreign_namespace.clone(),
            fixture_time(),
            vec![MemoryOperation::Create {
                node: draft(&fixture.foreign_node, &foreign_namespace),
            }],
        ))
        .await
        .expect("seed foreign multilingual node");
    (repository, namespace)
}

#[tokio::test]
async fn real_sessions_meet_versioned_multilingual_lexical_gate() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("valid multilingual fixture");
    assert_eq!(fixture.schema_version, 1);
    assert_eq!(
        fixture.retrieval_profile,
        DURABLE_MEMORY_RETRIEVAL_PROFILE_V1
    );
    let (repository, namespace) = seed(&fixture).await;
    let policy = DurableMemoryRecallPolicy::try_new(
        fixture.policy.max_results,
        fixture.policy.min_lexical_score,
    )
    .expect("valid multilingual recall policy")
    .try_with_related_lookups(fixture.policy.max_related_lookups)
    .expect("valid multilingual relation bound");
    let durable =
        DurableMemorySession::active_recall(repository.clone(), namespace.clone(), policy);
    assert_eq!(
        durable.binding().retrieval_profile(),
        fixture.retrieval_profile
    );

    let mut recalled = 0usize;
    let mut reciprocal_rank = 0.0;
    for query in &fixture.queries {
        let ids = durable
            .preview_recall(&query.query)
            .await
            .expect("preview multilingual recall")
            .hits
            .into_iter()
            .map(|hit| hit.node_id)
            .collect::<Vec<_>>();
        let rank = ids
            .iter()
            .position(|id| id == &query.relevant_node_id)
            .expect("relevant multilingual memory must be recalled");
        assert_eq!(rank, 0, "{} ranking changed: {ids:?}", query.id);
        if rank < 3 {
            recalled += 1;
        }
        reciprocal_rank += 1.0 / (rank + 1) as f64;
    }
    let recall_at_3 = recalled as f64 / fixture.queries.len() as f64;
    let mean_reciprocal_rank = reciprocal_rank / fixture.queries.len() as f64;

    let mut negative_queries_with_hits = 0usize;
    for query in &fixture.negative_queries {
        let hits = durable
            .preview_recall(&query.query)
            .await
            .expect("preview negative multilingual recall")
            .hits;
        if !hits.is_empty() {
            negative_queries_with_hits += 1;
        }
        assert!(
            hits.is_empty(),
            "{} unexpectedly returned {hits:?}",
            query.id
        );
    }

    let client = Arc::new(InspectingClient::new(&fixture));
    let agent = Agent::from_config(offline_config())
        .await
        .expect("create multilingual evaluation agent");
    let workspace = tempfile::tempdir().expect("create multilingual workspace");
    let env = host_env();
    for query in &fixture.queries {
        let options = SessionOptions::new()
            .with_session_id(format!("multilingual-{}", query.id))
            .with_planning_mode(PlanningMode::Disabled)
            .with_memory(Arc::new(InMemoryStore::new()))
            .with_durable_memory(durable.clone())
            .with_host_env(env.clone())
            .with_llm_client(client.clone());
        let session = agent
            .session_async(workspace.path().display().to_string(), Some(options))
            .await
            .expect("create multilingual evaluation session");
        let result = session
            .send(&query.query, None)
            .await
            .expect("run multilingual evaluation session");
        assert_eq!(result.text, format!("PASS:{}", query.id));
        session.close().await;
    }
    agent.close().await;

    let observations = client.observations();
    assert_eq!(observations.len(), fixture.queries.len());
    for observation in &observations {
        assert!(
            observation.target_visible,
            "{} target was absent from the real model context",
            observation.query_id
        );
        assert!(
            !observation.forbidden_visible,
            "{} leaked candidate or foreign memory",
            observation.query_id
        );
        assert!(
            observation.context_nodes <= fixture.thresholds.maximum_context_nodes_per_query,
            "{} injected {} nodes",
            observation.query_id,
            observation.context_nodes
        );
    }

    let mut admissions = 0u64;
    for node in &fixture.nodes {
        let usage = repository
            .usage_summary(&namespace, &node.id)
            .await
            .expect("read multilingual usage summary");
        admissions += usage.admissions;
        assert_eq!(usage.uses, 0, "evaluation must not fabricate use events");
    }
    let maximum_context_nodes = observations
        .iter()
        .map(|observation| observation.context_nodes)
        .max()
        .unwrap_or(0);
    let leaks = observations
        .iter()
        .filter(|observation| observation.forbidden_visible)
        .count();

    assert!(recall_at_3 >= fixture.thresholds.minimum_recall_at_3);
    assert!(mean_reciprocal_rank >= fixture.thresholds.minimum_mean_reciprocal_rank);
    assert!(
        observations.len()
            <= fixture.queries.len() * fixture.thresholds.maximum_model_calls_per_query
    );
    assert_eq!(admissions, fixture.thresholds.expected_admissions);

    println!(
        "A3S_DURABLE_MEMORY_MULTILINGUAL_EVAL={}",
        serde_json::to_string(&Report {
            schema_version: fixture.schema_version,
            retrieval_profile: DURABLE_MEMORY_RETRIEVAL_PROFILE_V1,
            queries: fixture.queries.len(),
            recall_at_3,
            mean_reciprocal_rank,
            model_calls: observations.len(),
            admissions,
            maximum_context_nodes,
            negative_queries_with_hits,
            candidate_or_foreign_leaks: leaks,
        })
        .expect("serialize multilingual report")
    );
}
