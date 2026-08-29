use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::embedding::EmbeddingExecutorConfig;
use a3s_code_core::host_env::{FixedClock, HostEnv, SequentialIdGenerator};
use a3s_code_core::memory::MemoryConfig;
use a3s_code_core::{
    Agent, DurableMemoryRecallChannel, DurableMemoryRecallPolicy, DurableMemorySemanticRecall,
    DurableMemorySemanticRecallPolicy, DurableMemorySession, PlanningMode, SessionOptions,
    DURABLE_MEMORY_HYBRID_BINDING_SCHEMA_VERSION, DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1,
    DURABLE_MEMORY_SEMANTIC_FUSION_PROFILE_V1,
};
use a3s_memory::repository::{
    EvidenceKind, EvidenceRef, InMemoryRepository, MemoryChangeSet, MemoryNamespace,
    MemoryNodeDraft, MemoryOperation, MemoryRepository, MemoryStatus, RevisionMode,
};
use a3s_memory::vector::{InMemoryVectorIndex, VectorIndex, VectorIndexDescriptor};
use a3s_memory::InMemoryStore;
use chrono::{TimeZone, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[path = "durable_memory_semantic_eval/fixture.rs"]
mod fixture;
#[path = "durable_memory_semantic_eval/model.rs"]
mod model;
#[path = "durable_memory_semantic_eval/provider.rs"]
mod provider;
use fixture::{Fixture, Node, StaleNode, FIXTURE};
use model::InspectingClient;
use provider::FixtureEmbeddingProvider;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Report<'a> {
    schema_version: u32,
    binding_schema_version: u32,
    semantic_binding_schema: &'a str,
    fusion_profile: &'a str,
    queries: usize,
    semantic_recall_at_1: f64,
    lexical_positive_hits: usize,
    negative_hits: usize,
    model_calls: usize,
    admissions: u64,
    maximum_context_nodes: usize,
    candidate_foreign_or_stale_leaks: usize,
}

fn fixture_time(second: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 30, 1, 0, second)
        .single()
        .expect("valid semantic fixture time")
}

fn evidence(id: &str, content: &str, kind: EvidenceKind, second: u32) -> EvidenceRef {
    EvidenceRef::try_new(
        format!("a3s://fixture/durable-memory-semantic-v1/{id}"),
        format!("sha256:{:x}", Sha256::digest(content.as_bytes())),
        kind,
        fixture_time(second),
    )
    .expect("valid semantic fixture evidence")
}

fn draft(node: &Node, namespace: &MemoryNamespace) -> MemoryNodeDraft {
    let evidence_kind = if node.status == MemoryStatus::Active {
        EvidenceKind::Verification
    } else {
        EvidenceKind::SessionTurn
    };
    MemoryNodeDraft::new(
        &node.id,
        namespace.clone(),
        node.kind,
        node.status,
        &node.content,
        vec![evidence(&node.id, &node.content, evidence_kind, 1)],
        fixture_time(1),
    )
}

fn stale_draft(node: &StaleNode, namespace: &MemoryNamespace) -> MemoryNodeDraft {
    MemoryNodeDraft::new(
        &node.id,
        namespace.clone(),
        node.kind,
        MemoryStatus::Active,
        &node.indexed_content,
        vec![evidence(
            &node.id,
            &node.indexed_content,
            EvidenceKind::Verification,
            1,
        )],
        fixture_time(1),
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
        Arc::new(SequentialIdGenerator::new("semantic")),
        Arc::new(FixedClock::new(
            u64::try_from(fixture_time(2).timestamp_millis()).unwrap(),
        )),
    ))
}

async fn seed(fixture: &Fixture) -> (Arc<InMemoryRepository>, MemoryNamespace, MemoryNamespace) {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace = MemoryNamespace::try_new("fixture-tenant", "fixture-principal", "workspace")
        .expect("valid semantic namespace");
    let mut operations = fixture
        .nodes
        .iter()
        .map(|node| MemoryOperation::Create {
            node: draft(node, &namespace),
        })
        .collect::<Vec<_>>();
    operations.push(MemoryOperation::Create {
        node: draft(&fixture.candidate_node, &namespace),
    });
    operations.push(MemoryOperation::Create {
        node: stale_draft(&fixture.stale_node, &namespace),
    });
    repository
        .apply(MemoryChangeSet::new(
            "seed-durable-memory-semantic-v1",
            namespace.clone(),
            fixture_time(1),
            operations,
        ))
        .await
        .expect("seed semantic corpus");

    let foreign = MemoryNamespace::try_new("foreign-tenant", "fixture-principal", "workspace")
        .expect("valid foreign semantic namespace");
    repository
        .apply(MemoryChangeSet::new(
            "seed-foreign-semantic-node",
            foreign.clone(),
            fixture_time(1),
            vec![MemoryOperation::Create {
                node: draft(&fixture.foreign_node, &foreign),
            }],
        ))
        .await
        .expect("seed foreign semantic node");
    (repository, namespace, foreign)
}

#[tokio::test]
async fn real_sessions_meet_versioned_semantic_recall_gate() {
    let fixture: Fixture = serde_json::from_str(FIXTURE).expect("valid semantic fixture");
    assert_eq!(fixture.schema_version, 1);
    assert_eq!(
        fixture.binding_schema_version,
        DURABLE_MEMORY_HYBRID_BINDING_SCHEMA_VERSION
    );
    assert_eq!(
        fixture.semantic_binding_schema,
        DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1
    );
    assert_eq!(
        fixture.fusion_profile,
        DURABLE_MEMORY_SEMANTIC_FUSION_PROFILE_V1
    );
    let (repository, namespace, foreign_namespace) = seed(&fixture).await;
    let recall_policy = DurableMemoryRecallPolicy::try_new(
        fixture.recall_policy.max_results,
        fixture.recall_policy.min_lexical_score,
    )
    .expect("valid semantic lexical policy")
    .try_with_related_lookups(fixture.recall_policy.max_related_lookups)
    .expect("valid semantic relation bound");
    let lexical =
        DurableMemorySession::active_recall(repository.clone(), namespace.clone(), recall_policy);

    let mut lexical_positive_hits = 0usize;
    for query in &fixture.queries {
        lexical_positive_hits += lexical
            .preview_recall(&query.query)
            .await
            .expect("preview lexical baseline")
            .hits
            .len();
    }

    let provider = Arc::new(FixtureEmbeddingProvider::new(&fixture));
    let index: Arc<dyn VectorIndex> = Arc::new(
        InMemoryVectorIndex::new(VectorIndexDescriptor::new(fixture.embedding.dimension))
            .expect("valid semantic fixture index"),
    );
    let semantic = DurableMemorySemanticRecall::new(
        &fixture.embedding.authority_digest,
        provider,
        EmbeddingExecutorConfig::default(),
        index,
        DurableMemorySemanticRecallPolicy::try_new(
            fixture.semantic_policy.candidate_limit,
            fixture.semantic_policy.min_score,
        )
        .expect("valid semantic candidate policy"),
    )
    .expect("construct semantic recall runtime");
    let mut indexed_nodes = Vec::new();
    for node in &fixture.nodes {
        indexed_nodes.push(
            repository
                .get(&namespace, &node.id)
                .await
                .expect("read semantic node")
                .expect("semantic node exists"),
        );
    }
    indexed_nodes.push(
        repository
            .get(&namespace, &fixture.stale_node.id)
            .await
            .expect("read stale semantic node")
            .expect("stale semantic node exists"),
    );
    semantic
        .replace_namespace(&namespace, indexed_nodes, CancellationToken::new())
        .await
        .expect("publish local semantic partition");
    let candidate = repository
        .get(&namespace, &fixture.candidate_node.id)
        .await
        .expect("read candidate semantic node")
        .expect("candidate semantic node exists");
    assert!(semantic
        .replace_namespace(&namespace, vec![candidate], CancellationToken::new())
        .await
        .is_err());
    semantic
        .replace_namespace(
            &foreign_namespace,
            vec![repository
                .get(&foreign_namespace, &fixture.foreign_node.id)
                .await
                .expect("read foreign semantic node")
                .expect("foreign semantic node exists")],
            CancellationToken::new(),
        )
        .await
        .expect("publish foreign semantic partition");
    repository
        .apply(MemoryChangeSet::new(
            "revise-stale-semantic-node",
            namespace.clone(),
            fixture_time(2),
            vec![MemoryOperation::Revise {
                node_id: fixture.stale_node.id.clone(),
                expected_revision: 1,
                content: fixture.stale_node.current_content.clone(),
                mode: RevisionMode::Correction,
                evidence: vec![evidence(
                    "stale-failover-revision-2",
                    &fixture.stale_node.current_content,
                    EvidenceKind::Verification,
                    2,
                )],
                confidence: None,
                importance: None,
            }],
        ))
        .await
        .expect("revise stale semantic node after indexing");

    let durable = lexical.with_semantic_recall(semantic).unwrap();
    let binding = durable.binding();
    assert_eq!(binding.schema_version(), fixture.binding_schema_version);
    let semantic_binding = binding
        .semantic_recall()
        .expect("semantic binding retained");
    assert_eq!(semantic_binding.schema(), fixture.semantic_binding_schema);
    assert_eq!(semantic_binding.fusion_profile(), fixture.fusion_profile);

    let mut recalled_at_one = 0usize;
    for query in &fixture.queries {
        let hits = durable
            .preview_recall(&query.query)
            .await
            .expect("preview semantic recall")
            .hits;
        assert_eq!(hits.len(), 1, "{} returned {hits:?}", query.id);
        assert_eq!(hits[0].node_id, query.relevant_node_id, "{}", query.id);
        assert_eq!(hits[0].channel, DurableMemoryRecallChannel::Semantic);
        recalled_at_one += 1;
    }
    let semantic_recall_at_1 = recalled_at_one as f64 / fixture.queries.len() as f64;

    let mut negative_hits = 0usize;
    for query in &fixture.negative_queries {
        let hits = durable
            .preview_recall(&query.query)
            .await
            .expect("preview semantic negative")
            .hits;
        negative_hits += hits.len();
        assert!(hits.is_empty(), "{} returned {hits:?}", query.id);
    }

    let client = Arc::new(InspectingClient::new(&fixture));
    let agent = Agent::from_config(offline_config())
        .await
        .expect("create semantic evaluation agent");
    let workspace = tempfile::tempdir().expect("create semantic evaluation workspace");
    let env = host_env();
    for query in &fixture.queries {
        let options = SessionOptions::new()
            .with_session_id(format!("semantic-{}", query.id))
            .with_planning_mode(PlanningMode::Disabled)
            .with_memory(Arc::new(InMemoryStore::new()))
            .with_durable_memory(durable.clone())
            .with_host_env(env.clone())
            .with_llm_client(client.clone());
        let session = agent
            .session_async(workspace.path().display().to_string(), Some(options))
            .await
            .expect("create semantic evaluation session");
        let result = session
            .send(&query.query, None)
            .await
            .expect("run semantic evaluation session");
        assert_eq!(result.text, format!("PASS:{}", query.id));
        session.close().await;
    }
    agent.close().await;

    let observations = client.observations();
    assert_eq!(observations.len(), fixture.queries.len());
    for observation in &observations {
        assert!(
            observation.target_visible,
            "{} target was absent from model context",
            observation.query_id
        );
        assert!(
            !observation.forbidden_visible,
            "{} leaked forbidden semantic content",
            observation.query_id
        );
        assert!(
            observation.context_nodes <= fixture.thresholds.maximum_context_nodes_per_query,
            "{} injected {} context nodes",
            observation.query_id,
            observation.context_nodes
        );
    }

    let mut admissions = 0u64;
    for node in &fixture.nodes {
        let usage = repository
            .usage_summary(&namespace, &node.id)
            .await
            .expect("read semantic usage summary");
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

    assert!(semantic_recall_at_1 >= fixture.thresholds.minimum_semantic_recall_at_1);
    assert!(lexical_positive_hits <= fixture.thresholds.maximum_lexical_positive_hits);
    assert!(negative_hits <= fixture.thresholds.maximum_negative_hits);
    assert!(
        observations.len()
            <= fixture.queries.len() * fixture.thresholds.maximum_model_calls_per_query
    );
    assert_eq!(admissions, fixture.thresholds.expected_admissions);
    assert_eq!(leaks, 0);

    println!(
        "A3S_DURABLE_MEMORY_SEMANTIC_EVAL={}",
        serde_json::to_string(&Report {
            schema_version: fixture.schema_version,
            binding_schema_version: binding.schema_version(),
            semantic_binding_schema: semantic_binding.schema(),
            fusion_profile: semantic_binding.fusion_profile(),
            queries: fixture.queries.len(),
            semantic_recall_at_1,
            lexical_positive_hits,
            negative_hits,
            model_calls: observations.len(),
            admissions,
            maximum_context_nodes,
            candidate_foreign_or_stale_leaks: leaks,
        })
        .expect("serialize semantic evaluation report")
    );
}
