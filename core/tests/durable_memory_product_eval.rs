use a3s_code_core::config::{CodeConfig, ModelConfig, ModelModalities, ProviderConfig};
use a3s_code_core::host_env::{FixedClock, HostEnv, SequentialIdGenerator};
use a3s_code_core::memory::MemoryConfig;
use a3s_code_core::{
    Agent, DurableMemoryRecallPolicy, DurableMemorySession, PlanningMode, SessionOptions,
};
use a3s_memory::repository::{
    DurableMemoryKind, EvidenceKind, EvidenceRef, InMemoryRepository, MemoryChangeSet,
    MemoryNamespace, MemoryNodeDraft, MemoryOperation, MemoryQuery, MemoryRelation,
    MemoryRelationKind, MemoryRepository, MemoryStatus,
};
use a3s_memory::{InMemoryStore, MemoryItem, MemoryStore, MemoryType};
use chrono::{TimeZone, Utc};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;

#[path = "durable_memory_product_eval/model.rs"]
mod model;
use model::{CaptureEvalClient, RecallEvalClient};

const PRODUCT_FIXTURE: &str = include_str!("fixtures/durable-memory-product-v1/evaluation.json");
const RETRIEVAL_FIXTURE: &str = include_str!("fixtures/durable-memory-retrieval-v1/corpus.json");

#[derive(Debug, Deserialize)]
struct ProductFixture {
    schema_version: u32,
    retrieval_fixture: String,
    capture: CaptureFixture,
    thresholds: Thresholds,
}

#[derive(Debug, Deserialize)]
struct CaptureFixture {
    session_id: String,
    prompt: String,
    main_response: String,
    old_memory: OldMemoryFixture,
    items: Vec<CaptureItemFixture>,
}

#[derive(Debug, Deserialize)]
struct OldMemoryFixture {
    content: String,
    importance: f32,
    tags: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CaptureItemFixture {
    #[serde(skip_serializing)]
    expected_accepted: bool,
    memory_type: String,
    content: String,
    importance: f32,
    confidence: f32,
    tags: Vec<String>,
    source: String,
    scope: String,
    reason: String,
    conflicts_with: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Thresholds {
    expected_task_success: ExpectedTaskSuccess,
    minimum_write_precision: f64,
    minimum_evidence_fidelity: f64,
    require_conflict_preservation: bool,
    maximum_memory_context_tokens_per_task: usize,
    maximum_model_calls_per_task: usize,
    maximum_capture_model_calls: usize,
    maximum_p95_latency_ms: f64,
    input_usd_per_million_tokens: f64,
    output_usd_per_million_tokens: f64,
    maximum_estimated_model_cost_usd_per_task: f64,
}

#[derive(Debug, Deserialize)]
struct ExpectedTaskSuccess {
    no_memory: f64,
    v1: f64,
    v2: f64,
}

#[derive(Debug, Deserialize)]
struct RetrievalFixture {
    schema_version: u32,
    policy: PolicyFixture,
    nodes: Vec<NodeFixture>,
    queries: Vec<QueryFixture>,
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
    query: String,
    relevant_node_ids: Vec<String>,
}

#[derive(Clone, Copy)]
enum EvaluationArm {
    NoMemory,
    V1,
    V2,
}

impl EvaluationArm {
    fn label(self) -> &'static str {
        match self {
            Self::NoMemory => "no_memory",
            Self::V1 => "v1",
            Self::V2 => "v2",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArmReport {
    tasks: usize,
    successes: usize,
    task_success_rate: f64,
    model_calls: usize,
    input_tokens: usize,
    output_tokens: usize,
    memory_context_tokens: usize,
    maximum_memory_context_tokens: usize,
    estimated_model_cost_usd: f64,
    estimated_model_cost_usd_per_task: f64,
    p95_latency_ms: f64,
    admissions: u64,
    uses: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CaptureReport {
    proposed_items: usize,
    accepted_v1_items: usize,
    v2_candidates: usize,
    write_precision: f64,
    evidence_fidelity: f64,
    conflict_preserved: bool,
    model_calls: usize,
    input_tokens: usize,
    output_tokens: usize,
    estimated_model_cost_usd: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProductReport {
    schema_version: u32,
    capture: CaptureReport,
    no_memory: ArmReport,
    v1: ArmReport,
    v2: ArmReport,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TurnEvidencePayload<'a> {
    schema: &'static str,
    session_id: &'a str,
    prompt: &'a str,
    response: &'a str,
    transcript: &'a str,
}

fn offline_config(llm_extraction: bool) -> CodeConfig {
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
            llm_extraction,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn fixture_time() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 29, 18, 0, 0)
        .single()
        .expect("valid fixture time")
}

fn evaluation_host_env(prefix: &str) -> Arc<HostEnv> {
    let now = Utc
        .with_ymd_and_hms(2026, 8, 30, 18, 0, 0)
        .single()
        .expect("valid evaluation time")
        .timestamp_millis();
    Arc::new(HostEnv::new(
        Arc::new(SequentialIdGenerator::new(prefix)),
        Arc::new(FixedClock::new(
            u64::try_from(now).expect("positive evaluation time"),
        )),
    ))
}

fn fixture_evidence(node: &NodeFixture) -> EvidenceRef {
    let kind = match node.status {
        MemoryStatus::Active => EvidenceKind::Verification,
        MemoryStatus::Candidate => EvidenceKind::SessionTurn,
        status => panic!("unsupported product fixture status: {status:?}"),
    };
    EvidenceRef::try_new(
        format!("a3s://fixture/durable-memory-product-v1/{}", node.id),
        format!("sha256:{:x}", Sha256::digest(node.content.as_bytes())),
        kind,
        fixture_time(),
    )
    .expect("valid fixture evidence")
}

fn fixture_draft(node: &NodeFixture, namespace: &MemoryNamespace) -> MemoryNodeDraft {
    let mut draft = MemoryNodeDraft::new(
        &node.id,
        namespace.clone(),
        node.kind,
        node.status,
        &node.content,
        vec![fixture_evidence(node)],
        fixture_time(),
    );
    for target in &node.related_to {
        draft = draft.with_relation(MemoryRelation::new(MemoryRelationKind::RelatedTo, target));
    }
    for target in &node.conflicts_with {
        draft = draft.with_relation(MemoryRelation::new(
            MemoryRelationKind::ConflictsWith,
            target,
        ));
    }
    draft
}

async fn seed_v1_store(fixture: &RetrievalFixture, store: &Arc<InMemoryStore>) {
    for node in fixture
        .nodes
        .iter()
        .filter(|node| node.status == MemoryStatus::Active)
    {
        let memory_type = match node.kind {
            DurableMemoryKind::Episodic => MemoryType::Episodic,
            DurableMemoryKind::Semantic => MemoryType::Semantic,
            DurableMemoryKind::Procedural => MemoryType::Procedural,
        };
        let mut item = MemoryItem::new(&node.content)
            .with_type(memory_type)
            .with_importance(1.0);
        item.id = node.id.clone();
        item.timestamp = fixture_time();
        store.store(item).await.expect("seed V1 memory");
    }
}

async fn seed_v2_repository(
    fixture: &RetrievalFixture,
) -> (
    Arc<InMemoryRepository>,
    MemoryNamespace,
    DurableMemorySession,
) {
    let repository = Arc::new(InMemoryRepository::new());
    let namespace =
        MemoryNamespace::try_new("product-tenant", "product-principal", "product-scope")
            .expect("valid product namespace");
    let operations = fixture
        .nodes
        .iter()
        .map(|node| MemoryOperation::Create {
            node: fixture_draft(node, &namespace),
        })
        .collect();
    repository
        .apply(MemoryChangeSet::new(
            "seed-durable-memory-product-v1",
            namespace.clone(),
            fixture_time(),
            operations,
        ))
        .await
        .expect("seed V2 product corpus");
    let policy = DurableMemoryRecallPolicy::try_new(
        fixture.policy.max_results,
        fixture.policy.min_lexical_score,
    )
    .expect("valid product recall policy")
    .try_with_related_lookups(fixture.policy.max_related_lookups)
    .expect("valid relation lookup bound");
    let binding =
        DurableMemorySession::active_recall(repository.clone(), namespace.clone(), policy);
    (repository, namespace, binding)
}

async fn run_retrieval_arm(
    fixture: &RetrievalFixture,
    thresholds: &Thresholds,
    arm: EvaluationArm,
) -> ArmReport {
    let store = Arc::new(InMemoryStore::new());
    if matches!(arm, EvaluationArm::V1) {
        seed_v1_store(fixture, &store).await;
    }
    let durable = if matches!(arm, EvaluationArm::V2) {
        Some(seed_v2_repository(fixture).await)
    } else {
        None
    };
    let content_by_id = fixture
        .nodes
        .iter()
        .map(|node| (node.id.clone(), node.content.clone()))
        .collect::<HashMap<_, _>>();
    let expected_by_query = fixture
        .queries
        .iter()
        .map(|query| {
            let relevant = query
                .relevant_node_ids
                .first()
                .expect("each product task must declare one relevant node");
            (
                query.query.clone(),
                (
                    query.id.clone(),
                    content_by_id
                        .get(relevant)
                        .expect("relevant node must exist")
                        .clone(),
                ),
            )
        })
        .collect();
    let client = Arc::new(RecallEvalClient::new(
        expected_by_query,
        fixture
            .nodes
            .iter()
            .map(|node| node.content.clone())
            .collect(),
    ));
    let agent = Agent::from_config(offline_config(false))
        .await
        .expect("create offline evaluation agent");
    let workspace = tempfile::tempdir().expect("create evaluation workspace");
    let host_env = evaluation_host_env(arm.label());
    let mut latencies = Vec::new();
    for query in &fixture.queries {
        let mut options = SessionOptions::new()
            .with_session_id(format!("product-{}-{}", arm.label(), query.id))
            .with_planning_mode(PlanningMode::Disabled)
            .with_memory(store.clone())
            .with_host_env(host_env.clone())
            .with_llm_client(client.clone());
        if let Some((_, _, binding)) = &durable {
            options = options.with_durable_memory(binding.clone());
        }
        let session = agent
            .session_async(workspace.path().display().to_string(), Some(options))
            .await
            .expect("create product evaluation session");
        let started = Instant::now();
        session
            .send(&query.query, None)
            .await
            .expect("execute product evaluation task");
        latencies.push(started.elapsed().as_secs_f64() * 1_000.0);
        session.close().await;
    }
    agent.close().await;

    let observations = client.observations();
    assert_eq!(observations.len(), fixture.queries.len());
    let successes = observations.iter().filter(|item| item.success).count();
    let input_tokens = observations.iter().map(|item| item.input_tokens).sum();
    let output_tokens = observations.iter().map(|item| item.output_tokens).sum();
    let memory_context_tokens = observations
        .iter()
        .map(|item| item.memory_context_tokens)
        .sum();
    let maximum_memory_context_tokens = observations
        .iter()
        .map(|item| item.memory_context_tokens)
        .max()
        .unwrap_or(0);
    let estimated_model_cost_usd = estimate_cost(input_tokens, output_tokens, thresholds);
    let (admissions, uses) = if let Some((repository, namespace, _)) = &durable {
        let mut admissions = 0;
        let mut uses = 0;
        for node in &fixture.nodes {
            let summary = repository
                .usage_summary(namespace, &node.id)
                .await
                .expect("read V2 usage summary");
            admissions += summary.admissions;
            uses += summary.uses;
        }
        (admissions, uses)
    } else {
        (0, 0)
    };
    let tasks = fixture.queries.len();
    ArmReport {
        tasks,
        successes,
        task_success_rate: successes as f64 / tasks as f64,
        model_calls: observations.len(),
        input_tokens,
        output_tokens,
        memory_context_tokens,
        maximum_memory_context_tokens,
        estimated_model_cost_usd,
        estimated_model_cost_usd_per_task: estimated_model_cost_usd / tasks as f64,
        p95_latency_ms: percentile_95(&mut latencies),
        admissions,
        uses,
    }
}

fn extraction_response(fixture: &CaptureFixture, old_memory_id: &str) -> String {
    let items = fixture
        .items
        .iter()
        .map(|item| {
            let mut value = serde_json::to_value(item).expect("serialize capture item");
            let conflicts = value["conflicts_with"]
                .as_array_mut()
                .expect("conflicts_with must be an array");
            for conflict in conflicts {
                if conflict.as_str() == Some("$OLD_MEMORY_ID") {
                    *conflict = serde_json::json!(old_memory_id);
                }
            }
            value
        })
        .collect::<Vec<_>>();
    serde_json::json!({ "items": items }).to_string()
}

async fn run_capture_evaluation(
    fixture: &CaptureFixture,
    thresholds: &Thresholds,
) -> CaptureReport {
    let store = Arc::new(InMemoryStore::new());
    let mut old = MemoryItem::new(&fixture.old_memory.content)
        .with_type(MemoryType::Semantic)
        .with_importance(fixture.old_memory.importance)
        .with_metadata("source", "project_fact");
    for tag in &fixture.old_memory.tags {
        old = old.with_tag(tag);
    }
    let old_memory_id = old.id.clone();
    store.store(old).await.expect("seed old V1 memory");

    let repository = Arc::new(InMemoryRepository::new());
    let namespace =
        MemoryNamespace::try_new("capture-tenant", "capture-principal", "capture-workspace")
            .expect("valid capture namespace");
    let binding = DurableMemorySession::shadow(repository.clone(), namespace.clone());
    let client = Arc::new(CaptureEvalClient::new(
        &fixture.main_response,
        extraction_response(fixture, &old_memory_id),
    ));
    let agent = Agent::from_config(offline_config(true))
        .await
        .expect("create capture evaluation agent");
    let workspace = tempfile::tempdir().expect("create capture workspace");
    let options = SessionOptions::new()
        .with_session_id(&fixture.session_id)
        .with_planning_mode(PlanningMode::Disabled)
        .with_memory(store.clone())
        .with_durable_memory(binding)
        .with_host_env(evaluation_host_env("capture"))
        .with_llm_client(client.clone());
    let session = agent
        .session_async(workspace.path().display().to_string(), Some(options))
        .await
        .expect("create capture evaluation session");
    let result = session
        .send(&fixture.prompt, None)
        .await
        .expect("execute capture evaluation turn");
    assert_eq!(result.text, fixture.main_response);
    session.close().await;
    agent.close().await;

    let stored = store.get_recent(100).await.expect("read captured V1 items");
    let accepted = stored
        .iter()
        .filter(|item| item.id != old_memory_id)
        .collect::<Vec<_>>();
    let expected_contents = fixture
        .items
        .iter()
        .filter(|item| item.expected_accepted)
        .map(|item| item.content.as_str())
        .collect::<BTreeSet<_>>();
    let accepted_correct = accepted
        .iter()
        .filter(|item| expected_contents.contains(item.content.as_str()))
        .count();
    assert_eq!(accepted.len(), expected_contents.len());
    let write_precision = accepted_correct as f64 / accepted.len().max(1) as f64;
    let replacement = accepted
        .iter()
        .find(|item| expected_contents.contains(item.content.as_str()))
        .expect("valid correction must be stored");
    let conflict_preserved = stored.iter().any(|item| item.id == old_memory_id)
        && replacement
            .metadata
            .get("conflicts_with")
            .is_some_and(|value| value == &old_memory_id);

    let candidates = repository
        .query(
            MemoryQuery::new(namespace.clone())
                .with_statuses([MemoryStatus::Candidate])
                .with_limit(100),
        )
        .await
        .expect("query V2 capture candidates")
        .hits
        .into_iter()
        .map(|hit| hit.node)
        .collect::<Vec<_>>();
    assert_eq!(candidates.len(), expected_contents.len());
    let extraction_prompt = client
        .extraction_prompt()
        .expect("capture client must observe extraction input");
    assert!(extraction_prompt.contains("[redacted sensitive value]"));
    assert!(!extraction_prompt.contains("fixture-secret-value-1234"));
    let (prompt, response, transcript) = extraction_evidence_fields(&extraction_prompt);
    let payload = TurnEvidencePayload {
        schema: "a3s.code.memory.turn-evidence.v1",
        session_id: &fixture.session_id,
        prompt: &prompt,
        response: &response,
        transcript: &transcript,
    };
    let expected_digest = format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(&payload).expect("serialize evidence payload"))
    );
    let encoded_session_id = utf8_percent_encode(&fixture.session_id, NON_ALPHANUMERIC).to_string();
    let evidence_matches = candidates
        .iter()
        .filter(|candidate| {
            candidate.content == replacement.content
                && candidate.status == MemoryStatus::Candidate
                && candidate.evidence.len() == 1
                && candidate.evidence[0].kind == EvidenceKind::SessionTurn
                && candidate.evidence[0].digest == expected_digest
                && candidate.evidence[0].uri.contains(&encoded_session_id)
                && !candidate.evidence[0]
                    .uri
                    .contains("fixture-secret-value-1234")
                && !serde_json::to_string(candidate)
                    .expect("serialize candidate for privacy check")
                    .contains("fixture-secret-value-1234")
        })
        .count();
    let evidence_fidelity = evidence_matches as f64 / candidates.len().max(1) as f64;
    let calls = client.calls();
    let input_tokens = calls.iter().map(|call| call.input_tokens).sum();
    let output_tokens = calls.iter().map(|call| call.output_tokens).sum();

    CaptureReport {
        proposed_items: fixture.items.len(),
        accepted_v1_items: accepted.len(),
        v2_candidates: candidates.len(),
        write_precision,
        evidence_fidelity,
        conflict_preserved,
        model_calls: calls.len(),
        input_tokens,
        output_tokens,
        estimated_model_cost_usd: estimate_cost(input_tokens, output_tokens, thresholds),
    }
}

fn extraction_evidence_fields(extraction_prompt: &str) -> (String, String, String) {
    let (_, after_prompt) = extraction_prompt
        .split_once("User request:\n")
        .expect("extraction prompt must contain user request");
    let (prompt, after_response) = after_prompt
        .split_once("\n\nAssistant final response:\n")
        .expect("extraction prompt must contain assistant response");
    let (response, after_related) = after_response
        .split_once("\n\nRelated existing memories:\n")
        .expect("extraction prompt must contain related memories");
    let (_, transcript) = after_related
        .split_once("\n\nCompressed turn transcript:\n")
        .expect("extraction prompt must contain transcript");
    (
        prompt.to_string(),
        response.to_string(),
        transcript.trim_end().to_string(),
    )
}

fn estimate_cost(input_tokens: usize, output_tokens: usize, thresholds: &Thresholds) -> f64 {
    (input_tokens as f64 * thresholds.input_usd_per_million_tokens
        + output_tokens as f64 * thresholds.output_usd_per_million_tokens)
        / 1_000_000.0
}

fn percentile_95(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let rank = (values.len() * 95).div_ceil(100).saturating_sub(1);
    values.get(rank).copied().unwrap_or(0.0)
}

fn assert_rate(label: &str, actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{label} changed: actual={actual}, expected={expected}"
    );
}

#[tokio::test]
async fn durable_memory_product_evaluation_v1() {
    let product: ProductFixture =
        serde_json::from_str(PRODUCT_FIXTURE).expect("valid product fixture");
    let retrieval: RetrievalFixture =
        serde_json::from_str(RETRIEVAL_FIXTURE).expect("valid retrieval fixture");
    assert_eq!(product.schema_version, 1);
    assert_eq!(retrieval.schema_version, 1);
    assert_eq!(
        product.retrieval_fixture,
        "../durable-memory-retrieval-v1/corpus.json"
    );

    let capture = run_capture_evaluation(&product.capture, &product.thresholds).await;
    let no_memory =
        run_retrieval_arm(&retrieval, &product.thresholds, EvaluationArm::NoMemory).await;
    let v1 = run_retrieval_arm(&retrieval, &product.thresholds, EvaluationArm::V1).await;
    let v2 = run_retrieval_arm(&retrieval, &product.thresholds, EvaluationArm::V2).await;

    assert_rate(
        "no-memory task success",
        no_memory.task_success_rate,
        product.thresholds.expected_task_success.no_memory,
    );
    assert_rate(
        "V1 task success",
        v1.task_success_rate,
        product.thresholds.expected_task_success.v1,
    );
    assert_rate(
        "V2 task success",
        v2.task_success_rate,
        product.thresholds.expected_task_success.v2,
    );
    assert!(capture.write_precision >= product.thresholds.minimum_write_precision);
    assert!(capture.evidence_fidelity >= product.thresholds.minimum_evidence_fidelity);
    assert!(!product.thresholds.require_conflict_preservation || capture.conflict_preserved);
    assert_eq!(
        capture.model_calls,
        product.thresholds.maximum_capture_model_calls
    );
    assert_eq!(no_memory.memory_context_tokens, 0);
    assert!(v2.admissions >= v2.successes as u64);
    assert_eq!(v2.uses, 0);
    for report in [&no_memory, &v1, &v2] {
        assert!(
            report.maximum_memory_context_tokens
                <= product.thresholds.maximum_memory_context_tokens_per_task
        );
        assert!(
            report.model_calls <= report.tasks * product.thresholds.maximum_model_calls_per_task
        );
        assert!(report.p95_latency_ms <= product.thresholds.maximum_p95_latency_ms);
        assert!(
            report.estimated_model_cost_usd_per_task
                <= product.thresholds.maximum_estimated_model_cost_usd_per_task
        );
    }

    let report = ProductReport {
        schema_version: product.schema_version,
        capture,
        no_memory,
        v1,
        v2,
    };
    println!(
        "A3S_DURABLE_MEMORY_PRODUCT_EVAL={}",
        serde_json::to_string(&report).expect("serialize product report")
    );
}
