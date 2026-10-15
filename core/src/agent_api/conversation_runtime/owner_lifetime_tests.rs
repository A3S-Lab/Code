#![cfg(test)]

use super::*;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::commands::{CommandContext, CommandOutput, SlashCommand};
use crate::llm::{LlmClient, LlmResponse, StreamEvent, TokenUsage, ToolDefinition};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;

#[derive(Default)]
struct ActualClient {
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl LlmClient for ActualClient {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        panic!("successful actual streaming call must not fallback");
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(1);
        tx.send(StreamEvent::Done(LlmResponse {
            message: Message::assistant("actual owned response"),
            usage: TokenUsage {
                prompt_tokens: 17,
                completion_tokens: 13,
                total_tokens: 30,
                cache_read_tokens: Some(2),
                cache_write_tokens: None,
            },
            stop_reason: None,
            token_logprobs: Vec::new(),
            meta: None,
        }))
        .await
        .unwrap();
        Ok(rx)
    }
}

struct ActualGuard {
    records: std::sync::Mutex<Vec<(String, TokenUsage)>>,
    entered: Semaphore,
    release: Semaphore,
    returned: Semaphore,
    block: bool,
}
impl ActualGuard {
    fn new(block: bool) -> Arc<Self> {
        Arc::new(Self {
            records: Default::default(),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            returned: Semaphore::new(0),
            block,
        })
    }
}
#[async_trait::async_trait]
impl BudgetGuard for ActualGuard {
    async fn check_before_llm(&self, _: &str, _: usize) -> BudgetDecision {
        BudgetDecision::Allow
    }
    async fn record_after_llm(&self, session: &str, usage: &TokenUsage) {
        self.records
            .lock()
            .unwrap()
            .push((session.to_owned(), usage.clone()));
        self.entered.add_permits(1);
        if self.block {
            self.release.acquire().await.unwrap().forget();
        }
        self.returned.add_permits(1);
    }
}

async fn fixture(
    label: &str,
    guard: Arc<ActualGuard>,
) -> (
    super::super::Agent,
    Arc<AgentSession>,
    tempfile::TempDir,
    Arc<ActualClient>,
) {
    let workspace = tempfile::tempdir().unwrap();
    let mut config = super::super::tests::test_config();
    // This fixture proves the main call and its original guard. Incidental
    // extraction would be a separate non-streaming provider call.
    config.memory = Some(crate::memory::MemoryConfig {
        llm_extraction: false,
        ..Default::default()
    });
    let agent = super::super::Agent::from_config(config).await.unwrap();
    let client = Arc::new(ActualClient::default());
    let session = Arc::new(
        agent
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &super::super::SessionOptions::new()
                    .with_session_id(label)
                    .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                    .with_budget_guard(guard)
                    .with_session_store(Arc::new(crate::store::MemorySessionStore::new())),
            )
            .unwrap(),
    );
    (agent, session, workspace, client)
}

async fn permit(actual: &Semaphore) {
    tokio::time::timeout(std::time::Duration::from_secs(5), actual.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}

struct ActualCommand {
    tasks: Arc<std::sync::Mutex<Vec<tokio::task::Id>>>,
}
impl SlashCommand for ActualCommand {
    fn name(&self) -> &str {
        "original_worker"
    }
    fn description(&self) -> &str {
        "original canonical command worker boundary"
    }
    fn execute(&self, _: &str, _: &CommandContext) -> CommandOutput {
        self.tasks.lock().unwrap().push(tokio::task::id());
        CommandOutput::text("actual command output")
    }
}

#[tokio::test]
async fn canonical_commands_execute_in_one_original_worker_without_a_fact_run() {
    for streaming in [false, true] {
        let guard = ActualGuard::new(false);
        let (_agent, session, _workspace, client) =
            fixture("canonical-command", guard.clone()).await;
        let tasks = Arc::new(std::sync::Mutex::new(Vec::new()));
        session
            .register_command(Arc::new(ActualCommand {
                tasks: tasks.clone(),
            }))
            .unwrap();
        let caller = tokio::task::try_id();
        if streaming {
            let mut stream = session
                .stream_with_run("/original_worker", None)
                .await
                .unwrap();
            assert!(stream.run.is_none());
            assert!(
                matches!(stream.events.recv().await, Some(AgentEvent::TextDelta { text }) if text == "actual command output")
            );
            assert!(matches!(
                stream.events.recv().await,
                Some(AgentEvent::End { .. })
            ));
            assert!(stream.events.recv().await.is_none());
            tokio::time::timeout(std::time::Duration::from_secs(5), stream.worker)
                .await
                .unwrap()
                .unwrap();
        } else {
            assert_eq!(
                session.send("/original_worker", None).await.unwrap().text,
                "actual command output"
            );
            // Normal blocking return observes guardian retirement: a second
            // operation cannot race an unreleased admission lease.
            assert_eq!(
                session.send("/original_worker", None).await.unwrap().text,
                "actual command output"
            );
        }
        {
            let tasks = tasks.lock().unwrap();
            assert_eq!(tasks.len(), if streaming { 1 } else { 2 });
            assert!(tasks.iter().all(|actual| Some(*actual) != caller));
            assert_eq!(client.calls.load(Ordering::SeqCst), 0);
            assert!(guard.records.lock().unwrap().is_empty());
        }
        assert!(session.run_store.list().await.is_empty());
    }
}

#[tokio::test]
async fn canonical_blocking_returns_after_actual_original_retirement_and_keeps_governance() {
    let guard = ActualGuard::new(false);
    let (_agent, session, _workspace, client) = fixture("canonical-main", guard.clone()).await;
    for _ in 0..2 {
        let result = session.send("actual prompt", None).await.unwrap();
        assert_eq!(result.text, "actual owned response");
        assert_eq!(result.usage.prompt_tokens, 17);
        assert_eq!(result.usage.completion_tokens, 13);
        assert_eq!(result.usage.total_tokens, 30);
        assert_eq!(result.usage.cache_read_tokens, Some(2));
        assert_eq!(result.usage.cache_write_tokens, None);
        assert!(session.current_run().await.is_none());
    }
    assert_eq!(client.calls.load(Ordering::SeqCst), 2);
    {
        let records = guard.records.lock().unwrap();
        assert_eq!(records.len(), 2);
        for (label, actual) in records.iter() {
            assert_eq!(label, "canonical-main");
            assert_eq!(
                (
                    actual.prompt_tokens,
                    actual.completion_tokens,
                    actual.total_tokens
                ),
                (17, 13, 30)
            );
            assert_eq!(
                (actual.cache_read_tokens, actual.cache_write_tokens),
                (Some(2), None)
            );
        }
    }
    let runs = session.run_store.list().await;
    assert_eq!(runs.len(), 2);
    assert_ne!(runs[0].id, runs[1].id);
    assert!(runs
        .iter()
        .all(|run| run.status == crate::run::RunStatus::Completed));
}

#[tokio::test]
async fn lost_blocking_caller_and_last_session_drop_retain_actual_callback_and_task_lease() {
    let guard = ActualGuard::new(true);
    let (_agent, session, _workspace, client) =
        fixture("canonical-lost-caller", guard.clone()).await;
    let scheduler = Arc::clone(&session.task_scheduler);
    let weak_session = Arc::downgrade(&session);
    let caller_session = session.clone();
    let caller = tokio::spawn(async move { caller_session.send("actual prompt", None).await });
    permit(&guard.entered).await;
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
    let health = scheduler.health().await.unwrap();
    assert_eq!((health.active, health.released), (1, 0));
    caller.abort(); // Only this public observer task; never the original worker.
    assert!(caller.await.unwrap_err().is_cancelled());
    drop(session);
    assert!(weak_session.upgrade().is_none());
    let health = scheduler.health().await.unwrap();
    assert_eq!((health.active, health.released), (1, 0));
    assert_eq!(guard.records.lock().unwrap().len(), 1);
    guard.release.add_permits(1);
    permit(&guard.returned).await;
    assert_eq!(guard.records.lock().unwrap().len(), 1);
    // Callback return is not external billing durability or owner release.
}

fn checkpoint(session: &AgentSession, source: &str) -> LoopCheckpoint {
    LoopCheckpoint {
        schema_version: crate::loop_checkpoint::LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: source.to_owned(),
        session_id: session.session_id.clone(),
        capability_binding: None,
        standing_context_digest: None,
        durable_source: None,
        turn: 1,
        messages: vec![
            Message::user("resume original"),
            Message::assistant("prior"),
        ],
        total_usage: Default::default(),
        tool_calls_count: 0,
        verification_reports: Vec::new(),
        convergence: Default::default(),
        checkpoint_ms: 1,
    }
}

#[tokio::test]
async fn prepared_recovery_from_another_admission_refuses_before_target_mutation() {
    let (_agent_a, first, _workspace_a, first_client) =
        fixture("same-label", ActualGuard::new(false)).await;
    let (_agent_b, second, _workspace_b, second_client) =
        fixture("same-label", ActualGuard::new(false)).await;
    let checkpoint = checkpoint(&first, "original-source");
    let evidence = SessionLogicalResumeEvidenceV1::from_checkpoint(&checkpoint).unwrap();
    let prepared = match prepare_recovery_from_checkpoint(
        &first,
        &evidence,
        "boundary",
        "new-target",
        checkpoint,
    )
    .await
    .unwrap()
    {
        ExactRecoveryPreparation::Ready(actual) => *actual,
        ExactRecoveryPreparation::Replayed(_) => panic!("fresh target unexpectedly replayed"),
    };
    assert!(spawn_prepared_recovery(&second, prepared).await.is_err());
    assert!(first.run_store.list().await.is_empty());
    assert!(second.run_store.list().await.is_empty());
    assert_eq!(first_client.calls.load(Ordering::SeqCst), 0);
    assert_eq!(second_client.calls.load(Ordering::SeqCst), 0);
    let first_fence = first.run_admission.try_acquire(&first.session_id).unwrap();
    assert!(first_fence.is_nonexecuting_fence_for(&first.run_admission));
    let second_fence = second
        .run_admission
        .try_acquire(&second.session_id)
        .unwrap();
    assert!(second_fence.is_nonexecuting_fence_for(&second.run_admission));
}

#[tokio::test]
async fn exact_invalid_identity_finishes_the_known_unconstructed_branch_without_a_model() {
    let (_agent, session, _workspace, client) =
        fixture("canonical-invalid-exact", ActualGuard::new(false)).await;
    let thread = crate::fact_control::thread_for_session(&session.session_id);
    let before = crate::fact_control::read_workspace_facts(&session.workspace, &thread).unwrap();
    let lease = ExecutionCoordinator::admit(&session, "actual invalid exact identity")
        .await
        .unwrap();
    let observation = launch_owned_conversation(
        &session,
        lease,
        OwnedConversationRequest::Prompt {
            prompt: "prompt".into(),
            attachments: Vec::new(),
            history: None,
            command_capable: false,
            exact_run_id: Some("\ninvalid".into()),
            goal_tick: None,
        },
        run_admission::ExecutionMode::Detached,
        false,
    )
    .unwrap();
    let OwnedObservation {
        ready,
        disposition,
        guardian,
        events,
        ..
    } = observation;
    assert!(ready.await.unwrap().is_err());
    // This is the typed disposition from the SAME handoff, followed by its
    // actual guardian proxy, rather than an idle bit or a missing registry.
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), disposition)
            .await
            .unwrap()
            .unwrap(),
        run_admission::OwnerDisposition::Released
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), guardian)
        .await
        .unwrap()
        .unwrap();
    drop(events);
    assert!(session.run_store.list().await.is_empty());
    assert_eq!(client.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        crate::fact_control::read_workspace_facts(&session.workspace, &thread).unwrap(),
        before
    );
}

#[tokio::test]
async fn detached_observer_drains_actual_events_before_waiting_for_the_original_proxy() {
    let (sender, receiver) = mpsc::channel(1);
    let (finish, finished) = tokio::sync::oneshot::channel::<()>();
    let actual_proxy = tokio::spawn(async move {
        finished.await.unwrap();
    });
    let observer = drain_detached_events(receiver, actual_proxy);
    sender
        .send(AgentEvent::TextDelta {
            text: "actual bounded lane".into(),
        })
        .await
        .unwrap();
    // A second send succeeding on capacity one proves the real receiver is
    // drained even while the supplied original proxy remains pending.
    sender
        .send(AgentEvent::TextDelta {
            text: "actual drained lane".into(),
        })
        .await
        .unwrap();
    assert!(!observer.is_finished());
    drop(sender);
    assert!(!observer.is_finished());
    finish.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), observer)
        .await
        .unwrap()
        .unwrap();
}
