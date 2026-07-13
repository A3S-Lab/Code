use super::tests::test_config;
use super::*;
use crate::store::MemorySessionStore;

fn replacement_options(store: Arc<MemorySessionStore>, model: &str) -> SessionOptions {
    SessionOptions::new()
        .with_session_store(store)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_model(model)
}

#[tokio::test]
async fn replacement_is_atomic_and_keeps_the_session_id() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let session_id = "atomic-session-replacement";
    let current = agent
        .session_async(
            "/tmp/atomic-session-replacement",
            Some(
                replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                    .with_session_id(session_id),
            ),
        )
        .await
        .unwrap();

    let replacement = agent
        .replace_session_async(
            &current,
            replacement_options(Arc::clone(&store), "openai/gpt-4o"),
        )
        .await
        .unwrap();

    assert!(current.is_closed());
    assert!(!replacement.is_closed());
    assert_eq!(replacement.session_id(), session_id);
    assert_eq!(replacement.model_name, "openai/gpt-4o");
    assert_eq!(agent.list_sessions().await, vec![session_id.to_string()]);
}

#[tokio::test]
async fn failed_replacement_leaves_the_current_session_live() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let store = Arc::new(MemorySessionStore::new());
    let session_id = "failed-session-replacement";
    let current = agent
        .session_async(
            "/tmp/failed-session-replacement",
            Some(
                replacement_options(Arc::clone(&store), "anthropic/claude-sonnet-4-20250514")
                    .with_session_id(session_id),
            ),
        )
        .await
        .unwrap();

    let error = agent
        .replace_session_async(
            &current,
            SessionOptions::new()
                .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                .with_model("openai/gpt-4o"),
        )
        .await
        .expect_err("replacement without a session store must fail");

    assert!(error.to_string().contains("session_store"));
    assert!(!current.is_closed());
    current.save().await.unwrap();
    assert_eq!(agent.list_sessions().await, vec![session_id.to_string()]);
}
