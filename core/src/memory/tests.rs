use super::*;
use crate::context::ContextProvider;
use a3s_memory::InMemoryStore;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct RecordingObserver {
    observations: Mutex<Vec<MemoryObservation>>,
    fail: bool,
}

#[async_trait::async_trait]
impl MemoryObserver for RecordingObserver {
    async fn on_memory_stored(&self, observation: MemoryObservation) -> anyhow::Result<()> {
        self.observations.lock().unwrap().push(observation);
        if self.fail {
            anyhow::bail!("observer projection failed");
        }
        Ok(())
    }
}

#[tokio::test]
async fn test_agent_memory_remember_and_recall() {
    let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
    memory
        .remember_success("create file", &["write".to_string()], "ok")
        .await
        .unwrap();
    memory
        .remember_failure("delete file", "denied", &["bash".to_string()])
        .await
        .unwrap();

    let results = memory.recall_similar("create", 10).await.unwrap();
    assert!(!results.is_empty());

    let stats = memory.stats().await.unwrap();
    assert_eq!(stats.long_term_count, 2);
    assert_eq!(stats.short_term_count, 2);
}

#[tokio::test]
async fn test_agent_memory_forget_removes_all_tiers() {
    let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
    let item = memory
        .remember_item(MemoryItem::new("superseded memory"))
        .await
        .unwrap();
    memory.add_to_working(item.clone()).await.unwrap();

    memory.forget(&item.id).await.unwrap();

    assert_eq!(memory.stats().await.unwrap().long_term_count, 0);
    assert!(memory.get_short_term().await.is_empty());
    assert!(memory.get_working().await.is_empty());
}

#[tokio::test]
async fn test_agent_memory_uses_canonical_store_item_for_duplicates() {
    let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
    let first = memory
        .remember_item(
            MemoryItem::new("Run focused memory extraction tests after parser changes.")
                .with_importance(0.3)
                .with_tag("memory"),
        )
        .await
        .unwrap();

    let duplicate = memory
        .remember_item(
            MemoryItem::new("  run focused MEMORY extraction tests after parser changes.  ")
                .with_importance(0.9)
                .with_tag("tests"),
        )
        .await
        .unwrap();

    assert_eq!(duplicate.id, first.id);
    assert_eq!(memory.stats().await.unwrap().long_term_count, 1);
    let short_term = memory.get_short_term().await;
    assert_eq!(short_term.len(), 1);
    assert_eq!(short_term[0].id, first.id);
    assert_eq!(short_term[0].importance, 0.9);
    assert!(short_term[0].tags.contains(&"memory".to_string()));
    assert!(short_term[0].tags.contains(&"tests".to_string()));
}

#[tokio::test]
async fn test_memory_observer_receives_incoming_and_canonical_duplicate() {
    let observer = Arc::new(RecordingObserver::default());
    let memory = AgentMemory::with_config_and_observers(
        Arc::new(InMemoryStore::new()),
        MemoryConfig::default(),
        vec![observer.clone()],
    );
    let first = memory
        .remember_item(
            MemoryItem::new("Run focused observer tests after memory persistence changes.")
                .with_importance(0.8)
                .with_metadata("session_id", "session-one"),
        )
        .await
        .unwrap();
    let duplicate_input =
        MemoryItem::new("  run focused OBSERVER tests after memory persistence changes.  ")
            .with_importance(0.95)
            .with_metadata("session_id", "session-two");
    let duplicate_input_id = duplicate_input.id.clone();
    let duplicate = memory.remember_item(duplicate_input).await.unwrap();

    let observations = observer.observations.lock().unwrap();
    assert_eq!(observations.len(), 2);
    assert!(!observations[0].merged);
    assert_eq!(observations[0].incoming.id, observations[0].stored.id);
    assert!(observations[1].merged);
    assert_eq!(observations[1].incoming.id, duplicate_input_id);
    assert_eq!(observations[1].stored.id, first.id);
    assert_eq!(observations[1].stored.id, duplicate.id);
    assert_ne!(observations[1].incoming.id, observations[1].stored.id);
    assert_eq!(
        observations[1]
            .incoming
            .metadata
            .get("session_id")
            .map(String::as_str),
        Some("session-two")
    );
}

#[tokio::test]
async fn test_memory_observer_failure_does_not_roll_back_persistence() {
    let store = Arc::new(InMemoryStore::new());
    let observer = Arc::new(RecordingObserver {
        observations: Mutex::new(Vec::new()),
        fail: true,
    });
    let memory = AgentMemory::with_config_and_observers(
        store.clone(),
        MemoryConfig::default(),
        vec![observer.clone()],
    );

    let stored = memory
        .remember_item(MemoryItem::new(
            "Persist even if a derived projection fails.",
        ))
        .await
        .expect("observer errors must not fail the durable memory write");

    assert_eq!(store.count().await.unwrap(), 1);
    assert_eq!(memory.short_term_count().await, 1);
    assert_eq!(memory.get_short_term().await[0].id, stored.id);
    assert_eq!(observer.observations.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn test_agent_memory_working() {
    let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
    memory
        .add_to_working(MemoryItem::new("task").with_type(MemoryType::Working))
        .await
        .unwrap();
    assert_eq!(memory.working_count().await, 1);
    memory.clear_working().await;
    assert_eq!(memory.working_count().await, 0);
}

#[tokio::test]
async fn test_agent_memory_working_overflow_trims() {
    let memory = AgentMemory {
        store: Arc::new(InMemoryStore::new()),
        short_term: Arc::new(RwLock::new(VecDeque::new())),
        working: Arc::new(RwLock::new(Vec::new())),
        max_short_term: 100,
        max_working: 3,
        relevance_config: RelevanceConfig::default(),
        llm_extraction: false,
        llm_extraction_max_items: 5,
        llm_extraction_max_input_chars: 8_000,
        semantic_usage_judge: None,
        extraction_queue: Arc::new(MemoryExtractionQueue::default()),
        observers: Arc::new(Vec::new()),
        durable_memory: None,
        prune_policy: None,
        prune_interval: std::time::Duration::from_secs(3600),
        maintenance_claimed: Arc::new(AtomicBool::new(false)),
    };
    for i in 0..5 {
        memory
            .add_to_working(MemoryItem::new(format!("task {i}")).with_importance(i as f32 * 0.2))
            .await
            .unwrap();
    }
    assert_eq!(memory.get_working().await.len(), 3);
}

#[tokio::test]
async fn test_agent_memory_recall_by_tags() {
    let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
    memory
        .remember_success("create file", &["write".to_string()], "ok")
        .await
        .unwrap();
    memory
        .remember_failure("delete file", "denied", &["bash".to_string()])
        .await
        .unwrap();

    let successes = memory
        .recall_by_tags(&["success".to_string()], 10)
        .await
        .unwrap();
    assert_eq!(successes.len(), 1);
    let failures = memory
        .recall_by_tags(&["failure".to_string()], 10)
        .await
        .unwrap();
    assert_eq!(failures.len(), 1);
}

#[tokio::test]
async fn test_agent_memory_short_term_trim() {
    let store = Arc::new(InMemoryStore::new());
    let memory = AgentMemory {
        store,
        short_term: Arc::new(RwLock::new(VecDeque::new())),
        working: Arc::new(RwLock::new(Vec::new())),
        max_short_term: 3,
        max_working: 10,
        relevance_config: RelevanceConfig::default(),
        llm_extraction: false,
        llm_extraction_max_items: 5,
        llm_extraction_max_input_chars: 8_000,
        semantic_usage_judge: None,
        extraction_queue: Arc::new(MemoryExtractionQueue::default()),
        observers: Arc::new(Vec::new()),
        durable_memory: None,
        prune_policy: None,
        prune_interval: std::time::Duration::from_secs(3600),
        maintenance_claimed: Arc::new(AtomicBool::new(false)),
    };
    for i in 0..5 {
        memory
            .remember(MemoryItem::new(format!("item {i}")))
            .await
            .unwrap();
    }
    assert_eq!(memory.short_term_count().await, 3);
}

#[tokio::test]
async fn superseded_memory_is_preserved_but_excluded_from_recall() {
    let store = Arc::new(InMemoryStore::new());
    let memory = AgentMemory::new(store.clone());
    let old = memory
        .remember_item(MemoryItem::new(
            "The service listens on the obsolete port 3000",
        ))
        .await
        .unwrap();
    let replacement = memory
        .remember_item(MemoryItem::new("The service now listens on port 4000"))
        .await
        .unwrap();

    assert!(memory
        .mark_superseded(&old.id, &replacement.id)
        .await
        .unwrap());
    assert_eq!(store.count().await.unwrap(), 2);
    let archived = store.retrieve(&old.id).await.unwrap().unwrap();
    assert_eq!(
        archived
            .metadata
            .get(MEMORY_STATUS_METADATA)
            .map(String::as_str),
        Some(MEMORY_STATUS_SUPERSEDED)
    );
    assert_eq!(
        archived.metadata.get("superseded_by").map(String::as_str),
        Some(replacement.id.as_str())
    );
    assert!(archived.metadata.contains_key("protected"));
    assert!(memory
        .recall_similar("obsolete port 3000", 10)
        .await
        .unwrap()
        .iter()
        .all(|item| item.id != old.id));
    assert!(memory
        .get_short_term()
        .await
        .iter()
        .all(|item| item.id != old.id));
}

#[tokio::test]
async fn test_agent_memory_prune_delegates() {
    use a3s_memory::PrunePolicy;

    let store = Arc::new(InMemoryStore::new());
    let memory = AgentMemory::new(store.clone());

    // Insert one old low-importance item directly into the store.
    let mut old_item = a3s_memory::MemoryItem::new("stale").with_importance(0.2);
    old_item.timestamp = chrono::Utc::now() - chrono::Duration::days(100);
    store.store(old_item).await.unwrap();

    assert_eq!(store.count().await.unwrap(), 1);

    // Calling prune on the underlying store via the public accessor works.
    let policy = PrunePolicy {
        max_age_days: 90,
        min_importance_to_keep: 0.5,
        max_items: 0,
    };
    let deleted = memory.store().prune(&policy).await.unwrap();
    assert_eq!(deleted, 1);
    assert_eq!(store.count().await.unwrap(), 0);
}

#[test]
fn test_agent_memory_score_uses_config() {
    let config = MemoryConfig {
        relevance: RelevanceConfig {
            decay_days: 7.0,
            importance_weight: 0.9,
            recency_weight: 0.1,
        },
        ..Default::default()
    };
    let memory = AgentMemory::with_config(Arc::new(InMemoryStore::new()), config);
    let item = MemoryItem::new("Test").with_importance(1.0);
    let score = memory.score(&item, Utc::now());
    assert!(score > 0.95, "Score was {score}");
}

#[test]
fn test_memory_config_partial_deserialize_keeps_llm_extraction_enabled() {
    let config: MemoryConfig = serde_json::from_str(r#"{"maxShortTerm": 12}"#).unwrap();
    assert!(config.llm_extraction);
    assert_eq!(config.max_short_term, 12);
}

#[test]
fn test_memory_config_allows_explicit_llm_extraction_disable() {
    let config: MemoryConfig =
        serde_json::from_str(r#"{"llmExtraction": false, "maxShortTerm": 12}"#).unwrap();
    assert!(!config.llm_extraction);
    assert_eq!(config.max_short_term, 12);
}

#[test]
fn test_memory_context_result_includes_relation_context() {
    let item = MemoryItem::new("Use the file memory store for local sessions.")
        .with_type(MemoryType::Procedural)
        .with_tag("consolidated")
        .with_tag("conflict")
        .with_metadata("supersedes", "old-preference, old-workflow")
        .with_metadata("conflicts_with", "legacy-default");

    let result = memory_items_to_context_result("memory", vec![item.clone()]);

    assert_eq!(result.items.len(), 1);
    let context_item = &result.items[0];
    assert!(context_item
        .content
        .contains("Use the file memory store for local sessions."));
    assert!(context_item.content.contains("Memory relations:"));
    assert!(context_item
        .content
        .contains("supersedes: memory://old-preference, memory://old-workflow"));
    assert!(context_item
        .content
        .contains("conflicts_with: memory://legacy-default"));
    assert_eq!(
        context_item.metadata.get("memory_id"),
        Some(&serde_json::json!(item.id))
    );
    assert_eq!(
        context_item.metadata.get("memory_type"),
        Some(&serde_json::json!("procedural"))
    );
    assert_eq!(
        context_item.metadata.get("tags"),
        Some(&serde_json::json!(["consolidated", "conflict"]))
    );
    assert_eq!(
        context_item.metadata.get("supersedes"),
        Some(&serde_json::json!(["old-preference", "old-workflow"]))
    );
    assert_eq!(
        context_item.metadata.get("conflicts_with"),
        Some(&serde_json::json!(["legacy-default"]))
    );
    assert_eq!(
        context_item.token_count,
        (context_item.content.len() / 4).max(1)
    );
}

#[test]
fn test_memory_context_relevance_preserves_recall_order() {
    let top_match = MemoryItem::new("Run focused memory extraction tests after parser changes.")
        .with_importance(0.2)
        .with_type(MemoryType::Procedural);
    let generic_high_importance = MemoryItem::new("Remember general memory behavior.")
        .with_importance(1.0)
        .with_type(MemoryType::Semantic);

    let result = memory_items_to_context_result("memory", vec![top_match, generic_high_importance]);

    assert_eq!(result.items.len(), 2);
    assert!(
        result.items[0].relevance > result.items[1].relevance,
        "search recall order should remain a strong memory context ranking signal"
    );
}

#[tokio::test]
async fn test_memory_context_provider_does_not_mechanically_store_turns() {
    let memory = AgentMemory::new(Arc::new(InMemoryStore::new()));
    let provider = MemoryContextProvider::new(memory.clone());

    provider
        .on_turn_complete("session-1", "remember nothing", "ok")
        .await
        .unwrap();

    assert_eq!(memory.stats().await.unwrap().long_term_count, 0);
}

struct NoopMaintenanceJob;

#[async_trait::async_trait]
impl MemoryMaintenanceJob for NoopMaintenanceJob {
    async fn run(
        &self,
        _context: &MemoryMaintenanceContext,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<MemoryMaintenanceOutcome> {
        Ok(MemoryMaintenanceOutcome::new(0))
    }
}

#[tokio::test(start_paused = true)]
async fn usage_consolidation_merges_near_duplicates_and_leaves_distinct_items() {
    let store = Arc::new(InMemoryStore::new());
    let memory = Arc::new(AgentMemory::new(store.clone()));

    let mut canonical = MemoryItem::new(
        "The release pipeline deploys the staging environment every Tuesday morning",
    )
    .with_importance(0.9);
    canonical
        .metadata
        .insert("usage_count".to_string(), "1".to_string());
    let mut twin = MemoryItem::new(
        "The release pipeline deploys the staging environment each Tuesday morning",
    )
    .with_importance(0.4);
    twin.metadata
        .insert("usage_count".to_string(), "2".to_string());
    let distinct = MemoryItem::new("Rust toolchain for the workspace stays pinned at 1.97.1")
        .with_importance(0.6);
    let (canonical_id, twin_id, distinct_id) =
        (canonical.id.clone(), twin.id.clone(), distinct.id.clone());
    store.store(canonical).await.unwrap();
    store.store(twin).await.unwrap();
    store.store(distinct).await.unwrap();

    let runtime = MemoryMaintenanceRuntime::start(
        "usage-consolidation-owner",
        memory,
        MemoryMaintenanceOptions::new().with_usage_consolidation(std::time::Duration::from_secs(1)),
    )
    .unwrap();

    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    let health = runtime.health();
    let job = health
        .jobs
        .iter()
        .find(|job| job.name == "v1_usage_consolidation")
        .expect("built-in usage consolidation job is registered");
    assert_eq!(job.successful_runs, 1);
    assert_eq!(job.last_affected_items, Some(1));
    assert!(job.last_error.is_none());

    assert!(
        store.retrieve(&twin_id).await.unwrap().is_none(),
        "the lower-importance near-duplicate is deleted"
    );
    let survivor = store
        .retrieve(&canonical_id)
        .await
        .unwrap()
        .expect("the canonical twin survives");
    assert_eq!(
        survivor.metadata.get("usage_count").map(String::as_str),
        Some("3"),
        "the twin's usage counter merged into the survivor"
    );
    let untouched = store
        .retrieve(&distinct_id)
        .await
        .unwrap()
        .expect("the distinct item survives untouched");
    assert_eq!(
        untouched.metadata.get("usage_count").map(String::as_str),
        None,
        "distinct items neither gain usage nor merge"
    );
    assert_eq!(store.count().await.unwrap(), 2);
    assert!(runtime.close().await.is_clean());
}

#[tokio::test(start_paused = true)]
async fn usage_consolidation_never_deletes_curation_protected_twins() {
    let store = Arc::new(InMemoryStore::new());
    let memory = Arc::new(AgentMemory::new(store.clone()));

    let pinned = MemoryItem::new(
        "The release pipeline deploys the staging environment each Tuesday morning",
    )
    .with_importance(0.4)
    .with_tag("pinned");
    let pinned_id = pinned.id.clone();
    let duplicate = MemoryItem::new(
        "The release pipeline deploys the staging environment every Tuesday morning",
    )
    .with_importance(0.9);
    store.store(pinned).await.unwrap();
    store.store(duplicate).await.unwrap();

    let runtime = MemoryMaintenanceRuntime::start(
        "protected-consolidation-owner",
        memory,
        MemoryMaintenanceOptions::new().with_usage_consolidation(std::time::Duration::from_secs(1)),
    )
    .unwrap();
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }

    assert!(
        store.retrieve(&pinned_id).await.unwrap().is_some(),
        "a pinned near-duplicate is never absorbed"
    );
    assert_eq!(store.count().await.unwrap(), 2);
    assert!(runtime.close().await.is_clean());
}

#[tokio::test]
async fn usage_consolidation_is_opt_in_and_reserves_its_job_name() {
    // Prune defaults ON (product decision 2026-10-08), so a default-config
    // memory carries the prune schedule; only with prune explicitly off do
    // empty options leave maintenance with nothing to run.
    let prune_off = MemoryConfig {
        prune_policy: None,
        ..MemoryConfig::default()
    };
    let memory = Arc::new(AgentMemory::with_config(
        Arc::new(InMemoryStore::new()),
        prune_off,
    ));

    // Without the built-in and without host jobs, maintenance stays disabled.
    assert!(matches!(
        MemoryMaintenanceRuntime::start(
            "gating-owner",
            memory.clone(),
            MemoryMaintenanceOptions::new()
        ),
        Err(MemoryMaintenanceError::NoJobsConfigured)
    ));

    // A default-config memory with empty options registers exactly the
    // default prune job — usage consolidation stays the explicit opt-in.
    let default_memory = Arc::new(AgentMemory::new(Arc::new(InMemoryStore::new())));
    let runtime = MemoryMaintenanceRuntime::start(
        "prune-default-owner",
        default_memory,
        MemoryMaintenanceOptions::new(),
    )
    .unwrap();
    let names: Vec<String> = runtime
        .health()
        .jobs
        .iter()
        .map(|job| job.name.clone())
        .collect();
    assert_eq!(names, vec!["v1_prune".to_string()]);
    assert!(runtime.close().await.is_clean());

    // The built-in name is reserved exactly like the prune job's.
    let stolen = ScheduledMemoryMaintenance::try_new(
        "v1_usage_consolidation",
        std::time::Duration::from_secs(1),
        Arc::new(NoopMaintenanceJob),
    )
    .unwrap();
    assert!(matches!(
        MemoryMaintenanceRuntime::start(
            "gating-owner",
            memory.clone(),
            MemoryMaintenanceOptions::new().with_job(stolen)
        ),
        Err(MemoryMaintenanceError::InvalidConfiguration { field, .. }) if field == "jobs.name"
    ));

    // An invalid interval fails before the runtime starts.
    assert!(matches!(
        MemoryMaintenanceRuntime::start(
            "gating-owner",
            memory,
            MemoryMaintenanceOptions::new()
                .with_usage_consolidation(std::time::Duration::ZERO)
        ),
        Err(MemoryMaintenanceError::InvalidConfiguration { field, .. }) if field == "job.interval"
    ));
}

/// A scripted judge: verdicts are queued per call; every call is counted.
struct ScriptedJudge {
    verdicts: Mutex<Vec<anyhow::Result<bool>>>,
    calls: std::sync::atomic::AtomicUsize,
}

impl ScriptedJudge {
    fn new(verdicts: Vec<anyhow::Result<bool>>) -> Arc<Self> {
        Arc::new(Self {
            verdicts: Mutex::new(verdicts),
            calls: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    fn call_count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait]
impl ConsolidationJudge for ScriptedJudge {
    async fn same_fact(&self, _left: &MemoryItem, _right: &MemoryItem) -> anyhow::Result<bool> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.verdicts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
            .unwrap_or(Ok(false))
    }
}

/// Seed a three-item store whose first two items are lexical near-duplicates.
async fn judged_consolidation_store() -> (Arc<InMemoryStore>, Arc<AgentMemory>, String, String) {
    let store = Arc::new(InMemoryStore::new());
    let memory = Arc::new(AgentMemory::new(store.clone()));
    let canonical = MemoryItem::new(
        "The release pipeline deploys the staging environment every Tuesday morning",
    )
    .with_importance(0.9);
    let twin = MemoryItem::new(
        "The release pipeline deploys the staging environment each Tuesday morning",
    )
    .with_importance(0.4);
    let (canonical_id, twin_id) = (canonical.id.clone(), twin.id.clone());
    store.store(canonical).await.unwrap();
    store.store(twin).await.unwrap();
    (store, memory, canonical_id, twin_id)
}

#[tokio::test]
async fn a_judged_consolidation_merges_only_confirmed_pairs() {
    let (store, memory, canonical_id, twin_id) = judged_consolidation_store().await;
    let judge = ScriptedJudge::new(vec![Ok(true)]);

    let runtime = MemoryMaintenanceRuntime::start(
        "judged-consolidation-owner",
        memory,
        MemoryMaintenanceOptions::new()
            .with_usage_consolidation(std::time::Duration::from_secs(1))
            .with_consolidation_judge(judge.clone()),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while judge.call_count() == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(runtime.close().await.is_clean());

    assert_eq!(
        judge.call_count(),
        1,
        "the lexical recall asked the judge once"
    );
    assert!(
        store.retrieve(&twin_id).await.unwrap().is_none(),
        "a judge-confirmed pair still merges"
    );
    assert!(store.retrieve(&canonical_id).await.unwrap().is_some());
}

#[tokio::test]
async fn a_judge_rejection_or_error_skips_the_pair_without_merging() {
    // A rejection: lexical near-duplicates that are NOT the same fact must
    // survive separately when the judge says DIFFERENT.
    let (store, memory, canonical_id, twin_id) = judged_consolidation_store().await;
    let judge = ScriptedJudge::new(vec![Ok(false)]);
    let runtime = MemoryMaintenanceRuntime::start(
        "judged-rejection-owner",
        memory,
        MemoryMaintenanceOptions::new()
            .with_usage_consolidation(std::time::Duration::from_secs(1))
            .with_consolidation_judge(judge.clone()),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while judge.call_count() == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(runtime.close().await.is_clean());
    assert_eq!(judge.call_count(), 1);
    assert!(
        store.retrieve(&twin_id).await.unwrap().is_some(),
        "a rejected pair never merges"
    );
    assert!(store.retrieve(&canonical_id).await.unwrap().is_some());

    // An erroring judge is equally fail-closed.
    let (store, memory, canonical_id, twin_id) = judged_consolidation_store().await;
    let judge = ScriptedJudge::new(vec![Err(anyhow::anyhow!("model unavailable"))]);
    let runtime = MemoryMaintenanceRuntime::start(
        "judged-error-owner",
        memory,
        MemoryMaintenanceOptions::new()
            .with_usage_consolidation(std::time::Duration::from_secs(1))
            .with_consolidation_judge(judge.clone()),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while judge.call_count() == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(runtime.close().await.is_clean());
    assert_eq!(
        judge.call_count(),
        1,
        "the error path was actually exercised"
    );
    assert!(store.retrieve(&twin_id).await.unwrap().is_some());
    assert!(store.retrieve(&canonical_id).await.unwrap().is_some());
}

#[test]
fn llm_judge_classifies_the_closed_set_and_refuses_anything_else() {
    use crate::llm::{LlmClient, LlmResponse, Message, ToolDefinition};

    struct VerdictClient(&'static str);
    #[async_trait]
    impl LlmClient for VerdictClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> anyhow::Result<LlmResponse> {
            Ok(LlmResponse {
                message: Message::assistant(self.0),
                usage: Default::default(),
                stop_reason: None,
                token_logprobs: Vec::new(),
                meta: None,
            })
        }
        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
            _cancel_token: tokio_util::sync::CancellationToken,
        ) -> anyhow::Result<tokio::sync::mpsc::Receiver<crate::llm::StreamEvent>> {
            anyhow::bail!("streaming not used by the judge")
        }
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let left = MemoryItem::new("deploys staging every Tuesday");
    let right = MemoryItem::new("deploys staging each Tuesday");

    let same = LlmConsolidationJudge::new(Arc::new(VerdictClient("SAME.")));
    assert!(runtime
        .block_on(ConsolidationJudge::same_fact(&same, &left, &right))
        .unwrap());
    let decorated = LlmConsolidationJudge::new(Arc::new(VerdictClient(
        "DIFFERENT — they cover different weeks",
    )));
    assert!(!runtime
        .block_on(ConsolidationJudge::same_fact(&decorated, &left, &right))
        .unwrap());
    let garbage = LlmConsolidationJudge::new(Arc::new(VerdictClient("probably?")));
    assert!(
        runtime
            .block_on(ConsolidationJudge::same_fact(&garbage, &left, &right))
            .is_err(),
        "a non-closed-set verdict is an error, and errors skip the pair"
    );
    // "NOT SAME" must never classify as SAME via the prefix rule.
    let negated = LlmConsolidationJudge::new(Arc::new(VerdictClient("NOT SAME")));
    assert!(runtime
        .block_on(ConsolidationJudge::same_fact(&negated, &left, &right))
        .is_err(),);
}
