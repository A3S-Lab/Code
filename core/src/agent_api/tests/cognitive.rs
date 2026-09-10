use super::*;

#[tokio::test(flavor = "current_thread")]
async fn blank_session_id_returns_typed_configuration_error() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let error = agent
        .session_async(
            "/tmp/test-blank-session-id",
            Some(SessionOptions::new().with_session_id("  \t")),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::SessionConfiguration {
            field: "session_id",
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn cognitive_package_binding_is_persisted_and_retained_by_run_events() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (cognitive_context, provider) = test_cognitive_context(TestCognitiveProviderMode::Valid);
    let expected = cognitive_context.binding().clone();
    let opts = SessionOptions::new()
        .with_session_id("cognitive-session")
        .with_session_store(store.clone())
        .with_llm_client(Arc::new(StaticStreamingClient::new("grounded response")))
        .with_planning(false)
        .with_cognitive_context(cognitive_context);
    let session = agent
        .session_async("/tmp/test-cognitive-session", Some(opts))
        .await
        .unwrap();

    assert_eq!(session.cognitive_package_binding(), Some(&expected));
    session
        .send("What is the retry policy?", None)
        .await
        .unwrap();
    session.save().await.unwrap();

    let requests = provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].session_id, "cognitive-session");
    assert_eq!(requests[0].binding, expected);

    let snapshot = store
        .load_snapshot("cognitive-session")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        snapshot.session.cognitive_package_binding.as_ref(),
        Some(&expected)
    );
    let bound_events = snapshot
        .run_records
        .iter()
        .flat_map(|record| &record.events)
        .filter_map(|record| match &record.event {
            AgentEvent::CognitiveContextBound { binding } => Some(binding),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(bound_events, vec![&expected]);
    assert!(!snapshot
        .run_records
        .iter()
        .flat_map(|record| &record.events)
        .any(|record| matches!(record.event, AgentEvent::MemoryRecalled { .. })));
    snapshot.validate_for_session("cognitive-session").unwrap();

    let mut tampered = snapshot.clone();
    let event_binding = tampered
        .run_records
        .iter_mut()
        .flat_map(|record| &mut record.events)
        .find_map(|record| match &mut record.event {
            AgentEvent::CognitiveContextBound { binding } => Some(binding),
            _ => None,
        })
        .unwrap();
    event_binding.limits.max_results -= 1;
    assert!(tampered.validate_for_session("cognitive-session").is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn cognitive_provider_failure_and_generation_drift_fail_closed_without_fallback() {
    for (session_id, mode) in [
        (
            "cognitive-provider-failure",
            TestCognitiveProviderMode::Fail,
        ),
        (
            "cognitive-provider-drift",
            TestCognitiveProviderMode::DriftGeneration,
        ),
    ] {
        let agent = Agent::from_config(test_config()).await.unwrap();
        let (cognitive_context, _) = test_cognitive_context(mode);
        let opts = SessionOptions::new()
            .with_session_id(session_id)
            .with_llm_client(Arc::new(StaticStreamingClient::new(
                "this response must never be reached",
            )))
            .with_planning(false)
            .with_cognitive_context(cognitive_context);
        let session = agent
            .session_async(format!("/tmp/{session_id}"), Some(opts))
            .await
            .unwrap();

        let error = session
            .send("Use the package evidence", None)
            .await
            .expect_err("required cognitive context must not be omitted");
        assert!(error.to_string().contains("failed closed"), "{error:#}");
        let records = session.run_store.records().await;
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].snapshot.status, crate::run::RunStatus::Failed);
        assert!(records[0]
            .events
            .iter()
            .any(|record| matches!(record.event, AgentEvent::CognitiveContextBound { .. })));
        assert!(!records[0].events.iter().any(|record| matches!(
            record.event,
            AgentEvent::MemoryRecalled { .. } | AgentEvent::End { .. }
        )));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cognitive_session_rejects_general_rag_or_graph_provider_fallback() {
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (cognitive_context, _) = test_cognitive_context(TestCognitiveProviderMode::Valid);
    let error = agent
        .session_async(
            "/tmp/test-cognitive-no-generic-fallback",
            Some(
                SessionOptions::new()
                    .with_session_id("cognitive-no-generic-fallback")
                    .with_llm_client(Arc::new(StaticStreamingClient::new("unused")))
                    .with_cognitive_context(cognitive_context)
                    .with_context_provider(Arc::new(CapturingContextProvider::default())),
            ),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::SessionConfiguration {
            field: "cognitive_context",
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn cognitive_session_resume_requires_the_same_host_injected_binding() {
    let store = Arc::new(crate::store::MemorySessionStore::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let (cognitive_context, _) = test_cognitive_context(TestCognitiveProviderMode::Valid);
    let expected = cognitive_context.binding().clone();
    let session = agent
        .session_async(
            "/tmp/test-cognitive-resume",
            Some(
                SessionOptions::new()
                    .with_session_id("cognitive-resume")
                    .with_session_store(store.clone())
                    .with_llm_client(Arc::new(StaticStreamingClient::new("unused")))
                    .with_cognitive_context(cognitive_context),
            ),
        )
        .await
        .unwrap();
    session.save().await.unwrap();
    session.close().await;
    drop(session);

    let missing = agent
        .resume_session_async(
            "cognitive-resume",
            SessionOptions::new()
                .with_session_store(store.clone())
                .with_llm_client(Arc::new(StaticStreamingClient::new("unused"))),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        missing,
        crate::error::CodeError::SessionConfiguration {
            field: "cognitive_context",
            ..
        }
    ));

    let mut different_binding = expected.clone();
    different_binding.limits.max_results -= 1;
    different_binding.validate().unwrap();
    let drift_provider: Arc<dyn crate::cognitive_context::CognitiveContextProvider> =
        Arc::new(TestCognitiveProvider::new(TestCognitiveProviderMode::Valid));
    let drift_context =
        crate::cognitive_context::CognitiveContextSession::new(different_binding, drift_provider)
            .unwrap();
    let drift = agent
        .resume_session_async(
            "cognitive-resume",
            SessionOptions::new()
                .with_session_store(store.clone())
                .with_llm_client(Arc::new(StaticStreamingClient::new("unused")))
                .with_cognitive_context(drift_context),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        drift,
        crate::error::CodeError::SessionConfiguration {
            field: "cognitive_context",
            ..
        }
    ));

    let (exact_context, _) = test_cognitive_context(TestCognitiveProviderMode::Valid);
    let resumed = agent
        .resume_session_async(
            "cognitive-resume",
            SessionOptions::new()
                .with_session_store(store)
                .with_llm_client(Arc::new(StaticStreamingClient::new("unused")))
                .with_cognitive_context(exact_context),
        )
        .await
        .unwrap();
    assert_eq!(resumed.cognitive_package_binding(), Some(&expected));
}
