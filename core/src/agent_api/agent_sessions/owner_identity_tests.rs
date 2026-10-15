#![cfg(test)]

use super::*;
use crate::agent_api::execution_coordinator::ExecutionCoordinator;
use crate::agent_api::run_admission::{ExecutionMode, OriginalExecutionContext, OwnerDisposition};
use crate::agent_api::run_lifecycle::RunControlState;
use crate::hooks::{HookEvent, HookExecutor, HookResult};
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use crate::store::{FileSessionStore, MemorySessionStore, SessionStore};
use crate::task_scheduler::{TaskSchedulerConfig, TaskSchedulerHealthSnapshot};
use a3s_effect::{FileLog, LogStore, NewFact};
use std::sync::atomic::AtomicUsize;
use tokio::sync::{oneshot, Semaphore};

#[derive(Default)]
struct NoDispatch(AtomicUsize);

#[async_trait::async_trait]
impl LlmClient for NoDispatch {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("identity component must not dispatch a model")
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<tokio::sync::mpsc::Receiver<StreamEvent>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("identity component must not dispatch a model")
    }
}

#[derive(Debug)]
struct CloseHook {
    entered: Semaphore,
    release: Semaphore,
    block_end: bool,
    cancelled: AtomicUsize,
}

impl CloseHook {
    fn new(block_end: bool) -> Arc<Self> {
        Arc::new(Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            block_end,
            cancelled: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl HookExecutor for CloseHook {
    async fn fire(&self, event: &HookEvent) -> HookResult {
        if matches!(event, HookEvent::SessionEnd(_)) {
            self.entered.add_permits(1);
            if self.block_end {
                self.release.acquire().await.unwrap().forget();
            }
        }
        HookResult::continue_()
    }

    async fn record_run_cancelled(&self, _: &str, _: &str, _: Option<&str>) {
        self.cancelled.fetch_add(1, Ordering::SeqCst);
    }
}

struct Fixture {
    workspace: tempfile::TempDir,
    agent: Agent,
    session: AgentSession,
    store: Arc<dyn SessionStore>,
    model: Arc<NoDispatch>,
    scheduler_baseline: TaskSchedulerHealthSnapshot,
}

async fn stores() -> Vec<(Arc<dyn SessionStore>, Option<tempfile::TempDir>)> {
    let files = tempfile::tempdir().unwrap();
    vec![
        (Arc::new(MemorySessionStore::new()), None),
        (
            Arc::new(FileSessionStore::new(files.path()).await.unwrap()),
            Some(files),
        ),
    ]
}

fn options(store: Arc<dyn SessionStore>, model: Arc<NoDispatch>, id: &str) -> SessionOptions {
    SessionOptions::new()
        .with_session_id(id)
        .with_session_store(store)
        .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
        .with_llm_client(model)
}

async fn fixture(store: Arc<dyn SessionStore>, id: &str, hook: Option<Arc<CloseHook>>) -> Fixture {
    let workspace = tempfile::tempdir().unwrap();
    let mut config = crate::agent_api::test_config();
    config.task_scheduler = TaskSchedulerConfig {
        max_active: 1,
        aging_interval_ms: 60_000,
    };
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        prune_policy: None,
        ..Default::default()
    });
    let agent = Agent::from_config(config).await.unwrap();
    let model = Arc::new(NoDispatch::default());
    let mut opts = options(store.clone(), model.clone(), id);
    if let Some(hook) = hook {
        opts = opts.with_hook_executor(hook);
    }
    // This existing fixture goes through create_session's REAL registry.
    let session = agent
        .build_session(workspace.path().display().to_string(), model.clone(), &opts)
        .unwrap();
    session.save().await.unwrap();
    let scheduler_baseline = session.task_scheduler_health().await.unwrap();
    assert_eq!(scheduler_baseline.max_active, 1);
    assert_eq!(
        (scheduler_baseline.active, scheduler_baseline.pending),
        (0, 0)
    );
    Fixture {
        workspace,
        agent,
        session,
        store,
        model,
        scheduler_baseline,
    }
}

fn registered(agent: &Agent, id: &str) -> Arc<SessionCloseHandle> {
    let registry = agent.sessions.lock().unwrap();
    match registry.entries.get(id).unwrap() {
        SessionRegistryEntry::Live(weak) => weak.upgrade().unwrap(),
        SessionRegistryEntry::Replacing { current, .. } => current.upgrade().unwrap(),
        SessionRegistryEntry::Building(_) => panic!("original must remain live"),
    }
}

async fn readback(store: &Arc<dyn SessionStore>, files: &Option<tempfile::TempDir>, id: &str) {
    let expected = serde_json::to_value(store.load_snapshot(id).await.unwrap()).unwrap();
    assert!(!expected.is_null());
    if let Some(files) = files {
        let reopened = FileSessionStore::new(files.path()).await.unwrap();
        assert_eq!(
            serde_json::to_value(reopened.load_snapshot(id).await.unwrap()).unwrap(),
            expected
        );
    }
}

async fn draft(f: &Fixture) -> AgentSession {
    build_resumed_session(
        &f.agent,
        f.session.session_id(),
        options(f.store.clone(), f.model.clone(), f.session.session_id()),
        None,
    )
    .await
    .unwrap()
}

async fn entered_owner(
    session: &AgentSession,
) -> (
    Arc<OriginalExecutionContext>,
    oneshot::Sender<()>,
    oneshot::Receiver<OwnerDisposition>,
    tokio::task::JoinHandle<()>,
    tokio_util::sync::CancellationToken,
) {
    let lease = ExecutionCoordinator::admit(session, "identity-component")
        .await
        .unwrap();
    let cancellation = session.session_cancel.child_token();
    let (handoff, start, original) = ExecutionCoordinator::prepare_original_handoff(
        session,
        lease,
        ExecutionMode::Blocking,
        cancellation.clone(),
    )
    .unwrap();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        entered_tx.send(()).unwrap();
        let _ = release_rx.await;
    });
    let (observed_tx, observed_rx) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), observed_tx);
    entered_rx.await.unwrap();
    (original, release_tx, observed_rx, proxy, cancellation)
}

// The actual Created/bind/attach producers must run on the SAME worker whose
// real task ID was retained by the handoff, never on this test's caller task.
async fn entered_control_owner(
    session: &AgentSession,
) -> (
    oneshot::Sender<()>,
    oneshot::Receiver<OwnerDisposition>,
    tokio::task::JoinHandle<()>,
    tokio_util::sync::CancellationToken,
    crate::run::RunHandle,
    Arc<crate::run_control::RunControlInbox>,
) {
    let lease = ExecutionCoordinator::admit(session, "identity-controls")
        .await
        .unwrap();
    let cancellation = session.session_cancel.child_token();
    let (handoff, start, original) = ExecutionCoordinator::prepare_original_handoff(
        session,
        lease,
        ExecutionMode::Blocking,
        cancellation.clone(),
    )
    .unwrap();
    let state = RunControlState::from_session(session)
        .bind_original(original.clone())
        .unwrap();
    let label = session.session_id().to_string();
    let worker_token = cancellation.clone();
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let worker = tokio::spawn(async move {
        assert!(start.wait().await.unwrap());
        let handle = state.start_run("exact original input").await;
        let inbox = crate::run_control::RunControlInbox::new(label, handle.id(), worker_token);
        original.bind_inbox_once(inbox.clone()).unwrap();
        state
            .attach_owned_run_control(handle.id(), inbox.clone())
            .await
            .unwrap();
        entered_tx.send((handle, inbox)).unwrap();
        let _ = release_rx.await;
    });
    let (observed_tx, observed_rx) = oneshot::channel();
    let proxy = handoff.supervise(worker, Vec::new(), observed_tx);
    let (handle, inbox) = entered_rx.await.unwrap();
    (release_tx, observed_rx, proxy, cancellation, handle, inbox)
}

#[tokio::test(start_paused = true)]
async fn joined_unproved_original_survives_last_session_drop_and_refuses_both_identity_ingresses() {
    for (store, files) in stores().await {
        let f = fixture(store, "retained-identity", None).await;
        let id = f.session.session_id().to_string();
        let close = Arc::downgrade(&f.session.close_handle);
        let log_dir = crate::session_folder::effect_log_dir(f.workspace.path(), &id);
        let thread = crate::fact_control::thread_for_session(&id);
        let log = FileLog::open(&log_dir).unwrap();
        log.append(
            &thread,
            &[NewFact {
                kind: "user.message".into(),
                key: "retained-original-input".into(),
                payload: serde_json::json!({"text": "same exact original input"}),
            }],
            None,
        )
        .unwrap();
        let path = log_dir.join(format!("{thread}.jsonl"));
        let before = std::fs::read(&path).unwrap();
        let (original, release, observed, proxy, _) = entered_owner(&f.session).await;
        release.send(()).unwrap();
        assert_eq!(observed.await.unwrap(), OwnerDisposition::RetainedUnproved);
        f.session.close().await;
        let Fixture {
            agent,
            session,
            workspace,
            store,
            model,
            scheduler_baseline,
        } = f;
        drop(session);
        assert!(
            close.upgrade().is_some(),
            "actual guardian's pin owns the Weak identity"
        );
        assert!(Arc::ptr_eq(
            &registered(&agent, &id),
            &close.upgrade().unwrap()
        ));
        assert_eq!(agent.list_sessions().await, vec![id.clone()]);
        assert!(agent
            .session_async(
                workspace.path().display().to_string(),
                Some(options(store.clone(), model.clone(), &id))
            )
            .await
            .is_err());
        assert!(agent
            .resume_session_async(&id, options(store.clone(), model.clone(), &id))
            .await
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let health = agent.task_scheduler_health().await.unwrap();
        assert_eq!(
            (health.active, health.admitted, health.released),
            (
                scheduler_baseline.active + 1,
                scheduler_baseline.admitted + 1,
                scheduler_baseline.released,
            )
        );
        assert!(!proxy.is_finished());
        assert!(!original.request_session_close().await);
        assert_eq!(model.0.load(Ordering::SeqCst), 0);
        readback(&store, &files, &id).await;
    }
}

#[tokio::test]
async fn replacement_reservation_holds_actual_fence_and_refuses_racing_admission_before_task_acquire(
) {
    for (store, files) in stores().await {
        let f = fixture(store, "fenced-replacement", None).await;
        let reservation = reserve_session_replacement(&f.agent, &f.session).unwrap();
        assert!(reservation
            .fence
            .is_nonexecuting_fence_for(&f.session.run_admission));
        assert!(matches!(
            ExecutionCoordinator::admit(&f.session, "racing-replacement").await,
            Err(CodeError::SessionBusy { .. })
        ));
        let health = f.agent.task_scheduler_health().await.unwrap();
        assert_eq!(
            (
                health.active,
                health.pending,
                health.admitted,
                health.released,
                health.cancelled,
                health.rejected,
            ),
            (
                f.scheduler_baseline.active,
                f.scheduler_baseline.pending,
                f.scheduler_baseline.admitted,
                f.scheduler_baseline.released,
                f.scheduler_baseline.cancelled,
                f.scheduler_baseline.rejected,
            )
        );
        assert!(Arc::ptr_eq(
            &registered(&f.agent, f.session.session_id()),
            &f.session.close_handle
        ));
        drop(reservation);
        let next = f
            .session
            .run_admission
            .try_acquire(f.session.session_id())
            .unwrap();
        assert!(next.is_nonexecuting_fence_for(&f.session.run_admission));
        drop(next);
        readback(&f.store, &files, f.session.session_id()).await;
    }
}

#[tokio::test]
async fn cancelled_replacement_drop_restores_closed_original_before_fence_release() {
    for (store, _) in stores().await {
        let hook = CloseHook::new(true);
        let f = fixture(store, "cancelled-replacement", Some(hook.clone())).await;
        let reservation = reserve_session_replacement(&f.agent, &f.session).unwrap();
        let replacement = draft(&f).await;
        let mut finalize = Box::pin(reservation.finalize(&replacement.close_handle));
        tokio::select! {
            entered = hook.entered.acquire() => entered.unwrap().forget(),
            result = &mut finalize => panic!("close returned before its actual hook barrier: {result:?}"),
        }
        assert!(f.session.is_closed());
        assert!(f
            .session
            .run_admission
            .try_acquire(f.session.session_id())
            .is_err());
        // Cancel the ACTUAL partially polled replacement future. Its owned
        // reservation rolls back even though old close already set closed.
        drop(finalize);
        assert!(Arc::ptr_eq(
            &registered(&f.agent, f.session.session_id()),
            &f.session.close_handle
        ));
        assert!(matches!(
            f.agent
                .sessions
                .lock()
                .unwrap()
                .entries
                .get(f.session.session_id()),
            Some(SessionRegistryEntry::Live(_))
        ));
        let fence = f
            .session
            .run_admission
            .try_acquire(f.session.session_id())
            .unwrap();
        assert!(fence.is_nonexecuting_fence_for(&f.session.run_admission));
        assert!(!replacement.is_closed());
    }
}

#[tokio::test]
async fn replacement_exposes_new_handle_only_after_actual_original_close_returns() {
    for (store, files) in stores().await {
        let hook = CloseHook::new(true);
        let f = fixture(store, "close-before-switch", Some(hook.clone())).await;
        let reservation = reserve_session_replacement(&f.agent, &f.session).unwrap();
        let replacement = draft(&f).await;
        let replacement_handle = replacement.close_handle.clone();
        let finalize = tokio::spawn(async move { reservation.finalize(&replacement_handle).await });
        hook.entered.acquire().await.unwrap().forget();
        assert!(f.session.is_closed());
        assert!(!replacement.is_closed());
        assert!(Arc::ptr_eq(
            &registered(&f.agent, f.session.session_id()),
            &f.session.close_handle
        ));
        assert_eq!(
            f.agent.list_sessions().await,
            vec![f.session.session_id().to_string()]
        );
        assert!(f
            .session
            .run_admission
            .try_acquire(f.session.session_id())
            .is_err());
        hook.release.add_permits(1);
        finalize.await.unwrap().unwrap();
        assert!(Arc::ptr_eq(
            &registered(&f.agent, f.session.session_id()),
            &replacement.close_handle
        ));
        assert!(f
            .session
            .run_admission
            .try_acquire(f.session.session_id())
            .is_ok());
        assert_eq!(f.model.0.load(Ordering::SeqCst), 0);
        readback(&f.store, &files, f.session.session_id()).await;
    }
}

#[tokio::test]
async fn registry_close_during_replacement_invalidates_token_without_original_identity_vacancy() {
    for (store, _) in stores().await {
        let hook = CloseHook::new(true);
        let f = fixture(store, "external-close", Some(hook.clone())).await;
        let reservation = reserve_session_replacement(&f.agent, &f.session).unwrap();
        let replacement = draft(&f).await;
        let replacement_handle = replacement.close_handle.clone();
        let finalize = tokio::spawn(async move { reservation.finalize(&replacement_handle).await });
        hook.entered.acquire().await.unwrap().forget();
        assert!(!f.agent.close_session(f.session.session_id()).await);
        assert!(Arc::ptr_eq(
            &registered(&f.agent, f.session.session_id()),
            &f.session.close_handle
        ));
        assert!(matches!(
            f.agent
                .sessions
                .lock()
                .unwrap()
                .entries
                .get(f.session.session_id()),
            Some(SessionRegistryEntry::Live(_))
        ));
        assert!(reserve_session(&f.agent, f.session.session_id()).is_err());
        hook.release.add_permits(1);
        assert!(finalize.await.unwrap().is_err());
        assert!(Arc::ptr_eq(
            &registered(&f.agent, f.session.session_id()),
            &f.session.close_handle
        ));
        replacement.close().await;
    }
}

#[tokio::test(start_paused = true)]
async fn agent_close_invalidates_replacement_and_rollback_retains_original_weak() {
    for (store, _) in stores().await {
        let f = fixture(store, "agent-close-replacement", None).await;
        let reservation = reserve_session_replacement(&f.agent, &f.session).unwrap();
        let replacement = draft(&f).await;
        f.agent.close().await;
        assert!(reservation
            .finalize(&replacement.close_handle)
            .await
            .is_err());
        assert!(Arc::ptr_eq(
            &registered(&f.agent, f.session.session_id()),
            &f.session.close_handle
        ));
        assert!(reserve_session(&f.agent, f.session.session_id()).is_err());
        replacement.close().await;
    }
}

#[tokio::test(start_paused = true)]
async fn stale_reservation_drop_cannot_erase_registry_close_restored_original() {
    for (store, _) in stores().await {
        let f = fixture(store, "stale-replacement", None).await;
        let reservation = reserve_session_replacement(&f.agent, &f.session).unwrap();
        assert!(f.agent.close_session(f.session.session_id()).await);
        assert!(Arc::ptr_eq(
            &registered(&f.agent, f.session.session_id()),
            &f.session.close_handle
        ));
        drop(reservation);
        assert!(Arc::ptr_eq(
            &registered(&f.agent, f.session.session_id()),
            &f.session.close_handle
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn close_preserves_accepted_controls_and_run_slots_and_drains_memory_before_parent_cancel() {
    for (store, files) in stores().await {
        let hook = CloseHook::new(false);
        let f = fixture(store, "close-original-only", Some(hook.clone())).await;
        let (release, observed, proxy, actual_token, handle, inbox) =
            entered_control_owner(&f.session).await;
        let request = crate::run_control::RunControlRequest::steer(
            handle.id(),
            "preserve exact accepted payload",
        );
        let accepted = inbox.submit(request.clone(), 101).await.unwrap();
        assert_eq!(
            accepted.state,
            crate::run_control::RunControlReceiptState::Accepted
        );
        let mut before = serde_json::to_value(inbox.snapshot().await).unwrap();
        assert_eq!(before["active"], true);
        // Close seals admission, while preserving every accepted control and
        // original slot. Sealing is not proof that the queue has drained.
        before["active"] = serde_json::Value::Bool(false);
        let ticket = f.session.memory.as_ref().unwrap().enqueue_llm_extraction();
        let mut close = Box::pin(f.session.close_handle.close());
        assert!(futures::poll!(close.as_mut()).is_pending());
        assert!(f.session.is_closed());
        assert!(!f.session.session_cancel.is_cancelled());
        assert!(!actual_token.is_cancelled());
        drop(ticket);
        close.await;
        assert!(f.session.session_cancel.is_cancelled());
        assert!(actual_token.is_cancelled());
        assert_eq!(handle.status().await, Some(crate::run::RunStatus::Created));
        assert_eq!(
            f.session.current_run_id.lock().await.as_deref(),
            Some(handle.id())
        );
        assert!(Arc::ptr_eq(
            f.session.active_run_control.lock().await.as_ref().unwrap(),
            &inbox
        ));
        assert!(f.session.cancel_token.lock().await.is_some());
        assert_eq!(
            serde_json::to_value(inbox.snapshot().await).unwrap(),
            before
        );
        assert_eq!(inbox.submit(request, 102).await.unwrap(), accepted);
        assert_eq!(hook.cancelled.load(Ordering::SeqCst), 0);
        release.send(()).unwrap();
        assert_eq!(observed.await.unwrap(), OwnerDisposition::RetainedUnproved);
        assert!(!proxy.is_finished());
        assert_eq!(f.model.0.load(Ordering::SeqCst), 0);
        readback(&f.store, &files, f.session.session_id()).await;
    }
}

#[tokio::test]
async fn replacement_close_refuses_foreign_fence_and_poisoned_boundary_before_effects() {
    let f = fixture(Arc::new(MemorySessionStore::new()), "foreign-fence", None).await;
    let other = fixture(Arc::new(MemorySessionStore::new()), "other-fence", None).await;
    let foreign = other
        .session
        .run_admission
        .try_acquire(other.session.session_id())
        .unwrap();
    assert!(f
        .session
        .close_handle
        .close_for_replacement(&foreign)
        .await
        .is_err());
    assert!(!f.session.is_closed());
    assert!(!f.session.session_cancel.is_cancelled());
    let fence = f
        .session
        .run_admission
        .try_acquire(f.session.session_id())
        .unwrap();
    let boundary = f.session.close_handle.clone();
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = boundary.immediate_extension_mutation.lock().unwrap();
        panic!("actual original boundary poison");
    }))
    .is_err());
    assert!(f
        .session
        .close_handle
        .close_for_replacement(&fence)
        .await
        .is_err());
    assert!(!f.session.is_closed());
    assert!(!f.session.session_cancel.is_cancelled());
    assert_eq!(f.model.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn nonexecuting_close_and_replacement_futures_are_send_and_closed_idle_identity_can_be_reused(
) {
    fn send<T: Send>(value: T) -> T {
        value
    }
    for (store, files) in stores().await {
        let f = fixture(store, "closed-idle-reuse", None).await;
        let fence = f
            .session
            .run_admission
            .try_acquire(f.session.session_id())
            .unwrap();
        send(f.session.close_handle.close_for_replacement(&fence))
            .await
            .unwrap();
        assert!(fence.is_nonexecuting_fence_for(&f.session.run_admission));
        assert_eq!(
            f.agent.list_sessions().await,
            vec![f.session.session_id().to_string()]
        );
        drop(fence);
        assert!(f.agent.list_sessions().await.is_empty());
        let resumed = f
            .agent
            .resume_session_async(
                f.session.session_id(),
                options(f.store.clone(), f.model.clone(), f.session.session_id()),
            )
            .await
            .unwrap();
        assert!(!resumed.is_closed());
        assert!(!Arc::ptr_eq(&resumed.close_handle, &f.session.close_handle));
        let reservation = reserve_session_replacement(&f.agent, &resumed).unwrap();
        // Compile the actual owned reservation future, then cancel before its
        // first poll. Drop must restore this same original and release fence.
        let future = send(reservation.finalize(&f.session.close_handle));
        drop(future);
        assert!(Arc::ptr_eq(
            &registered(&f.agent, resumed.session_id()),
            &resumed.close_handle
        ));
        readback(&f.store, &files, resumed.session_id()).await;
    }
}
