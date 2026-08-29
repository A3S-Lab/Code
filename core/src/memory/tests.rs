use super::*;
use crate::context::ContextProvider;
use a3s_memory::InMemoryStore;
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
