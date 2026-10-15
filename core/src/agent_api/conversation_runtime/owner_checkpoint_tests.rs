#![cfg(test)]

use super::*;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::llm::{LlmClient, LlmResponse, StreamEvent, TokenUsage, ToolDefinition};
use crate::store::{FileSessionStore, MemorySessionStore, SessionStore};
use tokio::sync::{oneshot, Semaphore};

async fn cancelled_charge_retirement(store: Arc<dyn SessionStore>, reopen: Option<PathBuf>) {
    let workspace = tempfile::tempdir().unwrap();
    let mut config = super::super::test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let agent = super::super::Agent::from_config(config).await.unwrap();
    let guard = Arc::new(PartialGuard {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
        records: Default::default(),
    });
    let client = Arc::new(OpenDone {
        sender: Default::default(),
        calls: Default::default(),
    });
    let options = super::super::SessionOptions::new()
        .with_session_id("governed-checkpoint")
        .with_session_store(store.clone())
        .with_auto_save(true)
        .with_budget_guard(guard.clone())
        .with_planning(false)
        .with_auto_delegation_enabled(false)
        .with_manual_delegation_enabled(false);
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &options,
        )
        .unwrap();
    let first = session
        .stream_with_run("first original run", None)
        .await
        .unwrap();
    let first_run = first.run.unwrap();
    let first_id = first_run.id().to_string();
    tokio::time::timeout(std::time::Duration::from_secs(5), guard.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert!(first_run.cancel().await);
    assert!(matches!(
        session
            .send("must refuse while the original charge is pending", None)
            .await,
        Err(CodeError::SessionBusy { .. })
    ));
    assert_eq!(client.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(session.task_scheduler.health().await.unwrap().active, 1);
    let worker = first.worker;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if first_run.snapshot().await.unwrap().status == crate::run::RunStatus::Cancelled {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        !worker.is_finished(),
        "publication alone cannot release original ownership"
    );
    // Both actual successful calls owe this same original guard one callback.
    guard.release.add_permits(2);
    tokio::time::timeout(std::time::Duration::from_secs(5), worker)
        .await
        .expect("actual callback and original child join must allow clean retirement")
        .unwrap();
    let mut events = first.events;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while let Some(event) = events.recv().await {
            assert!(!matches!(event, AgentEvent::End { .. }));
        }
    })
    .await
    .unwrap();
    assert_eq!(
        first_run.snapshot().await.unwrap().status,
        crate::run::RunStatus::Cancelled
    );
    assert!(session.run_admission.current_original().is_none());
    let next = session
        .stream_with_run("next original run", None)
        .await
        .unwrap();
    let next_run = next.run.unwrap();
    let next_id = next_run.id().to_string();
    assert_ne!(first_id, next_id);
    let mut events = next.events;
    while events.recv().await.is_some() {}
    tokio::time::timeout(std::time::Duration::from_secs(5), next.worker)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        next_run.snapshot().await.unwrap().status,
        crate::run::RunStatus::Completed
    );
    assert_eq!(client.calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    {
        let charges = guard.records.lock().unwrap();
        assert_eq!(charges.len(), 2);
        assert!(charges.iter().all(|usage| {
            usage.prompt_tokens == 17
                && usage.completion_tokens == 13
                && usage.total_tokens == 30
                && usage.cache_read_tokens == Some(2)
                && usage.cache_write_tokens.is_none()
        }));
    }
    let health = session.task_scheduler.health().await.unwrap();
    assert_eq!((health.active, health.released), (0, 2));
    let read: Arc<dyn SessionStore> = match reopen {
        Some(path) => Arc::new(FileSessionStore::new(path).await.unwrap()),
        None => store,
    };
    let runs = read
        .load_run_records("governed-checkpoint")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().any(|run| run.snapshot.id == first_id
        && run.snapshot.status == crate::run::RunStatus::Cancelled));
    assert!(runs
        .iter()
        .any(|run| run.snapshot.id == next_id
            && run.snapshot.status == crate::run::RunStatus::Completed));
    session.close().await;
}

#[tokio::test]
async fn cancelled_success_waits_for_original_charge_then_releases_in_both_stores() {
    cancelled_charge_retirement(Arc::new(MemorySessionStore::new()), None).await;
    let root = tempfile::tempdir().unwrap();
    cancelled_charge_retirement(
        Arc::new(FileSessionStore::new(root.path()).await.unwrap()),
        Some(root.path().to_path_buf()),
    )
    .await;
}

struct PartialGuard {
    entered: Semaphore,
    release: Semaphore,
    records: std::sync::Mutex<Vec<TokenUsage>>,
}
#[async_trait::async_trait]
impl BudgetGuard for PartialGuard {
    async fn check_before_llm(&self, session: &str, estimate: usize) -> BudgetDecision {
        assert_eq!(session, "governed-checkpoint");
        assert!(estimate > 0);
        BudgetDecision::Allow
    }
    async fn record_after_llm(&self, session: &str, usage: &TokenUsage) {
        assert_eq!(session, "governed-checkpoint");
        self.records.lock().unwrap().push(usage.clone());
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
    }
}
struct OpenDone {
    sender: std::sync::Mutex<Option<mpsc::Sender<StreamEvent>>>,
    calls: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl LlmClient for OpenDone {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        panic!("successful actual Done must not fallback");
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (sender, receiver) = mpsc::channel(1);
        sender
            .send(StreamEvent::Done(LlmResponse {
                message: Message::assistant("actual completed provider"),
                usage: TokenUsage {
                    prompt_tokens: 17,
                    completion_tokens: 13,
                    total_tokens: 30,
                    cache_read_tokens: Some(2),
                    cache_write_tokens: None,
                },
                stop_reason: None,
                token_logprobs: vec![],
                meta: None,
            }))
            .await
            .unwrap();
        *self.sender.lock().unwrap() = Some(sender); // Do not manufacture EOF.
        Ok(receiver)
    }
}

async fn scenario(store: Arc<dyn SessionStore>, reopen: Option<PathBuf>) {
    let workspace = tempfile::tempdir().unwrap();
    let agent = super::super::Agent::from_config(super::super::test_config())
        .await
        .unwrap();
    let guard = Arc::new(PartialGuard {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
        records: Default::default(),
    });
    let client = Arc::new(OpenDone {
        sender: Default::default(),
        calls: Default::default(),
    });
    let session = Arc::new(
        agent
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &super::super::SessionOptions::new()
                    .with_session_id("governed-checkpoint")
                    .with_session_store(store.clone())
                    .with_auto_save(true)
                    .with_budget_guard(guard.clone()),
            )
            .unwrap(),
    );
    let lease = ExecutionCoordinator::admit(&session, "same governed component")
        .await
        .unwrap();
    let mut captured = OwnedConversationCapture::capture(&session, false).unwrap();
    let original_cancel = captured.preparation.cancellation.clone();
    let checkpoint_store = Arc::clone(session.session_store.as_ref().unwrap());
    let checkpoint_session_id = session.session_id.clone();
    let checkpoint_ms = session.config.host_env.now_ms();
    let (handoff, start, original) = ExecutionCoordinator::prepare_original_handoff(
        &session,
        lease,
        super::super::run_admission::ExecutionMode::Blocking,
        original_cancel,
    )
    .unwrap();
    captured.controls = captured.controls.bind_original(original.clone()).unwrap();
    let context = original.clone();
    let (cancel_ready, cancel_rx) = oneshot::channel();
    let (eof, eof_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        // The actual canonical capture/constructor replaces the formerly
        // unqualified legacy fixture. No Session or proxy committer is moved
        // into the original execution worker.
        context
            .record_scope_not_attempted(run_admission::OriginalScopeKind::Command)
            .unwrap();
        let OwnedPinnedFact {
            run,
            parts,
            guard: capability_run,
            finish,
            ..
        } = prepare_owned_fact(captured, context.clone(), "same actual prompt", None, None)
            .await
            .unwrap();
        assert!(
            parts.original_model.is_some(),
            "canonical constructor supplies the original weak model port"
        );
        let surface = parts.surface.as_ref().unwrap();
        let checkpoint = surface.checkpoint.as_ref().unwrap();
        let run_id = checkpoint.run_id.clone();
        let saved = crate::loop_checkpoint::LoopCheckpoint {
            schema_version: crate::loop_checkpoint::LOOP_CHECKPOINT_SCHEMA_VERSION,
            run_id: run_id.clone(),
            session_id: checkpoint_session_id,
            capability_binding: checkpoint.capability_binding.clone(),
            standing_context_digest: None,
            durable_source: None,
            turn: 1,
            messages: vec![Message::user("actual checkpoint before model")],
            total_usage: TokenUsage::default(),
            tool_calls_count: 0,
            verification_reports: vec![],
            convergence: Default::default(),
            checkpoint_ms,
        };
        checkpoint.sink.save_checkpoint(&saved).await;
        assert_eq!(
            checkpoint_store
                .load_loop_checkpoint(&run_id)
                .await
                .unwrap()
                .unwrap()
                .messages[0]
                .text(),
            "actual checkpoint before model"
        );
        surface
            .streamed_text
            .store(true, std::sync::atomic::Ordering::Relaxed);
        cancel_ready
            .send((surface.cancel.clone(), Arc::clone(&surface.streamed_text)))
            .unwrap();
        let fact = parts.open().unwrap();
        let result = fact.user_text("same actual prompt").await;
        assert!(
            result.is_ok(),
            "original cancellation keeps the actor interrupt result"
        );
        // Drop every actual parent holder, then await the actual original
        // receiver EOF/committer join while the callback remains blocked.
        drop(fact);
        drop(parts);
        drop(finish);
        drop(run);
        let close = capability_run.close().await;
        context
            .record_scope_close(run_admission::OriginalScopeKind::Fact, &close)
            .unwrap();
        close.unwrap();
        let actual =
            tokio::time::timeout(std::time::Duration::from_secs(5), context.wait_committer())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            actual,
            run_admission::ActualCommitterJoin::Joined(
                run_admission::ActualJoinObservation::Completed
            )
        );
        eof.send(run_id).unwrap();
    });
    let (disposition, observed) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), disposition);
    let (actual_cancel, actual_streamed_text) = cancel_rx.await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), guard.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert_eq!(guard.records.lock().unwrap().len(), 1);
    assert_eq!(guard.records.lock().unwrap()[0].cache_read_tokens, Some(2));
    assert!(client.sender.lock().unwrap().is_some());
    assert!(
        !actual_streamed_text.load(std::sync::atomic::Ordering::Relaxed),
        "this actual Done-only response reset the prior response marker before dispatch"
    );
    actual_cancel.cancel();
    let run_id = tokio::time::timeout(std::time::Duration::from_secs(5), eof_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(client.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        !proxy.is_finished(),
        "same guardian still owns blocked actual callback"
    );
    let health = session.task_scheduler.health().await.unwrap();
    assert_eq!((health.active, health.released), (1, 0));
    assert!(
        ExecutionCoordinator::admit(&session, "must not overlap callback")
            .await
            .is_err()
    );
    let read: Arc<dyn SessionStore> = match reopen {
        Some(path) => Arc::new(FileSessionStore::new(path).await.unwrap()),
        None => store,
    };
    let checkpoint = read.load_loop_checkpoint(&run_id).await.unwrap().unwrap();
    checkpoint
        .ensure_owned_by(&run_id, "governed-checkpoint")
        .unwrap();
    assert_eq!(
        checkpoint.messages[0].text(),
        "actual checkpoint before model"
    );
    let snapshot = read
        .load_snapshot("governed-checkpoint")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.session.id, "governed-checkpoint");
    assert_eq!(
        snapshot.session.messages[0].text(),
        "actual checkpoint before model"
    );
    guard.release.add_permits(1);
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), observed)
            .await
            .unwrap()
            .unwrap(),
        super::super::run_admission::OwnerDisposition::RetainedUnproved
    );
    assert_eq!(guard.records.lock().unwrap().len(), 1);
    assert_eq!(session.task_scheduler.health().await.unwrap().released, 0);
    assert!(Arc::ptr_eq(
        &original,
        &session.run_admission.current_original().unwrap()
    ));
}

#[tokio::test]
async fn actual_checkpoint_eof_is_independent_of_partial_model_callback_in_both_session_stores() {
    scenario(Arc::new(MemorySessionStore::new()), None).await;
    let root = tempfile::tempdir().unwrap();
    scenario(
        Arc::new(FileSessionStore::new(root.path()).await.unwrap()),
        Some(root.path().to_path_buf()),
    )
    .await;
}

// These callers exercise the existing facades and original stores. They do not
// construct an executor, bill usage, or save an expected Session snapshot.
const MATRIX_SESSION: &str = "code-mode-matrix";
const MATRIX_PROMPT: &str = "Remember this workspace preference: use codename MATRIX-4411.";
const MATRIX_ANSWER: &str = "The workspace answer is complete.";
const MATRIX_MEMORY: &str = "Use codename MATRIX-4411 for this workspace.";
const MATRIX_SOURCE: &str = "matrix-source";
const MATRIX_TARGET: &str = "matrix-target";

#[derive(Clone, Copy, Debug)]
enum MatrixMode {
    Send,
    SendAttachments,
    Stream,
    StreamAttachments,
    Detached,
    ExactRecovery,
    PreparedStored,
    PreparedSupplied,
    OrdinaryCheckpoint,
    OrdinaryLog,
}
impl MatrixMode {
    fn ordinary(self) -> bool {
        matches!(self, Self::OrdinaryCheckpoint | Self::OrdinaryLog)
    }
    fn attachments(self) -> bool {
        matches!(self, Self::SendAttachments | Self::StreamAttachments)
    }
    fn checkpoint(self) -> bool {
        matches!(
            self,
            Self::ExactRecovery
                | Self::PreparedStored
                | Self::PreparedSupplied
                | Self::OrdinaryCheckpoint
        )
    }
    fn prompt(self) -> String {
        match self {
            Self::ExactRecovery => format!("<resume run={MATRIX_SOURCE}>"),
            Self::PreparedStored | Self::PreparedSupplied => {
                exact_recovery_prompt("matrix-boundary")
            }
            Self::OrdinaryLog => String::new(),
            _ => MATRIX_PROMPT.into(),
        }
    }
}

fn matrix_usage(extraction: bool) -> TokenUsage {
    if extraction {
        TokenUsage {
            prompt_tokens: 11,
            completion_tokens: 7,
            total_tokens: 18,
            cache_read_tokens: None,
            cache_write_tokens: Some(3),
        }
    } else {
        TokenUsage {
            prompt_tokens: 17,
            completion_tokens: 13,
            total_tokens: 30,
            cache_read_tokens: Some(2),
            cache_write_tokens: None,
        }
    }
}
fn assert_matrix_usage(actual: &TokenUsage, extraction: bool) {
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(matrix_usage(extraction)).unwrap()
    );
}

#[derive(Default)]
struct MatrixGuard {
    checks: std::sync::Mutex<Vec<String>>,
    records: std::sync::Mutex<Vec<(String, TokenUsage)>>,
}
#[async_trait::async_trait]
impl BudgetGuard for MatrixGuard {
    async fn check_before_llm(&self, session: &str, estimate: usize) -> BudgetDecision {
        assert!(estimate > 0);
        self.checks.lock().unwrap().push(session.to_owned());
        BudgetDecision::Allow
    }
    async fn record_after_llm(&self, session: &str, usage: &TokenUsage) {
        self.records
            .lock()
            .unwrap()
            .push((session.to_owned(), usage.clone()));
    }
}

#[derive(Default)]
struct MatrixClient {
    main: std::sync::Mutex<Vec<Vec<Message>>>,
    extraction: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl LlmClient for MatrixClient {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        // The original extractor uses this tool-free port. Any main fallback
        // or incidental call makes the exact call/callback oracles fail.
        assert!(tools.is_empty());
        self.extraction
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let output = serde_json::json!({"items":[{
            "memory_type":"semantic", "content":MATRIX_MEMORY,
            "importance":0.95, "confidence":0.98, "tags":["verification"],
            "source":"preference", "scope":"workspace",
            "reason":"The user supplied a durable workspace preference."
        }]});
        Ok(LlmResponse {
            message: Message::assistant(&output.to_string()),
            usage: matrix_usage(true),
            stop_reason: None,
            token_logprobs: vec![],
            meta: None,
        })
    }
    async fn complete_streaming(
        &self,
        messages: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.main.lock().unwrap().push(messages.to_vec());
        let (sender, receiver) = mpsc::channel(2);
        sender
            .try_send(StreamEvent::TextDelta(MATRIX_ANSWER.into()))
            .unwrap();
        sender
            .try_send(StreamEvent::Done(LlmResponse {
                message: Message::assistant(MATRIX_ANSWER),
                usage: matrix_usage(false),
                stop_reason: None,
                token_logprobs: vec![],
                meta: None,
            }))
            .unwrap();
        Ok(receiver)
    }
}

async fn matrix_drain_events(mut receiver: mpsc::Receiver<AgentEvent>, worker: JoinHandle<()>) {
    let mut ends = 0;
    while let Some(event) = receiver.recv().await {
        match event {
            AgentEvent::End { text, .. } => {
                assert_eq!(text, MATRIX_ANSWER);
                ends += 1;
            }
            AgentEvent::Error { message } => panic!("actual facade failed: {message}"),
            _ => {}
        }
    }
    worker.await.unwrap();
    assert_eq!(ends, 1);
}

async fn matrix_join_spawn(spawn: AgentRunSpawn) -> String {
    let AgentRunSpawn::Started { snapshot, worker } = spawn else {
        panic!("fresh identity unexpectedly replayed");
    };
    assert_eq!(snapshot.id, MATRIX_TARGET);
    worker.await.unwrap();
    snapshot.id
}

async fn matrix_scenario(mode: MatrixMode, file_session: bool, file_memory: bool) {
    use a3s_effect::{FileLog, LogStore, NewFact};
    use a3s_memory::{FileMemoryStore, InMemoryStore, MemoryStore};
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    eprintln!("mode={mode:?} file_session={file_session} file_memory={file_memory}");
    let workspace = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let session_dir = tempfile::tempdir().unwrap();
    let memory_dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn SessionStore> = if file_session {
        Arc::new(FileSessionStore::new(session_dir.path()).await.unwrap())
    } else {
        Arc::new(MemorySessionStore::new())
    };
    let memory: Arc<dyn MemoryStore> = if file_memory {
        Arc::new(FileMemoryStore::new(memory_dir.path()).await.unwrap())
    } else {
        Arc::new(InMemoryStore::new())
    };
    let mut config = super::super::test_config();
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: true,
        prune_policy: None,
        ..Default::default()
    });
    let agent = crate::Agent::from_config(config).await.unwrap();
    let client = Arc::new(MatrixClient::default());
    let guard = Arc::new(MatrixGuard::default());
    let session = agent
        .build_session(
            workspace.path().display().to_string(),
            client.clone(),
            &crate::SessionOptions::new()
                .with_session_id(MATRIX_SESSION)
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_memory(memory.clone())
                .with_budget_guard(guard.clone())
                .with_session_store(store.clone())
                .with_auto_save(true),
        )
        .unwrap();
    let supplied_guard: Arc<dyn BudgetGuard> = guard.clone();
    assert!(Arc::ptr_eq(
        session.config.budget_guard.as_ref().unwrap(),
        &supplied_guard
    ));
    let before_health = session.task_scheduler.health().await.unwrap();
    let checkpoint = LoopCheckpoint {
        schema_version: crate::loop_checkpoint::LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: MATRIX_SOURCE.into(),
        session_id: MATRIX_SESSION.into(),
        capability_binding: None,
        standing_context_digest: None,
        durable_source: None,
        turn: 1,
        messages: vec![Message::user(MATRIX_PROMPT)],
        total_usage: Default::default(),
        tool_calls_count: 0,
        verification_reports: vec![],
        convergence: Default::default(),
        checkpoint_ms: 1,
    };
    // Explicit source fixture, not an automatically emitted tool checkpoint.
    // Supplied preparation deliberately receives no stored source fragment.
    let stored_source = mode.checkpoint() && !matches!(mode, MatrixMode::PreparedSupplied);
    if stored_source {
        store
            .save_loop_checkpoint(MATRIX_SOURCE, &checkpoint)
            .await
            .unwrap();
    }
    let source_before =
        serde_json::to_value(store.load_loop_checkpoint(MATRIX_SOURCE).await.unwrap()).unwrap();
    let thread = crate::fact_control::thread_for_session(MATRIX_SESSION);
    let log_dir = crate::session_folder::effect_log_dir(workspace.path(), MATRIX_SESSION);
    if matches!(mode, MatrixMode::OrdinaryLog) {
        FileLog::open(&log_dir)
            .unwrap()
            .append(
                &thread,
                &[NewFact {
                    kind: "user.message".into(),
                    key: "matrix-pending-input".into(),
                    payload: serde_json::json!({"text":MATRIX_PROMPT}),
                }],
                None,
            )
            .unwrap();
    }
    let log_path = log_dir.join(format!("{thread}.jsonl"));
    let prefix = std::fs::read(&log_path).unwrap_or_default();
    let attachment = Attachment::png(vec![137, 80, 78, 71, 1, 2, 3]);
    let returned_id = match mode {
        MatrixMode::Send | MatrixMode::SendAttachments => {
            let result = if mode.attachments() {
                session
                    .send_with_attachments(MATRIX_PROMPT, std::slice::from_ref(&attachment), None)
                    .await
            } else {
                session.send(MATRIX_PROMPT, None).await
            }
            .unwrap();
            assert_eq!(result.text, MATRIX_ANSWER);
            assert_matrix_usage(&result.usage, false);
            None
        }
        MatrixMode::Stream => {
            let stream = session.stream_with_run(MATRIX_PROMPT, None).await.unwrap();
            let id = stream.run.as_ref().unwrap().id().to_owned();
            matrix_drain_events(stream.events, stream.worker).await;
            Some(id)
        }
        MatrixMode::StreamAttachments => {
            let (events, worker) = session
                .stream_with_attachments(MATRIX_PROMPT, std::slice::from_ref(&attachment), None)
                .await
                .unwrap();
            matrix_drain_events(events, worker).await;
            None
        }
        MatrixMode::Detached => Some(
            matrix_join_spawn(
                session
                    .spawn_run_with_id(MATRIX_TARGET, MATRIX_PROMPT)
                    .await
                    .unwrap(),
            )
            .await,
        ),
        MatrixMode::ExactRecovery => Some(
            matrix_join_spawn(
                session
                    .spawn_recovery_with_run_id(MATRIX_SOURCE, MATRIX_TARGET)
                    .await
                    .unwrap(),
            )
            .await,
        ),
        MatrixMode::PreparedStored | MatrixMode::PreparedSupplied => {
            let evidence = SessionLogicalResumeEvidenceV1::from_checkpoint(&checkpoint).unwrap();
            let prepared = if matches!(mode, MatrixMode::PreparedSupplied) {
                prepare_recovery_from_checkpoint(
                    &session,
                    &evidence,
                    "matrix-boundary",
                    MATRIX_TARGET,
                    checkpoint,
                )
                .await
            } else {
                prepare_recovery_with_evidence(
                    &session,
                    &evidence,
                    "matrix-boundary",
                    MATRIX_TARGET,
                )
                .await
            }
            .unwrap();
            assert!(session.run_store.records().await.is_empty());
            assert!(client.main.lock().unwrap().is_empty());
            assert_eq!(client.extraction.load(Ordering::SeqCst), 0);
            assert!(guard.checks.lock().unwrap().is_empty());
            assert!(guard.records.lock().unwrap().is_empty());
            let prepared_health = session.task_scheduler.health().await.unwrap();
            assert_eq!(prepared_health.active, before_health.active + 1);
            assert_eq!(prepared_health.admitted, before_health.admitted + 1);
            assert_eq!(prepared_health.released, before_health.released);
            assert_eq!(std::fs::read(&log_path).unwrap_or_default(), prefix);
            let ExactRecoveryPreparation::Ready(prepared) = prepared else {
                panic!("fresh preparation replayed");
            };
            Some(
                matrix_join_spawn(spawn_prepared_recovery(&session, *prepared).await.unwrap())
                    .await,
            )
        }
        MatrixMode::OrdinaryCheckpoint | MatrixMode::OrdinaryLog => {
            let result = session.resume_run(MATRIX_SOURCE).await.unwrap();
            assert_eq!(result.text, MATRIX_ANSWER);
            assert_matrix_usage(&result.usage, false);
            // Ordinary resume must not admit even a pending retention ticket.
            let drain = session
                .memory()
                .unwrap()
                .drain_llm_extractions(Duration::from_secs(5));
            tokio::pin!(drain);
            assert!(matches!(
                futures::poll!(&mut drain),
                std::task::Poll::Ready(true)
            ));
            None
        }
    };
    // A stream's worker join is not a memory receipt. Wait on the original
    // retention queue, then read canonical state from the selected adapter.
    assert!(
        session
            .memory()
            .unwrap()
            .drain_llm_extractions(Duration::from_secs(5))
            .await
    );
    let expected_extraction = usize::from(!mode.ordinary());
    assert_eq!(
        client.extraction.load(Ordering::SeqCst),
        expected_extraction
    );
    {
        let main = client.main.lock().unwrap();
        assert_eq!(main.len(), 1);
        assert!(main[0]
            .iter()
            .any(|message| message.text().contains(MATRIX_PROMPT)));
        let images: Vec<_> = main[0]
            .iter()
            .flat_map(|message| &message.content)
            .filter_map(|block| {
                if let crate::llm::ContentBlock::Image { source } = block {
                    Some(source)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(images.len(), usize::from(mode.attachments()));
        if mode.attachments() {
            assert_eq!(images[0].data, attachment.base64_data());
            assert_eq!(images[0].media_type, attachment.media_type);
        }
        let records = guard.records.lock().unwrap();
        assert_eq!(records.len(), 1 + expected_extraction);
        for (index, (id, usage)) in records.iter().enumerate() {
            assert_eq!(id, MATRIX_SESSION);
            assert_matrix_usage(usage, index == 1);
        }
        let checks = guard.checks.lock().unwrap();
        assert_eq!(checks.len(), records.len());
        assert!(checks.iter().all(|id| id == MATRIX_SESSION));
    }
    let live_memory = memory.get_recent(10).await.unwrap();
    assert_eq!(live_memory.len(), expected_extraction);
    let live_records = session.run_store.records().await;
    assert_eq!(live_records.len(), 1);
    let live_run = &live_records[0].snapshot;
    assert_ne!(live_run.id, MATRIX_SOURCE);
    if let Some(id) = returned_id {
        assert_eq!(live_run.id, id);
    }
    assert_eq!(live_run.session_id, MATRIX_SESSION);
    assert_eq!(live_run.prompt, mode.prompt());
    assert_eq!(live_run.status, crate::run::RunStatus::Completed);
    let health = session.task_scheduler.health().await.unwrap();
    assert_eq!(health.active, before_health.active);
    assert_eq!(health.admitted, before_health.admitted + 1);
    assert_eq!(health.released, before_health.released + 1);
    let fence = session.run_admission.try_acquire(MATRIX_SESSION).unwrap();
    assert!(fence.is_nonexecuting_fence_for(&session.run_admission));
    drop(fence);

    let read_store: Arc<dyn SessionStore> = if file_session {
        Arc::new(FileSessionStore::new(session_dir.path()).await.unwrap())
    } else {
        store
    };
    let snapshot = read_store
        .load_snapshot(MATRIX_SESSION)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(snapshot.session.id, MATRIX_SESSION);
    assert_matrix_usage(&snapshot.session.total_usage, false);
    assert_eq!(snapshot.run_records.len(), 1);
    assert_eq!(
        serde_json::to_value(&snapshot.run_records).unwrap(),
        serde_json::to_value(&live_records).unwrap()
    );
    let ends: Vec<_> = snapshot.run_records[0]
        .events
        .iter()
        .filter_map(|record| {
            if let AgentEvent::End { text, usage, .. } = &record.event {
                Some((text, usage))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].0, MATRIX_ANSWER);
    assert_matrix_usage(ends[0].1, false);
    let events = &snapshot.run_records[0].events;
    let inputs: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(index, record)| match &record.event {
            AgentEvent::ModelInputBound { snapshot } => Some((index, snapshot)),
            _ => None,
        })
        .collect();
    let usages: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(index, record)| match &record.event {
            AgentEvent::ModelUsageBound { snapshot } => Some((index, snapshot)),
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 1);
    assert_eq!(usages.len(), 1);
    let (input_index, input) = inputs[0];
    let (usage_index, usage) = usages[0];
    usage.validate_against(input).unwrap();
    assert_eq!(input.image_block_count, usize::from(mode.attachments()));
    assert_matrix_usage(
        &TokenUsage {
            prompt_tokens: usage.reported_prompt_tokens,
            completion_tokens: usage.reported_completion_tokens,
            total_tokens: usage.reported_total_tokens,
            cache_read_tokens: usage.reported_cache_read_tokens,
            cache_write_tokens: usage.reported_cache_write_tokens,
        },
        false,
    );
    let end_index = events
        .iter()
        .position(|record| matches!(record.event, AgentEvent::End { .. }))
        .unwrap();
    assert!(input_index < usage_index && usage_index < end_index);
    assert!(!snapshot.run_records[0]
        .events
        .iter()
        .any(|record| matches!(record.event, AgentEvent::Error { .. })));
    assert_eq!(
        serde_json::to_value(
            read_store
                .load_loop_checkpoint(MATRIX_SOURCE)
                .await
                .unwrap()
        )
        .unwrap(),
        source_before
    );
    // Pure-text completion does not claim an automatic tool-round checkpoint.
    let read_memory: Arc<dyn MemoryStore> = if file_memory {
        Arc::new(FileMemoryStore::new(memory_dir.path()).await.unwrap())
    } else {
        memory
    };
    let items = read_memory.get_recent(10).await.unwrap();
    assert_eq!(items.len(), expected_extraction);
    if let Some(item) = items.first() {
        assert_eq!(item.id, live_memory[0].id);
        assert_eq!(item.content, MATRIX_MEMORY);
        assert_eq!(
            item.metadata.get("session_id").map(String::as_str),
            Some(MATRIX_SESSION)
        );
        assert_eq!(
            item.metadata.get("workspace").map(String::as_str),
            workspace.path().to_str()
        );
        assert_eq!(
            item.metadata.get("schema").map(String::as_str),
            Some("a3s.memory.durable.v1")
        );
    }
    assert!(std::fs::read(log_path).unwrap().starts_with(&prefix));
    let facts = crate::fact_control::read_workspace_facts(workspace.path(), &thread).unwrap();
    let decisions: Vec<_> = facts
        .iter()
        .filter(|fact| fact.kind == "model.turn")
        .collect();
    assert_eq!(decisions.len(), 1);
    assert_eq!(
        serde_json::from_value::<a3s_effect::coding::ModelDecision>(decisions[0].payload.clone())
            .unwrap(),
        a3s_effect::coding::ModelDecision::Text {
            text: MATRIX_ANSWER.into()
        }
    );
    let users: Vec<_> = facts
        .iter()
        .filter(|fact| fact.kind == "user.message")
        .collect();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].payload["text"], MATRIX_PROMPT);
    if mode.attachments() {
        assert_eq!(
            users[0].payload["images"][0]["data"],
            attachment.base64_data()
        );
        assert_eq!(
            users[0].payload["images"][0]["media_type"],
            attachment.media_type
        );
    }
    session.close().await;
}

macro_rules! matrix_test {
    ($name:ident, $mode:ident) => {
        #[tokio::test]
        async fn $name() {
            for file_session in [false, true] {
                for file_memory in [false, true] {
                    tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        matrix_scenario(MatrixMode::$mode, file_session, file_memory),
                    )
                    .await
                    .unwrap();
                }
            }
        }
    };
}
matrix_test!(both_adapters_send, Send);
matrix_test!(both_adapters_send_attachments, SendAttachments);
matrix_test!(both_adapters_stream, Stream);
matrix_test!(both_adapters_stream_attachments, StreamAttachments);
matrix_test!(both_adapters_detached, Detached);
matrix_test!(both_adapters_exact_recovery, ExactRecovery);
matrix_test!(both_adapters_prepared_stored, PreparedStored);
matrix_test!(both_adapters_prepared_supplied, PreparedSupplied);
matrix_test!(both_adapters_ordinary_checkpoint, OrdinaryCheckpoint);
matrix_test!(both_adapters_ordinary_log, OrdinaryLog);
