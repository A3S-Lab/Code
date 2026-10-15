#![cfg(test)]

use super::*;
use crate::budget::{BudgetDecision, BudgetGuard};
use crate::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use crate::loop_checkpoint::LoopCheckpoint;
use crate::run_control::{RunControlReceiptState, SteerRequest};
use crate::session_checkpoint::{SessionCheckpointExportSink, SessionCheckpointExportV1};
use crate::store::{
    FileSessionStore, MemorySessionStore, SessionData, SessionSnapshotV1, SessionStore,
};
use crate::AgentEvent;
use tokio::sync::{mpsc, Semaphore};

const SESSION: &str = "runtime-checkpoint-failure";
const STEER: &str = " \t保留这条终止前的原始指令。\n";

#[derive(Clone, Copy, Debug)]
enum ExportReturn {
    Panic,
    Error,
    Ok,
}

#[derive(Default)]
struct NoCommands(AtomicUsize);
#[async_trait::async_trait]
impl crate::sandbox::BashSandbox for NoCommands {
    async fn exec_command(
        &self,
        _: &str,
        _: &str,
    ) -> anyhow::Result<crate::sandbox::SandboxOutput> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("checkpoint qualification refuses command execution")
    }
    async fn shutdown(&self) {}
}

#[derive(Default)]
struct TwoResponses {
    streaming: AtomicUsize,
    complete: AtomicUsize,
}
#[async_trait::async_trait]
impl LlmClient for TwoResponses {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.complete.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("actual Done must not fall back")
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let call = self.streaming.fetch_add(1, Ordering::SeqCst);
        assert!(call < 2, "no third provider response in this scenario");
        let message = if call == 0 {
            Message {
                role: "assistant".into(),
                content: vec![ContentBlock::ToolUse {
                    id: "denied-read".into(),
                    name: "read".into(),
                    input: serde_json::json!({"file_path":"never-read.txt"}),
                }],
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: crate::llm::TranscriptVisibility::Wire,
            }
        } else {
            Message::assistant("The informational reply is complete.")
        };
        let (tx, rx) = mpsc::channel(1);
        tx.try_send(StreamEvent::Done(LlmResponse {
            message,
            usage: TokenUsage {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: None,
            token_logprobs: vec![],
            meta: None,
        }))
        .unwrap();
        Ok(rx)
    }
}

struct LastAccounting {
    entered: Semaphore,
    release: Semaphore,
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl BudgetGuard for LastAccounting {
    async fn check_before_llm(&self, _: &str, _: usize) -> BudgetDecision {
        BudgetDecision::Allow
    }
    async fn record_after_llm(&self, _: &str, _: &TokenUsage) {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
        }
    }
}

struct ExportBarrier {
    returned: ExportReturn,
    entered: Semaphore,
    release: Semaphore,
    actual: std::sync::Mutex<Vec<SessionCheckpointExportV1>>,
    task: std::sync::Mutex<Option<tokio::task::Id>>,
}
#[async_trait::async_trait]
impl SessionCheckpointExportSink for ExportBarrier {
    async fn export_checkpoint(&self, checkpoint: SessionCheckpointExportV1) -> anyhow::Result<()> {
        assert!(self
            .task
            .lock()
            .unwrap()
            .replace(tokio::task::id())
            .is_none());
        self.actual.lock().unwrap().push(checkpoint);
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        match self.returned {
            ExportReturn::Panic => panic!("actual runtime checkpoint export panic"),
            ExportReturn::Error => anyhow::bail!("actual export returned failure"),
            ExportReturn::Ok => Ok(()),
        }
    }
}

// This adapter returns the actual backend result. It neither injects a save
// result nor replaces the runtime receiver/committer/checkpoint writer.
struct RecordedStore {
    actual: Arc<dyn SessionStore>,
    checkpoints: std::sync::Mutex<Vec<(LoopCheckpoint, std::result::Result<(), String>)>>,
}
#[async_trait::async_trait]
impl SessionStore for RecordedStore {
    async fn save(&self, value: &SessionData) -> anyhow::Result<()> {
        self.actual.save(value).await
    }
    async fn load(&self, id: &str) -> anyhow::Result<Option<SessionData>> {
        self.actual.load(id).await
    }
    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.actual.delete(id).await
    }
    async fn list(&self) -> anyhow::Result<Vec<String>> {
        self.actual.list().await
    }
    async fn exists(&self, id: &str) -> anyhow::Result<bool> {
        self.actual.exists(id).await
    }
    async fn save_snapshot(&self, value: &SessionSnapshotV1) -> anyhow::Result<()> {
        self.actual.save_snapshot(value).await
    }
    async fn load_snapshot(&self, id: &str) -> anyhow::Result<Option<SessionSnapshotV1>> {
        self.actual.load_snapshot(id).await
    }
    async fn save_loop_checkpoint(&self, id: &str, value: &LoopCheckpoint) -> anyhow::Result<()> {
        let result = self.actual.save_loop_checkpoint(id, value).await;
        eprintln!("runtime-checkpoint save_result={result:?}");
        self.checkpoints.lock().unwrap().push((
            value.clone(),
            result
                .as_ref()
                .map(|_| ())
                .map_err(|error| format!("{error:#}")),
        ));
        result
    }
    async fn load_loop_checkpoint(&self, id: &str) -> anyhow::Result<Option<LoopCheckpoint>> {
        self.actual.load_loop_checkpoint(id).await
    }
    async fn delete_loop_checkpoint(&self, id: &str) -> anyhow::Result<()> {
        self.actual.delete_loop_checkpoint(id).await
    }
    fn backend_name(&self) -> &str {
        self.actual.backend_name()
    }
}

async fn scenario(file: bool, streaming: bool, returned: ExportReturn, io_fault: bool) {
    let workspace = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let inner: Arc<dyn SessionStore> = if file {
        Arc::new(FileSessionStore::new(root.path()).await.unwrap())
    } else {
        Arc::new(MemorySessionStore::new())
    };
    let blocker = root.path().join("v1/loop_checkpoints");
    if io_fault {
        assert!(file);
        std::fs::write(&blocker, b"actual checkpoint directory obstruction").unwrap();
    }
    let store = Arc::new(RecordedStore {
        actual: inner,
        checkpoints: Default::default(),
    });
    let export = Arc::new(ExportBarrier {
        returned,
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
        actual: Default::default(),
        task: Default::default(),
    });
    let accounting = Arc::new(LastAccounting {
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
        calls: AtomicUsize::new(0),
    });
    let client = Arc::new(TwoResponses::default());
    let commands = Arc::new(NoCommands::default());
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
    let session = Arc::new(
        agent
            .build_session(
                workspace.path().display().to_string(),
                client.clone(),
                &SessionOptions::new()
                    .with_session_id(SESSION)
                    .with_sandbox_handle(commands.clone())
                    .with_permission_policy(crate::permissions::PermissionPolicy::new().deny("*"))
                    .with_planning_mode(crate::PlanningMode::Disabled)
                    .with_auto_delegation_enabled(false)
                    .with_manual_delegation_enabled(false)
                    .with_memory(Arc::new(a3s_memory::InMemoryStore::new()))
                    .with_session_store(store.clone())
                    .with_auto_save(true)
                    .with_session_checkpoint_export_sink(export.clone())
                    .with_budget_guard(accounting.clone()),
            )
            .unwrap(),
    );
    let prompt = "Give an informational answer after the refused read.";
    let mut stream = if streaming {
        Some(session.stream_with_run(prompt, None).await.unwrap())
    } else {
        None
    };
    let caller = if streaming {
        None
    } else {
        let session = session.clone();
        Some(tokio::spawn(
            async move { session.send(prompt, None).await },
        ))
    };
    export.entered.acquire().await.unwrap().forget();
    let run = session.current_run().await.unwrap();
    let original = session.run_admission.current_original().unwrap();
    let inbox = session
        .active_run_control
        .lock()
        .await
        .as_ref()
        .unwrap()
        .clone();
    assert!(session.tool_executor.registry().get("read").is_some());
    let actual_export = export.actual.lock().unwrap()[0].open().unwrap();
    let checkpoint = actual_export.logical_resume.as_ref().unwrap();
    checkpoint.ensure_owned_by(run.id(), SESSION).unwrap();
    assert!(checkpoint.capability_binding.is_some());
    assert_eq!(checkpoint.tool_calls_count, 1);
    {
        let saves = store.checkpoints.lock().unwrap();
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].0.run_id, run.id());
        assert_eq!(saves[0].1.is_err(), io_fault);
        if io_fault {
            // The backend formats its path-inspection IO error, so it no
            // longer carries a typed IO cause. This independent filesystem
            // probe proves the obstruction, not the backend error's type.
            let probe =
                std::fs::symlink_metadata(blocker.join("independent-path-probe")).unwrap_err();
            assert_eq!(probe.kind(), std::io::ErrorKind::NotADirectory);
            assert!(blocker.is_file());
        }
    }
    export.release.add_permits(1);
    accounting.entered.acquire().await.unwrap().forget();
    let mut request = SteerRequest::new(STEER).with_run_id(run.id());
    request.request_id = Some("original-late-checkpoint-steer".into());
    let accepted = session.steer(request.clone()).await.unwrap();
    assert_eq!(accepted.state, RunControlReceiptState::Accepted);
    assert_eq!(session.steer(request.clone()).await.unwrap(), accepted);
    accounting.release.add_permits(1);
    let mut wire = Vec::new();
    let mut guardian = None;
    let caller_error = if let Some(mut actual) = stream.take() {
        while let Some(event) = actual.events.recv().await {
            wire.push(event);
        }
        guardian = Some(actual.worker);
        None
    } else {
        caller
            .unwrap()
            .await
            .unwrap()
            .err()
            .map(|error| error.to_string())
    };
    let panicked = matches!(returned, ExportReturn::Panic);
    // The actual original worker must complete independently of its committer.
    loop {
        if original.observed_worker_join().is_some() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        original.observed_worker_join(),
        Some(ActualJoinObservation::Completed)
    );
    {
        let state = original.committer.state.lock().await;
        if panicked {
            let OriginalCommitterState::Joined(Err(error)) = &*state else {
                panic!("actual failed committer result lost: {state:?}");
            };
            assert!(error.is_panic());
            assert!(!error.is_cancelled());
            assert_eq!(error.id(), export.task.lock().unwrap().unwrap());
        } else {
            assert!(matches!(&*state, OriginalCommitterState::Joined(Ok(()))));
        }
    }
    let receipt = loop {
        let actual = if panicked {
            session.steer(request.clone()).await.unwrap()
        } else {
            inbox
                .submit(request.clone().into_protocol(SESSION, run.id()), 1)
                .await
                .unwrap()
        };
        if actual.state != RunControlReceiptState::Accepted {
            break actual;
        }
        tokio::task::yield_now().await;
    };
    let repeated = if panicked {
        session.steer(request.clone()).await.unwrap()
    } else {
        inbox
            .submit(request.clone().into_protocol(SESSION, run.id()), 1)
            .await
            .unwrap()
    };
    assert_eq!(repeated, receipt);
    let events = session.run_store.events(run.id()).await;
    let facts = crate::fact_control::read_workspace_facts(
        workspace.path(),
        &crate::fact_control::thread_for_session(SESSION),
    )
    .unwrap();
    let tool: Vec<_> = facts.iter().filter(|f| f.kind == "tool.result").collect();
    assert_eq!(tool.len(), 1);
    let denied: a3s_effect::ToolExecutionResultV1 =
        serde_json::from_value(tool[0].payload["receipt"].clone()).unwrap();
    denied.validate().unwrap();
    assert_eq!(denied.outcome, a3s_effect::ToolExecutionOutcome::Denied);
    assert_eq!(
        denied.effects.completeness,
        a3s_effect::ToolObservationCompleteness::Complete
    );
    assert!(denied.effects.changes.is_empty());
    assert_eq!(denied.identity.admission.as_ref().unwrap().run_id, run.id());
    assert_eq!(client.streaming.load(Ordering::SeqCst), 2);
    assert_eq!(client.complete.load(Ordering::SeqCst), 0);
    assert_eq!(accounting.calls.load(Ordering::SeqCst), 2);
    assert_eq!(commands.0.load(Ordering::SeqCst), 0);
    assert!(!events.iter().any(|e| matches!(
        e.event,
        AgentEvent::ToolExecutionStart { .. } | AgentEvent::ToolStart { .. }
    )));
    let inputs: Vec<_> = facts
        .iter()
        .filter(|f| f.kind == "user.message" && f.payload["text"] == STEER)
        .collect();
    if panicked {
        if let Some(guardian) = &guardian {
            assert!(
                !guardian.is_finished(),
                "unproved original guardian must retain ownership"
            );
        }
        assert_eq!(receipt.state, RunControlReceiptState::Settled);
        assert_eq!(
            receipt.error.as_ref().unwrap().code,
            "STEER_PERSISTENCE_FAILED"
        );
        assert!(inputs.is_empty());
        assert!(!facts
            .iter()
            .any(|f| f.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.event, AgentEvent::Error { .. }))
                .count(),
            1
        );
        assert!(events.iter().any(|e| matches!(&e.event, AgentEvent::Error { message } if message.contains("checkpoint committer") && message.contains("Panicked"))));
        assert!(!events
            .iter()
            .any(|e| matches!(e.event, AgentEvent::End { .. })));
        if streaming {
            assert_eq!(
                wire.iter()
                    .filter(|e| matches!(e, AgentEvent::Error { .. }))
                    .count(),
                1
            );
            assert!(!wire.iter().any(|e| matches!(e, AgentEvent::End { .. })));
        }
        assert!(session
            .run_admission
            .current_original()
            .is_some_and(|x| Arc::ptr_eq(&x, &original)));
        assert_eq!(
            session.current_run_id.lock().await.as_deref(),
            Some(run.id())
        );
        {
            let state = original.state.lock().unwrap();
            let OriginalScopeObservation::Returned {
                close: Some(Ok(report)),
                ..
            } = &state.release.scopes[OriginalScopeKind::Fact.index()]
            else {
                panic!("actual Fact close proof missing");
            };
            assert!(report.is_clean());
            assert!(state
                .release
                .scopes
                .iter()
                .all(OriginalScopeObservation::clean));
        }
        assert_eq!(
            session.run_store.snapshot(run.id()).await.unwrap().status,
            crate::RunStatus::Failed
        );
        assert!(!original.state.lock().unwrap().release.slots_retired);
        assert!(!original
            .state
            .lock()
            .unwrap()
            .release
            .controls
            .unwrap()
            .has_accepted());
        assert_eq!(session.task_scheduler.health().await.unwrap().active, 1);
        assert!(
            !session
                .cancel_and_settle(Duration::from_millis(20), Duration::from_millis(20))
                .await
        );
        assert!(
            ExecutionCoordinator::admit(&session, "must not reuse panicked committer")
                .await
                .is_err()
        );
    } else {
        if let Some(guardian) = guardian.take() {
            guardian.await.unwrap();
        }
        assert_eq!(receipt.state, RunControlReceiptState::Applied);
        assert_eq!(inputs.len(), 1);
        let marker = facts
            .iter()
            .find(|f| f.kind == crate::fact_control::TURN_COMPLETE_FACT_KIND)
            .unwrap();
        assert!(inputs[0].seq < marker.seq);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.event, AgentEvent::End { .. }))
                .count(),
            1
        );
        assert!(!events
            .iter()
            .any(|e| matches!(e.event, AgentEvent::Error { .. })));
        assert!(caller_error.is_none());
        assert!(
            session
                .cancel_and_settle(Duration::from_secs(2), Duration::from_secs(1))
                .await
        );
        assert!(session.run_admission.current_original().is_none());
        assert_eq!(session.task_scheduler.health().await.unwrap().active, 0);
        assert_eq!(session.task_scheduler.health().await.unwrap().released, 1);
        assert!(original.state.lock().unwrap().release.retired);
        assert!(original.state.lock().unwrap().release.slots_retired);
        assert!(matches!(
            session.steer(request).await,
            Err(crate::CodeError::RunControl(
                crate::RunControlError::NoActiveRun
            ))
        ));
    }
    let snapshot = serde_json::to_string(&serde_json::json!({"file":file,"streaming":streaming,"export":format!("{returned:?}"),"io_fault":io_fault,"original_run_id":run.id(),"accepted":accepted,"terminal":receipt,"caller_error":caller_error,"save_results":store.checkpoints.lock().unwrap().iter().map(|(c,r)| serde_json::json!({"run_id":c.run_id,"result":r})).collect::<Vec<_>>(),"facts":facts})).unwrap();
    assert!(snapshot.len() < 64 * 1024);
    eprintln!("runtime-checkpoint facts_snapshot={snapshot}");
    if panicked && !streaming {
        assert!(
            caller_error
                .as_ref()
                .is_some_and(|error| error.contains("checkpoint committer")),
            "blocking caller must retain original committer diagnostic: {caller_error:?}"
        );
    }
}

async fn bounded(file: bool, streaming: bool, returned: ExportReturn, io_fault: bool) {
    tokio::time::timeout(
        Duration::from_secs(20),
        scenario(file, streaming, returned, io_fault),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn actual_committer_panic_memory_blocking() {
    bounded(false, false, ExportReturn::Panic, false).await;
}
#[tokio::test]
async fn actual_committer_panic_memory_streaming() {
    bounded(false, true, ExportReturn::Panic, false).await;
}
#[tokio::test]
async fn actual_committer_panic_file_blocking() {
    bounded(true, false, ExportReturn::Panic, false).await;
}
#[tokio::test]
async fn actual_committer_panic_file_streaming() {
    bounded(true, true, ExportReturn::Panic, false).await;
}
#[tokio::test]
async fn actual_checkpoint_io_error_blocking() {
    bounded(true, false, ExportReturn::Ok, true).await;
}
#[tokio::test]
async fn actual_checkpoint_io_error_streaming() {
    bounded(true, true, ExportReturn::Ok, true).await;
}
#[tokio::test]
async fn actual_export_error_memory_blocking() {
    bounded(false, false, ExportReturn::Error, false).await;
}
#[tokio::test]
async fn actual_export_error_memory_streaming() {
    bounded(false, true, ExportReturn::Error, false).await;
}
#[tokio::test]
async fn actual_export_error_file_blocking() {
    bounded(true, false, ExportReturn::Error, false).await;
}
#[tokio::test]
async fn actual_export_error_file_streaming() {
    bounded(true, true, ExportReturn::Error, false).await;
}
