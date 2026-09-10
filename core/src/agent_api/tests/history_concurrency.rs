use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn test_concurrent_history_reads() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .session_async("/tmp/test-ws-concurrent", None)
            .await
            .unwrap(),
    );

    let handles: Vec<_> = (0..10)
        .map(|_| {
            let s = Arc::clone(&session);
            tokio::spawn(async move { s.history().len() })
        })
        .collect();

    for h in handles {
        h.await.unwrap();
    }
}

// ========================================================================
// init_warning tests
// ========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn test_session_no_init_warning_without_file_memory() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-ws-no-warn", None)
        .await
        .unwrap();
    assert!(session.init_warning().is_none());
}
