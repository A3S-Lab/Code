#![cfg(test)]

use super::*;

enum RecoveryIngress {
    Direct,
    StoredEvidence,
    SuppliedCheckpoint,
    PreparedAdmission,
}

async fn assert_source_target_alias_refused(file_backed: bool) {
    let store_root = tempfile::tempdir().unwrap();
    let store: Arc<dyn crate::store::SessionStore> = if file_backed {
        Arc::new(
            crate::store::FileSessionStore::new(store_root.path())
                .await
                .unwrap(),
        )
    } else {
        Arc::new(crate::store::MemorySessionStore::new())
    };
    let mut fixture = CacheCoverageFixture::new().await;
    fixture.session.session_store = Some(Arc::clone(&store));
    fixture.append(&fixture.original());
    let source_id = fixture.checkpoint.run_id.clone();
    store
        .save_loop_checkpoint(&source_id, &fixture.checkpoint)
        .await
        .unwrap();
    fixture.pinned.close().await;
    fixture.session.save().await.unwrap();
    fixture.session.close().await;
    let session_id = fixture.session.session_id().to_owned();
    drop(fixture.session);
    let reopened_store: Arc<dyn crate::store::SessionStore> = if file_backed {
        drop(store);
        Arc::new(
            crate::store::FileSessionStore::new(store_root.path())
                .await
                .unwrap(),
        )
    } else {
        store
    };
    let source_bytes = serde_json::to_vec(
        &reopened_store
            .load_loop_checkpoint(&source_id)
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let source_file = file_backed.then(|| {
        let mut entries = std::fs::read_dir(store_root.path().join("v1").join("loop_checkpoints"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        let path = entries.pop().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        (path, bytes)
    });
    let agent = crate::Agent::from_config(crate::agent_api::tests::test_config())
        .await
        .unwrap();
    let mut session = agent
        .resume_session_async(
            &session_id,
            crate::SessionOptions::new().with_session_store(Arc::clone(&reopened_store)),
        )
        .await
        .unwrap();
    let client = Arc::new(DetachedAdmissionClient::new(false));
    session.llm_client = client.clone();
    // A checkpoint on disk must not impersonate a RunStore reservation. This
    // is the reopened-Session condition under which S could become target S.
    assert!(session.run_snapshot(&source_id).await.is_none());
    let original = fixture.log.read(&fixture.thread).unwrap();
    let evidence = SessionLogicalResumeEvidenceV1::from_checkpoint(&fixture.checkpoint).unwrap();
    let checkpoint_identity = evidence.content_digest.as_str();

    for ingress in [
        RecoveryIngress::Direct,
        RecoveryIngress::StoredEvidence,
        RecoveryIngress::SuppliedCheckpoint,
        RecoveryIngress::PreparedAdmission,
    ] {
        let error = match ingress {
            RecoveryIngress::Direct => match session
                .spawn_recovery_with_run_id(&source_id, &source_id)
                .await
            {
                Err(error) => error,
                Ok(_) => panic!("direct recovery admitted its immutable source as target"),
            },
            RecoveryIngress::StoredEvidence => {
                match session
                    .prepare_recovery_with_evidence(&evidence, checkpoint_identity, &source_id)
                    .await
                {
                    Err(ExactRecoveryError::Code(error)) => error,
                    _ => panic!("stored recovery must refuse the source/target identity alias"),
                }
            }
            RecoveryIngress::SuppliedCheckpoint => {
                match session
                    .prepare_recovery_from_checkpoint(
                        &evidence,
                        checkpoint_identity,
                        &source_id,
                        fixture.checkpoint.clone(),
                    )
                    .await
                {
                    Err(ExactRecoveryError::Code(error)) => error,
                    _ => panic!("supplied recovery must refuse the source/target identity alias"),
                }
            }
            RecoveryIngress::PreparedAdmission => {
                // Exercise the admission guard independently of preparation;
                // an internal prepared value cannot bypass the same boundary.
                let lease = ExecutionCoordinator::admit(&session, "alias-admission-pin")
                    .await
                    .unwrap();
                let prepared = PreparedExactRecovery {
                    checkpoint: fixture.checkpoint.clone(),
                    evidence: evidence.clone(),
                    run_id: source_id.clone(),
                    prompt: exact_recovery_prompt(checkpoint_identity),
                    lease,
                };
                match session.spawn_prepared_recovery(prepared).await {
                    Err(ExactRecoveryError::Code(error)) => error,
                    _ => panic!("prepared admission must refuse the source/target identity alias"),
                }
            }
        };
        assert!(matches!(
            error,
            CodeError::RunIdentityConflict { run_id } if run_id == source_id
        ));
        assert!(session.run_snapshot(&source_id).await.is_none());
        assert!(session.run_control_snapshot().await.is_none());
        assert_eq!(client.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(fixture.log.read(&fixture.thread).unwrap(), original);
        let retained = reopened_store
            .load_loop_checkpoint(&source_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::to_vec(&retained).unwrap(), source_bytes);
        if let Some((path, bytes)) = &source_file {
            assert_eq!(std::fs::read(path).unwrap(), *bytes);
        }
        assert_eq!(
            std::fs::read(fixture.workspace.path().join("cached.txt")).unwrap(),
            b"actual cached bytes"
        );
    }
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn reopened_memory_session_refuses_exact_recovery_target_equal_to_source() {
    assert_source_target_alias_refused(false).await;
}

#[tokio::test]
async fn reopened_file_session_refuses_exact_recovery_target_equal_to_source() {
    assert_source_target_alias_refused(true).await;
}
