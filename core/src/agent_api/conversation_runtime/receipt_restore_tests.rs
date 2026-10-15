#![cfg(test)]

use super::*;
use a3s_effect::{
    FileLog, LogStore, NewFact, ToolEffectObservationV1, ToolExecutionOutcome,
    ToolExecutionResultV1, ToolFileIdentityV1, ToolFileKind, ToolInvocationIdentityV1,
    ToolPathChangeV1, ToolPreparedV1, ToolRecoveryPolicy, TOOL_PREPARED_SCHEMA, TOOL_RESULT_SCHEMA,
};
use serde_json::json;

async fn session(workspace: &std::path::Path) -> AgentSession {
    let agent = crate::Agent::from_config(crate::agent_api::tests::test_config())
        .await
        .unwrap();
    agent
        .build_session(
            workspace.to_string_lossy().into_owned(),
            Arc::new(crate::agent_api::tests::StaticStreamingClient::new(
                "narrative",
            )),
            &crate::SessionOptions::new().with_session_id("receipt-cache-recovery"),
        )
        .unwrap()
}

fn checkpoint(session: &AgentSession) -> LoopCheckpoint {
    LoopCheckpoint {
        schema_version: crate::loop_checkpoint::LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: "cached-run".into(),
        session_id: session.session_id.clone(),
        capability_binding: None,
        standing_context_digest: None,
        durable_source: None,
        turn: 1,
        // Compaction retained no original tool call/result messages.
        messages: vec![
            Message::user("compacted summary: work was interrupted"),
            Message::assistant("already done"),
        ],
        total_usage: Default::default(),
        tool_calls_count: 1,
        verification_reports: Vec::new(),
        convergence: crate::loop_checkpoint::LoopConvergenceState {
            mutations: serde_json::from_value(json!({
                "records":[], "unresolved_tool_effects":["cached-unknown"]
            }))
            .unwrap(),
            ..Default::default()
        },
        checkpoint_ms: 1,
    }
}

fn prepared(id: &str) -> ToolPreparedV1 {
    let digest = a3s_effect::tool_args_digest(&json!({})).unwrap();
    ToolPreparedV1 {
        schema: TOOL_PREPARED_SCHEMA.into(),
        identity: ToolInvocationIdentityV1 {
            turn: 1,
            cycle: 0,
            tool_call_id: id.into(),
            batch_index: 0,
            admission: None,
        },
        tool_name: "bash".into(),
        original_args_digest: digest.clone(),
        effective_args_digest: digest,
        recovery: ToolRecoveryPolicy::NeverReplay,
        baseline: None,
    }
}

fn fact(kind: &str, key: &str, payload: serde_json::Value) -> NewFact {
    NewFact {
        kind: kind.into(),
        key: key.into(),
        payload,
    }
}

fn unresolved(pinned: &PinnedFact) -> Vec<String> {
    unresolved_parts(&pinned.parts)
}

fn unresolved_parts(parts: &FactSession) -> Vec<String> {
    let ledger = parts.surface.as_ref().unwrap().ledger.lock().unwrap();
    let serialized = serde_json::to_value(&*ledger).unwrap();
    let Some(unresolved) = serialized.get("unresolved_tool_effects") else {
        return Vec::new();
    };
    unresolved
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().into())
        .collect()
}

#[tokio::test]
async fn missing_log_and_compacted_checkpoint_preserve_unknown_after_history_import() {
    let root = tempfile::tempdir().unwrap();
    let session = session(root.path()).await;
    let mut checkpoint = checkpoint(&session);
    checkpoint
        .convergence
        .mutations
        .observe_unseen_paths(&["cached.txt".into()]);
    let cached_report = crate::verification::VerificationReport::new(
        "cached passing verification",
        vec![crate::verification::VerificationCheck::required(
            "read-back",
            "command",
            "bytes match",
        )
        .with_status(crate::verification::VerificationStatus::Passed)],
    )
    .with_effect_digest(checkpoint.convergence.mutations.digest());
    checkpoint.verification_reports.push(cached_report.clone());
    let mut pinned = prepare_pinned(&session, "resume", false).await.unwrap();
    let later_report = crate::verification::VerificationReport::new("later report", Vec::new());
    pinned
        .parts
        .surface
        .as_ref()
        .unwrap()
        .reports
        .lock()
        .unwrap()
        .push(later_report.clone());
    seed_resumed_mutation_state(&mut pinned, &checkpoint).unwrap();
    seed_exact_recovery(&pinned, &checkpoint.messages).unwrap();
    crate::fact_control::replay_unsettled_mutations(pinned.parts.surface.as_ref().unwrap());
    let debt = unresolved(&pinned);
    assert!(debt.contains(&"cached-unknown".into()));
    assert!(debt.contains(&"tool_effect_cache_evidence_missing".into()));
    {
        let surface = pinned.parts.surface.as_ref().unwrap();
        let reports = surface.reports.lock().unwrap();
        assert!(reports.contains(&cached_report));
        assert!(reports.contains(&later_report));
        assert!(matches!(
            crate::harness_loop::decide_completion(
                &surface.ledger.lock().unwrap(),
                &reports,
                &[],
                false,
            ),
            crate::harness_loop::CompletionGate::Incomplete { .. }
        ));
    }
    assert!(!pinned
        .parts
        .open()
        .unwrap()
        .read_facts()
        .unwrap()
        .iter()
        .any(|fact| fact.kind == "tool.prepared" || fact.kind == "tool.result"));
    pinned.close().await;
}

#[tokio::test]
async fn original_log_wins_for_later_known_bytes_and_preserves_all_unknowns_past_marker() {
    let root = tempfile::tempdir().unwrap();
    let session = session(root.path()).await;
    let mut checkpoint = checkpoint(&session);
    checkpoint
        .convergence
        .mutations
        .observe_unseen_paths(&["changed.txt".into()]);
    let known = prepared("known-later");
    let unknown = prepared("unknown-later");
    let receipt = ToolExecutionResultV1 {
        schema: TOOL_RESULT_SCHEMA.into(),
        identity: known.identity.clone(),
        prepared_digest: known.digest().unwrap(),
        outcome: ToolExecutionOutcome::Failed,
        exit_code: Some(7),
        output: json!("failed after writing"),
        effects: ToolEffectObservationV1::complete(vec![ToolPathChangeV1 {
            path: "changed.txt".into(),
            before: None,
            after: ToolFileIdentityV1 {
                kind: ToolFileKind::File,
                content_digest: Some(format!("sha256:{}", sha256::digest(b"later known bytes"))),
                mode: Some(0o644),
            },
        }]),
    };
    let unknown_receipt = ToolExecutionResultV1::unknown(&unknown, "process died").unwrap();
    let thread = crate::fact_control::thread_for_session(&session.session_id);
    let log = FileLog::open(crate::session_folder::effect_log_dir(
        root.path(),
        &session.session_id,
    ))
    .unwrap();
    log.append(
        &thread,
        &[
            fact("tool.prepared", "prepare-known", json!({"prepared":known})),
            fact("tool.result", "result-known", json!({"receipt":receipt})),
            fact(
                "tool.prepared",
                "prepare-unknown",
                json!({"prepared":unknown}),
            ),
            fact(
                "tool.result",
                "result-unknown",
                json!({"receipt":unknown_receipt}),
            ),
            fact(
                "turn.complete",
                "invalid-forgiveness",
                json!({"receipt_keys":[unknown.identity.key().unwrap()]}),
            ),
        ],
        None,
    )
    .unwrap();
    let mut pinned = prepare_pinned(&session, "resume", false).await.unwrap();
    // Opening may bind standing context. History seeding must preserve every
    // authoritative fact already present after that normal admission step.
    let original = pinned.parts.open().unwrap().read_facts().unwrap();
    seed_resumed_mutation_state(&mut pinned, &checkpoint).unwrap();
    let seeded = pinned.parts.open().unwrap().read_facts().unwrap();
    assert_eq!(&seeded[..original.len()], original.as_slice());
    assert_eq!(seeded.len(), original.len() + 1);
    assert_eq!(seeded.last().unwrap().kind, "recovery.unproved");
    seed_exact_recovery(&pinned, &checkpoint.messages).unwrap();
    let debt = unresolved(&pinned);
    assert!(debt.contains(&"cached-unknown".into()));
    assert!(debt.contains(&unknown.identity.key().unwrap()));
    assert!(debt.contains(&"tool_effect_cache_evidence_missing".into()));
    {
        let surface = pinned.parts.surface.as_ref().unwrap();
        let ledger = surface.ledger.lock().unwrap();
        assert!(ledger.content_digest_matches("changed.txt", &sha256::digest(b"later known bytes")));
    }
    assert_eq!(
        pinned.parts.open().unwrap().read_facts().unwrap(),
        seeded,
        "checkpoint history import must not reset or replace the existing immutable fact log"
    );
    pinned.close().await;
}

mod cache_coverage;

#[tokio::test]
async fn retained_end_and_external_end_share_the_actual_byte_check_summary() {
    let workspace = tempfile::tempdir().unwrap();
    let bytes = b"retained verification evidence";
    std::fs::write(workspace.path().join("verified.txt"), bytes).unwrap();
    let read_back = std::fs::read(workspace.path().join("verified.txt")).unwrap();
    assert_eq!(read_back, bytes);
    let session = session(workspace.path()).await;
    let prepared = prepared("summary-byte-write");
    let receipt = ToolExecutionResultV1 {
        schema: TOOL_RESULT_SCHEMA.into(),
        identity: prepared.identity.clone(),
        prepared_digest: prepared.digest().unwrap(),
        outcome: ToolExecutionOutcome::Succeeded,
        exit_code: Some(0),
        output: json!("actual byte observation"),
        effects: ToolEffectObservationV1::complete(vec![ToolPathChangeV1 {
            path: "verified.txt".into(),
            before: None,
            after: ToolFileIdentityV1 {
                kind: ToolFileKind::File,
                content_digest: Some(format!("sha256:{}", sha256::digest(&read_back))),
                mode: Some(0o644),
            },
        }]),
    };
    let mut ledger = crate::harness_loop::MutationLedger::default();
    ledger.observe_tool_receipt("write", &receipt);
    // This is the exact Rust file read above, not a claimed project suite.
    let report = crate::verification::VerificationReport::new(
        "actual file byte check",
        vec![crate::verification::VerificationCheck::required(
            "exact-byte-read",
            "file",
            "read bytes equal the bytes written in this test",
        )
        .with_status(crate::verification::VerificationStatus::Passed)],
    )
    .with_effect_digest(ledger.digest());
    let reports = vec![report];
    let crate::harness_loop::CompletionGate::Allow(completion) =
        crate::harness_loop::decide_completion(&ledger, &reports, &[], false)
    else {
        panic!("the actual byte check must close its exact receipt projection")
    };
    assert!(matches!(
        completion,
        crate::harness_loop::CompletionTerminal::Verified { .. }
    ));
    let result = AgentResult {
        text: "the byte check passed".into(),
        messages: Vec::new(),
        usage: Default::default(),
        tool_calls_count: 1,
        verification_reports: reports,
        completion,
        forced_stop: None,
        run_admission: "ordinary".into(),
    };
    let run_id = "retained-summary-run";
    session
        .run_store
        .reserve_run_with_id(run_id.into(), session.session_id(), "verify file bytes")
        .await;
    *session.current_run_id.lock().await = Some(run_id.into());
    let mut view = a3s_effect::CodingView::empty();
    view.assistant = Some(result.text.clone());
    let settled = Ok(a3s_effect::Settlement {
        view,
        log: Vec::new(),
        steps: 0,
        cut: a3s_effect::cut_log(&[]),
        trace: Vec::new(),
    });
    let (tx, mut rx) = mpsc::channel(4);
    emit_settled(&tx, &settled, Some(&result), true).await;
    let AgentEvent::End {
        verification_summary: external,
        completion_terminal: external_completion,
        ..
    } = rx.recv().await.unwrap()
    else {
        panic!("missing external End")
    };
    TurnFinish::from_session(
        &session,
        tokio_util::sync::CancellationToken::new(),
        Some(run_id.into()),
    )
    .publish("verify file bytes", &Ok(result.clone()), false)
    .await;
    let retained = session.run_events(run_id).await;
    assert_eq!(retained.len(), 1);
    let AgentEvent::End {
        verification_summary: stored,
        completion_terminal: stored_completion,
        ..
    } = &retained[0].event
    else {
        panic!("missing retained End for the same Run")
    };
    assert_eq!(stored, &external);
    assert_eq!(stored_completion, &external_completion);
    assert_eq!(stored.report_count, 1);
    assert_eq!(stored.required_check_count, 1);
    assert_eq!(
        stored.status,
        crate::verification::VerificationStatus::Passed
    );
}

struct DetachedAdmissionClient {
    calls: std::sync::atomic::AtomicUsize,
    messages: std::sync::Mutex<Vec<Vec<Message>>>,
    park_first: bool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
}

impl DetachedAdmissionClient {
    fn new(park_first: bool) -> Self {
        Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
            messages: std::sync::Mutex::new(Vec::new()),
            park_first,
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
        }
    }
}

#[async_trait::async_trait]
impl crate::llm::LlmClient for DetachedAdmissionClient {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        _tools: &[crate::llm::ToolDefinition],
    ) -> anyhow::Result<crate::llm::LlmResponse> {
        let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.messages.lock().unwrap().push(messages.to_vec());
        if self.park_first && call == 0 {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
            return Ok(crate::agent_api::tests::scripted_tool_call_response(
                "detached-control-read",
                "read",
                json!({"file_path":"readable.txt"}),
            ));
        }
        Ok(crate::agent_api::tests::StaticStreamingClient::new("finished").response())
    }

    async fn complete_streaming(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[crate::llm::ToolDefinition],
        _cancel: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<crate::llm::StreamEvent>> {
        let response = self.complete(messages, system, tools).await?;
        let (tx, rx) = mpsc::channel(1);
        tx.send(crate::llm::StreamEvent::Done(response))
            .await
            .unwrap();
        Ok(rx)
    }
}

async fn detached_session(
    workspace: &std::path::Path,
    client: Arc<DetachedAdmissionClient>,
    cognitive: bool,
) -> AgentSession {
    let mut config = crate::agent_api::tests::test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = crate::Agent::from_config(config).await.unwrap();
    let mut options = crate::SessionOptions::new()
        .with_session_id("detached-admission-proof")
        .with_session_store(Arc::new(crate::store::MemorySessionStore::new()))
        .with_planning_mode(crate::prompts::PlanningMode::Disabled)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false)
        .with_continuation(false)
        .with_permission_policy(crate::permissions::PermissionPolicy::new().allow("*"));
    if cognitive {
        let (context, _) = crate::agent_api::tests::test_cognitive_context(
            crate::agent_api::tests::TestCognitiveProviderMode::Valid,
        );
        options = options.with_cognitive_context(context);
    }
    agent
        .build_session(workspace.to_string_lossy().into_owned(), client, &options)
        .unwrap()
}

struct AdmissionCloseEffect {
    closed: Arc<std::sync::atomic::AtomicUsize>,
    fail: bool,
}

#[async_trait::async_trait]
impl crate::capability::CapabilityEffect for AdmissionCloseEffect {
    fn name(&self) -> &str {
        "detached-admission-close-proof"
    }

    async fn close(
        self: Box<Self>,
    ) -> std::result::Result<(), crate::capability::CapabilityEffectError> {
        // The assertion after admission cleanup must observe asynchronous
        // close completion, rather than merely dropping or cancelling a Run.
        tokio::task::yield_now().await;
        self.closed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail {
            Err(crate::capability::CapabilityEffectError::new(
                "admission close sentinel",
            ))
        } else {
            Ok(())
        }
    }
}

mod owned_admission;

#[tokio::test]
async fn detached_binding_uses_the_pinned_capability_cognitive_and_control_identity() {
    owned_admission::binding_identity().await;
}

#[tokio::test]
async fn failed_recovery_seed_closes_bound_run_before_releasing_session_slots() {
    owned_admission::recovery_seed_failure().await;
}

#[tokio::test]
async fn conflicting_detached_binding_fails_before_execution_and_retains_close_failure() {
    owned_admission::binding_conflict_retains_failed_close().await;
}

#[tokio::test]
async fn detached_live_controls_apply_exact_steer_and_settle_parked_interrupt() {
    for interrupt in [false, true] {
        let workspace = tempfile::tempdir().unwrap();
        std::fs::write(workspace.path().join("readable.txt"), b"read-only proof").unwrap();
        let client = Arc::new(DetachedAdmissionClient::new(true));
        let session = detached_session(workspace.path(), client.clone(), false).await;
        let run_id = if interrupt {
            "detached-interrupt"
        } else {
            "detached-steer"
        };
        let spawned = session
            .spawn_run_with_id(run_id, "read the provided file")
            .await
            .unwrap();
        let AgentRunSpawn::Started { worker, .. } = spawned else {
            panic!("fresh Run replayed")
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            client.entered.notified(),
        )
        .await
        .unwrap();
        let control = session.run_control_snapshot().await.unwrap();
        assert_eq!(control.run_id, run_id);
        assert!(control.active && control.turn_id.is_some());
        let exact_input = "  保留 Unicode 与两侧空格\nsecond line  ";
        let receipt = if interrupt {
            session
                .interrupt(
                    crate::run_control::InterruptRequest::new()
                        .with_run_id(run_id)
                        .with_reason("parked interrupt proof"),
                )
                .await
                .unwrap()
        } else {
            session
                .steer(crate::run_control::SteerRequest::new(exact_input).with_run_id(run_id))
                .await
                .unwrap()
        };
        assert_eq!(
            receipt.state,
            crate::run_control::RunControlReceiptState::Accepted
        );
        if !interrupt {
            client.release.add_permits(1);
        }
        tokio::time::timeout(std::time::Duration::from_secs(10), worker)
            .await
            .unwrap()
            .unwrap();
        let events = session.run_events(run_id).await;
        let applied = events.iter().filter(|record| matches!(&record.event, AgentEvent::RunControlApplied { request_id, .. } if request_id == &receipt.request_id)).collect::<Vec<_>>();
        assert_eq!(
            applied.len(),
            1,
            "every accepted control needs exactly one retained applied event: {events:?}"
        );
        if interrupt {
            assert!(matches!(
                &applied[0].event,
                AgentEvent::RunControlApplied {
                    operation: crate::run_control::RunControlOperation::Interrupt,
                    ..
                }
            ));
            assert!(!events
                .iter()
                .any(|record| matches!(record.event, AgentEvent::End { .. })));
            assert_eq!(
                session.run_snapshot(run_id).await.unwrap().status,
                crate::run::RunStatus::Cancelled
            );
        } else {
            assert!(
                matches!(&applied[0].event, AgentEvent::RunControlApplied { operation: crate::run_control::RunControlOperation::Steer, input: Some(input), .. } if input == exact_input)
            );
            {
                let calls = client.messages.lock().unwrap();
                assert!(
                    calls.iter().skip(1).any(|messages| messages
                        .iter()
                        .any(|message| message.role == "user" && message.text() == exact_input)),
                    "the next actual provider call must consume the exact steer bytes"
                );
            }
            assert_eq!(
                session.run_snapshot(run_id).await.unwrap().status,
                crate::run::RunStatus::Completed
            );
        }
        assert!(session.run_control_snapshot().await.is_none());
    }
}
