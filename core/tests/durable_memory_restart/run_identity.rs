use super::*;

#[tokio::test]
async fn resumed_session_rejects_a_retained_run_id_collision_without_overwrite() {
    let repository_root = tempfile::tempdir().unwrap();
    let session_root = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let namespace = MemoryNamespace::try_new("tenant", "principal", "workspace-a").unwrap();
    let repository = Arc::new(
        FileMemoryRepository::open(repository_root.path())
            .await
            .unwrap(),
    );
    repository
        .apply(MemoryChangeSet::new(
            "create-active-restart-collision-node",
            namespace.clone(),
            time(18),
            vec![MemoryOperation::Create {
                node: MemoryNodeDraft::new(
                    NODE_ID,
                    namespace.clone(),
                    DurableMemoryKind::Procedural,
                    MemoryStatus::Active,
                    MEMORY_CONTENT,
                    vec![evidence(
                        "a3s://verification/restart-collision",
                        EvidenceKind::Verification,
                        18,
                    )],
                    time(18),
                ),
            }],
        ))
        .await
        .unwrap();
    let binding =
        DurableMemorySession::active_recall(repository.clone(), namespace.clone(), recall_policy());
    let session_store = Arc::new(FileSessionStore::new(session_root.path()).await.unwrap());

    let first_client = Arc::new(InspectingClient::new());
    let first_agent = Agent::from_config(offline_config()).await.unwrap();
    let first = first_agent
        .session_async(
            workspace.path().display().to_string(),
            Some(
                session_options(
                    session_store.clone(),
                    first_client.clone(),
                    Some(binding.clone()),
                    host_env("retained-collision", 19),
                )
                .with_session_id(SESSION_ID),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        first.send(QUERY, None).await.unwrap().text,
        "MEMORY_VISIBLE"
    );
    first.save().await.unwrap();
    let original = first.runs().await.pop().unwrap();
    assert_eq!(first_client.observations(), vec![true]);
    first.close().await;
    drop(first);
    first_agent.close().await;
    drop(first_agent);
    drop(binding);
    drop(repository);
    drop(session_store);

    let repository = Arc::new(
        FileMemoryRepository::open(repository_root.path())
            .await
            .unwrap(),
    );
    let binding =
        DurableMemorySession::active_recall(repository.clone(), namespace.clone(), recall_policy());
    let session_store = Arc::new(FileSessionStore::new(session_root.path()).await.unwrap());
    let resumed_client = Arc::new(InspectingClient::new());
    let resumed_agent = Agent::from_config(offline_config()).await.unwrap();
    let resumed = resumed_agent
        .resume_session_async(
            SESSION_ID,
            session_options(
                session_store,
                resumed_client.clone(),
                Some(binding),
                host_env("retained-collision", 19),
            ),
        )
        .await
        .unwrap();
    let error = resumed.send(QUERY, None).await.unwrap_err();
    assert!(matches!(error, CodeError::RunIdentityConflict { .. }));
    assert!(resumed_client.observations().is_empty());
    let retained = resumed.runs().await;
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].id, original.id);
    assert_eq!(retained[0].result_text, original.result_text);
    assert_eq!(retained[0].event_count, original.event_count);
    assert_eq!(
        repository
            .usage_summary(&namespace, NODE_ID)
            .await
            .unwrap()
            .admissions,
        1
    );
    resumed.close().await;
    drop(resumed);
    resumed_agent.close().await;
}
