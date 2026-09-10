use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn test_session_with_memory_store() {
    use a3s_memory::InMemoryStore;
    let store = Arc::new(InMemoryStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_memory(store);
    let session = agent
        .session_async("/tmp/test-ws-memory", Some(opts))
        .await
        .unwrap();
    assert!(session.memory().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_without_memory_store() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-no-memory", None)
        .await
        .unwrap();
    assert!(
        session.memory().is_some(),
        "sessions should have a default memory store"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_memory_wired_into_config() {
    use a3s_memory::InMemoryStore;
    let store = Arc::new(InMemoryStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_memory(store);
    let session = agent
        .session_async("/tmp/test-ws-mem-config", Some(opts))
        .await
        .unwrap();
    // memory is accessible via the public session API
    assert!(session.memory().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_memory_uses_code_config_limits() {
    use a3s_memory::{InMemoryStore, MemoryItem};

    let mut config = test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        max_short_term: 1,
        ..Default::default()
    });
    let store = Arc::new(InMemoryStore::new());
    let agent = Agent::from_config(config).await.unwrap();
    let opts = SessionOptions::new().with_memory(store);
    let session = agent
        .session_async("/tmp/test-ws-mem-config-limits", Some(opts))
        .await
        .unwrap();

    let memory = session.memory().unwrap();
    memory.remember(MemoryItem::new("one")).await.unwrap();
    memory.remember(MemoryItem::new("two")).await.unwrap();

    assert_eq!(memory.short_term_count().await, 1);
    assert_eq!(memory.stats().await.unwrap().long_term_count, 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_with_file_memory() {
    let dir = tempfile::TempDir::new().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_file_memory(dir.path());
    let session = agent
        .session_async("/tmp/test-ws-file-mem", Some(opts))
        .await
        .unwrap();
    assert!(session.memory().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_uses_configured_default_memory_dir() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut config = test_config();
    config.memory_dir = Some(dir.path().join("memory"));
    let agent = Agent::from_config(config).await.unwrap();

    let session = agent
        .session_async("/tmp/test-ws-default-file-mem", None)
        .await
        .unwrap();
    let memory = session.memory().expect("default memory store");
    memory
        .remember(a3s_memory::MemoryItem::new("configured default memory dir"))
        .await
        .unwrap();

    assert!(dir.path().join("memory/index.json").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_file_memory_initialization_failure_is_typed() {
    let dir = tempfile::TempDir::new().unwrap();
    let blocked_path = dir.path().join("blocked-memory");
    std::fs::write(&blocked_path, "not a directory").unwrap();

    let mut config = test_config();
    config.memory_dir = Some(blocked_path.clone());
    let agent = Agent::from_config(config).await.unwrap();

    let error = agent
        .session_async("/tmp/test-ws-memory-fallback", None)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::SessionInitialization {
            resource: crate::error::SessionBuildResource::MemoryStore,
            ..
        }
    ));
    assert!(blocked_path.is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_memory_remember_and_recall() {
    use a3s_memory::InMemoryStore;
    let store = Arc::new(InMemoryStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let opts = SessionOptions::new().with_memory(store);
    let session = agent
        .session_async("/tmp/test-ws-mem-recall", Some(opts))
        .await
        .unwrap();

    let memory = session.memory().unwrap();
    memory
        .remember_success("write a file", &["write".to_string()], "done")
        .await
        .unwrap();

    let results = memory.recall_similar("write", 5).await.unwrap();
    assert!(!results.is_empty());
    let stats = memory.stats().await.unwrap();
    assert_eq!(stats.long_term_count, 1);
}

// ========================================================================
// Tool timeout tests
// ========================================================================
