#![cfg(test)]

use super::*;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn original_facts(fixture: &CacheCoverageFixture) -> Vec<a3s_effect::Fact> {
    FileLog::open(crate::session_folder::effect_log_dir(
        fixture.workspace.path(),
        fixture.session.session_id(),
    ))
    .unwrap()
    .read(&fixture.thread)
    .unwrap()
}

fn negative_facts(fixture: &CacheCoverageFixture) -> Vec<a3s_effect::Fact> {
    original_facts(fixture)
        .into_iter()
        .filter(|fact| fact.kind == "recovery.unproved")
        .collect()
}

fn assert_actual_cached_bytes(fixture: &CacheCoverageFixture) {
    let bytes = std::fs::read(fixture.workspace.path().join("cached.txt")).unwrap();
    assert_eq!(bytes, b"actual cached bytes");
    let digest = format!("sha256:{}", sha256::digest(&bytes));
    assert_eq!(
        fixture.written.effects.changes[0]
            .after
            .content_digest
            .as_deref(),
        Some(digest.as_str())
    );
}

fn assert_sticky_gate(pinned: &PinnedFact, key: &str) {
    assert_sticky_parts(&pinned.parts, key);
}

fn assert_sticky_parts(parts: &FactSession, key: &str) {
    let debts = unresolved_parts(parts);
    let retained = if key == "invalid-recovery-unproved:" {
        debts.iter().any(|debt| debt.starts_with(key))
    } else {
        debts.iter().any(|debt| debt == key)
    };
    assert!(
        retained,
        "the fresh projection must retain exact original debt {key}: {:?}",
        debts
    );
    let surface = parts.surface.as_ref().unwrap();
    let ledger = surface.ledger.lock().unwrap();
    let reports = surface.reports.lock().unwrap();
    assert!(matches!(
        crate::harness_loop::decide_completion(&ledger, &reports, &[], false),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
    let waivers = crate::harness_loop::CompletionWaiverV1::new(
        ledger.digest(),
        "a waiver cannot establish missing original authority",
    )
    .into_iter()
    .collect::<Vec<_>>();
    assert_eq!(waivers.len(), usize::from(!ledger.digest().is_empty()));
    assert!(matches!(
        crate::harness_loop::decide_completion(&ledger, &reports, &waivers, true),
        crate::harness_loop::CompletionGate::Incomplete { .. }
    ));
}

async fn fresh_surface(
    mut fixture: CacheCoverageFixture,
    client: Arc<DetachedAdmissionClient>,
) -> CacheCoverageFixture {
    fixture.pinned.close().await;
    // Install before prepare: a pinned surface must capture this client,
    // rather than the fixture's previous client or a real provider.
    fixture.session.llm_client = client;
    fixture.pinned = prepare_pinned(&fixture.session, "fresh original log only", false)
        .await
        .unwrap();
    fixture
}

async fn assert_send_refused(session: &AgentSession) {
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        session.send("report the retained workspace state", None),
    )
    .await
    .expect("provider-free send has a bounded terminal");
    let error = result.expect_err("original unproved debt must refuse a successful actual send");
    assert!(
        error.to_string().contains("tool_effect_unresolved"),
        "the refusal must come from the typed debt gate: {error:#}"
    );
}

#[tokio::test]
async fn committed_negative_input_survives_surface_destruction_and_actual_send() {
    let mut fixture = CacheCoverageFixture::new().await;
    fixture.seed();
    let negative = negative_facts(&fixture);
    assert_eq!(negative.len(), 1);
    assert!(negative[0].cause.is_none());
    assert_eq!(
        negative[0].payload["source_run_id"],
        fixture.checkpoint.run_id
    );
    let client = Arc::new(DetachedAdmissionClient::new(false));
    let mut fixture = fresh_surface(fixture, client).await;
    assert_sticky_gate(&fixture.pinned, &negative[0].key);
    assert_actual_cached_bytes(&fixture);
    fixture.pinned.close().await;
    assert_send_refused(&fixture.session).await;
    fixture.pinned = prepare_pinned(&fixture.session, "after refused actual send", false)
        .await
        .unwrap();
    assert_sticky_gate(&fixture.pinned, &negative[0].key);
    assert_eq!(negative_facts(&fixture), negative);
    assert!(!original_facts(&fixture)
        .iter()
        .any(|fact| fact.kind == "turn.complete"));
    assert_actual_cached_bytes(&fixture);
    fixture.pinned.close().await;
    fixture.session.close().await;
}

async fn prove_session_store_reopen<F, Fut>(store: Arc<dyn crate::store::SessionStore>, reopen: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Arc<dyn crate::store::SessionStore>>,
{
    let mut fixture = CacheCoverageFixture::new().await;
    fixture.session.session_store = Some(Arc::clone(&store));
    fixture.seed();
    let negative = negative_facts(&fixture);
    let source_id = fixture.checkpoint.run_id.clone();
    let mut target = fixture.checkpoint.clone();
    target.run_id = "removed-recovery-target".into();
    store
        .save_loop_checkpoint(&source_id, &fixture.checkpoint)
        .await
        .unwrap();
    store
        .save_loop_checkpoint(&target.run_id, &target)
        .await
        .unwrap();
    fixture.pinned.close().await;
    fixture.session.save().await.unwrap();
    fixture.session.close().await;
    store.delete_loop_checkpoint(&source_id).await.unwrap();
    store.delete_loop_checkpoint(&target.run_id).await.unwrap();
    let session_id = fixture.session.session_id().to_owned();
    drop(fixture.session);
    let reopened = reopen().await;
    assert!(reopened
        .load_loop_checkpoint(&source_id)
        .await
        .unwrap()
        .is_none());
    assert!(reopened
        .load_loop_checkpoint(&target.run_id)
        .await
        .unwrap()
        .is_none());
    let agent = crate::Agent::from_config(crate::agent_api::tests::test_config())
        .await
        .unwrap();
    let mut session = agent
        .resume_session_async(
            &session_id,
            crate::SessionOptions::new().with_session_store(reopened),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::canonicalize(session.workspace()).unwrap(),
        std::fs::canonicalize(fixture.workspace.path()).unwrap()
    );
    let client = Arc::new(DetachedAdmissionClient::new(false));
    session.llm_client = client;
    let pinned = prepare_pinned(&session, "reopened without source or target cache", false)
        .await
        .unwrap();
    assert_sticky_gate(&pinned, &negative[0].key);
    assert_eq!(
        std::fs::read(fixture.workspace.path().join("cached.txt")).unwrap(),
        b"actual cached bytes"
    );
    pinned.close().await;
    assert_send_refused(&session).await;
    let log = FileLog::open(crate::session_folder::effect_log_dir(
        fixture.workspace.path(),
        &session_id,
    ))
    .unwrap();
    let retained = log.read(&fixture.thread).unwrap();
    assert_eq!(
        retained
            .iter()
            .filter(|fact| fact.kind == "recovery.unproved")
            .cloned()
            .collect::<Vec<_>>(),
        negative
    );
    assert!(!retained.iter().any(|fact| fact.kind == "turn.complete"));
    session.close().await;
    agent.close().await;
}

#[tokio::test]
async fn original_negative_debt_survives_memory_and_reopened_file_session_stores_without_caches() {
    let memory: Arc<dyn crate::store::SessionStore> =
        Arc::new(crate::store::MemorySessionStore::new());
    let reopened_memory = Arc::clone(&memory);
    prove_session_store_reopen(memory, move || async move { reopened_memory }).await;
    let root = tempfile::tempdir().unwrap();
    let file: Arc<dyn crate::store::SessionStore> = Arc::new(
        crate::store::FileSessionStore::new(root.path())
            .await
            .unwrap(),
    );
    let store_path = root.path().to_path_buf();
    prove_session_store_reopen(file, move || async move {
        let store: Arc<dyn crate::store::SessionStore> = Arc::new(
            crate::store::FileSessionStore::new(store_path)
                .await
                .unwrap(),
        );
        store
    })
    .await;
}

#[tokio::test]
async fn distinct_target_runs_reuse_one_negative_identity_but_changed_source_content_does_not() {
    let mut fixture = CacheCoverageFixture::new().await;
    let source = fixture.checkpoint.run_id.clone();
    let mut first = None;
    for target in ["target-one", "target-two", "target-changed-source"] {
        if target == "target-changed-source" {
            fixture
                .checkpoint
                .messages
                .push(Message::user("a distinct exact source checkpoint message"));
        }
        fixture.pinned.close().await;
        let checkpoint = fixture.checkpoint.clone();
        let (_, disposition) =
            owned_admission::scenario(&fixture.session, target, move |pinned, _| async move {
                seed_resumed_mutation_parts(&pinned.parts, &checkpoint).unwrap();
                let negatives = pinned
                    .parts
                    .open()
                    .unwrap()
                    .read_facts()
                    .unwrap()
                    .into_iter()
                    .filter(|fact| fact.kind == "recovery.unproved")
                    .collect::<Vec<_>>();
                assert!(!negatives.is_empty());
                assert_sticky_parts(&pinned.parts, &negatives[0].key);
                fail_owned_prepared(
                    pinned,
                    CodeError::Session("identity-only recovery pin cleanup".into()),
                )
                .await
            })
            .await;
        assert_eq!(disposition, run_admission::OwnerDisposition::Released);
        fixture.pinned = prepare_pinned(
            &fixture.session,
            "fresh original log after exact target",
            false,
        )
        .await
        .unwrap();
        let negative = negative_facts(&fixture);
        assert!(negative.iter().all(|fact| {
            fact.payload["source_run_id"] == source
                && fact.payload.get("target_run_id").is_none()
                && fact.cause.is_none()
        }));
        if target == "target-changed-source" {
            assert_eq!(negative.len(), 2);
            assert_ne!(negative[0].key, negative[1].key);
            assert_ne!(
                negative[0].payload["source_checkpoint_content_digest"],
                negative[1].payload["source_checkpoint_content_digest"]
            );
        } else {
            assert_eq!(negative.len(), 1);
            if let Some(previous) = &first {
                assert_eq!(&negative[0], previous);
            } else {
                first = Some(negative[0].clone());
            }
        }
        assert_sticky_gate(&fixture.pinned, &negative[0].key);
    }
    assert_actual_cached_bytes(&fixture);
    fixture.pinned.close().await;
    fixture.session.close().await;
}

#[tokio::test]
async fn full_log_negative_survives_compaction_settlement_later_original_proof_and_current_passed()
{
    let mut fixture = CacheCoverageFixture::new().await;
    fixture.seed();
    let negative = negative_facts(&fixture);
    fixture.append(&[
        fact(
            "compaction.done",
            "compact-after-negative",
            json!({"summary":"compacted transcript", "kept":[]}),
        ),
        fact(
            "turn.complete",
            "marker-after-negative-before-original-proof",
            json!({"receipt_keys":[]}),
        ),
    ]);
    // These are the fixture's exact A+V identities and actual cached.txt
    // observations, not unrelated reads or a narrative success claim.
    fixture.append(&fixture.original());
    fixture.append(&[fact(
        "turn.complete",
        "marker-after-exact-original-proof",
        json!({"receipt_keys":[fixture.write.identity.key().unwrap(),fixture.verify.identity.key().unwrap()]}),
    )]);
    std::fs::write(
        fixture.workspace.path().join("current.txt"),
        b"actual current bytes after settlement",
    )
    .unwrap();
    let mut current_write = fixture.write.clone();
    current_write.identity.tool_call_id = "current-write-after-settlement".into();
    let mut current_written = fixture.written.clone();
    current_written.identity = current_write.identity.clone();
    current_written.prepared_digest = current_write.digest().unwrap();
    current_written.effects = crate::porcelain::observe_path_effects(
        fixture.workspace.path(),
        crate::porcelain::PathDelta {
            paths: vec!["current.txt".into()],
            incomplete: false,
        },
    );
    fixture.append(&CacheCoverageFixture::pair(
        &current_write,
        &current_written,
    ));
    let fixture = fresh_surface(fixture, Arc::new(DetachedAdmissionClient::new(false))).await;
    let surface = fixture.pinned.parts.surface.as_ref().unwrap();
    let current_digest = surface.ledger.lock().unwrap().digest().to_owned();
    assert!(!current_digest.is_empty());
    assert_eq!(
        std::fs::read(fixture.workspace.path().join("current.txt")).unwrap(),
        b"actual current bytes after settlement"
    );
    let report = crate::verification::VerificationReport::new(
        "actual current bytes after settlement",
        vec![crate::verification::VerificationCheck::required(
            "current-exact-byte-read",
            "file",
            "the test read and checked the actual current.txt bytes",
        )
        .with_status(crate::verification::VerificationStatus::Passed)],
    )
    .with_effect_digest(current_digest);
    assert_eq!(
        report.status,
        crate::verification::VerificationStatus::Passed
    );
    surface.reports.lock().unwrap().push(report);
    crate::fact_control::replay_unsettled_mutations(surface);
    assert_sticky_gate(&fixture.pinned, &negative[0].key);
    assert_eq!(negative_facts(&fixture), negative);
    assert_actual_cached_bytes(&fixture);
    fixture.pinned.close().await;
    fixture.session.close().await;
}

#[tokio::test]
async fn actual_original_log_io_fault_refuses_resume_before_provider_and_retains_source_cache() {
    let mut fixture = CacheCoverageFixture::new().await;
    let store: Arc<dyn crate::store::SessionStore> =
        Arc::new(crate::store::MemorySessionStore::new());
    store
        .save_loop_checkpoint(&fixture.checkpoint.run_id, &fixture.checkpoint)
        .await
        .unwrap();
    fixture.session.session_store = Some(Arc::clone(&store));
    let client = Arc::new(DetachedAdmissionClient::new(false));
    let mut fixture = fresh_surface(fixture, Arc::clone(&client)).await;
    // This creates a real IO error even under a privileged runner, unlike
    // chmod-based fault injection. The original log has no facts to remove.
    let log_file = crate::session_folder::effect_log_dir(
        fixture.workspace.path(),
        fixture.session.session_id(),
    )
    .join(format!("{}.jsonl", fixture.thread));
    assert!(!log_file.exists());
    std::fs::create_dir(&log_file).unwrap();
    let live_error = seed_resumed_mutation_state(&mut fixture.pinned, &fixture.checkpoint)
        .expect_err("a failed append cannot acknowledge a negative input");
    assert!(live_error
        .to_string()
        .contains("tool_effect_recovery_unproved_append_failed"));
    assert!(unresolved(&fixture.pinned)
        .iter()
        .any(|key| key == "tool_effect_cache_evidence_missing"));
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    fixture.pinned.close().await;
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        fixture.session.resume_run(&fixture.checkpoint.run_id),
    )
    .await
    .expect("failed original append must settle without provider work");
    let error = result.expect_err("an uncommitted negative input must refuse recovery");
    // Resume now validates the original source before attempting receipt seeding.
    assert!(
        error.to_string().contains(
            "durable session source: fact schema: fact log must be a single-link regular file"
        ),
        "the original IO error must retain its named boundary: {error:#}"
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    assert!(store
        .load_loop_checkpoint(&fixture.checkpoint.run_id)
        .await
        .unwrap()
        .is_some());
    std::fs::remove_dir(&log_file).unwrap();
    fixture.pinned = prepare_pinned(&fixture.session, "inspect failed append only", false)
        .await
        .unwrap();
    assert!(original_facts(&fixture).is_empty());
    // Together with actor::tests::append_only_input's live tool-body probe,
    // zero provider calls and no preparation prove no tool acquired here.
    assert_actual_cached_bytes(&fixture);
    fixture.pinned.close().await;
    fixture.session.close().await;
}

#[tokio::test]
async fn malformed_and_unknown_negative_inputs_remain_unproved_on_fresh_surface_and_actual_send() {
    let mut template = CacheCoverageFixture::new().await;
    template.seed();
    let valid = negative_facts(&template).remove(0);
    template.pinned.close().await;
    template.session.close().await;
    for scenario in [
        "schema",
        "missing-field",
        "reason",
        "key",
        "transition-cause",
    ] {
        let mut fixture = CacheCoverageFixture::new().await;
        let mut input = NewFact {
            kind: valid.kind.clone(),
            key: valid.key.clone(),
            payload: valid.payload.clone(),
        };
        match scenario {
            "schema" => input.payload["schema"] = json!("a3s.code.recovery-unproved.v99"),
            "missing-field" => {
                input
                    .payload
                    .as_object_mut()
                    .unwrap()
                    .remove("source_run_id");
            }
            "reason" => input.payload["reason"] = json!("unknown-reason"),
            "key" => input.key = "recovery-unproved:wrong-identity".into(),
            "transition-cause" => {}
            _ => unreachable!(),
        }
        fixture
            .log
            .append(
                &fixture.thread,
                &[input],
                (scenario == "transition-cause").then_some("unexpected-transition"),
            )
            .unwrap();
        fixture.append(&[fact(
            "turn.complete",
            "marker-cannot-hide-invalid-negative",
            json!({"receipt_keys":[]}),
        )]);
        fixture = fresh_surface(fixture, Arc::new(DetachedAdmissionClient::new(false))).await;
        assert_sticky_gate(&fixture.pinned, "invalid-recovery-unproved:");
        assert_actual_cached_bytes(&fixture);
        fixture.pinned.close().await;
        assert_send_refused(&fixture.session).await;
        fixture.session.close().await;
    }
}

#[tokio::test]
async fn exact_target_publisher_cleanup_never_deletes_source_or_discharges_original_negative() {
    for outcome in ["completed", "failed", "cancelled"] {
        let mut fixture = CacheCoverageFixture::new().await;
        let store: Arc<dyn crate::store::SessionStore> =
            Arc::new(crate::store::MemorySessionStore::new());
        fixture.session.session_store = Some(Arc::clone(&store));
        fixture.seed();
        let negative = negative_facts(&fixture);
        let source = fixture.checkpoint.run_id.clone();
        let target = format!("cleanup-target-{outcome}");
        let mut target_checkpoint = fixture.checkpoint.clone();
        target_checkpoint.run_id = target.clone();
        store
            .save_loop_checkpoint(&source, &fixture.checkpoint)
            .await
            .unwrap();
        store
            .save_loop_checkpoint(&target, &target_checkpoint)
            .await
            .unwrap();
        fixture
            .session
            .run_store
            .reserve_run_with_id(target.clone(), fixture.session.session_id(), "cleanup only")
            .await;
        *fixture.session.current_run_id.lock().await = Some(target.clone());
        let cancellation = tokio_util::sync::CancellationToken::new();
        if outcome == "cancelled" {
            cancellation.cancel();
        }
        // This is a publisher cleanup unit pin, not proof that a debt-bearing
        // Run may successfully complete. Actual send refusal is pinned above.
        let result = if outcome == "failed" {
            Err(anyhow::anyhow!("exact target publisher failure sentinel"))
        } else {
            Ok(AgentResult {
                text: "publisher fixture".into(),
                messages: Vec::new(),
                usage: Default::default(),
                tool_calls_count: 0,
                verification_reports: Vec::new(),
                completion: crate::harness_loop::CompletionTerminal::Narrative,
                run_admission: "ordinary".into(),
                forced_stop: None,
            })
        };
        TurnFinish::from_session(&fixture.session, cancellation, Some(target.clone()))
            .publish("cleanup only", &result, false)
            .await;
        let expected_status = match outcome {
            "completed" => crate::run::RunStatus::Completed,
            "failed" => crate::run::RunStatus::Failed,
            "cancelled" => crate::run::RunStatus::Cancelled,
            _ => unreachable!(),
        };
        assert_eq!(
            fixture.session.run_snapshot(&target).await.unwrap().status,
            expected_status
        );
        assert!(store.load_loop_checkpoint(&target).await.unwrap().is_none());
        assert!(store.load_loop_checkpoint(&source).await.unwrap().is_some());
        assert_eq!(negative_facts(&fixture), negative);
        fixture = fresh_surface(fixture, Arc::new(DetachedAdmissionClient::new(false))).await;
        assert_sticky_gate(&fixture.pinned, &negative[0].key);
        store.delete_loop_checkpoint(&source).await.unwrap();
        crate::fact_control::replay_unsettled_mutations(
            fixture.pinned.parts.surface.as_ref().unwrap(),
        );
        assert_sticky_gate(&fixture.pinned, &negative[0].key);
        assert_actual_cached_bytes(&fixture);
        fixture.pinned.close().await;
        fixture.session.close().await;
    }
}
