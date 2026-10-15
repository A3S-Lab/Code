#![cfg(test)]

use super::*;
use crate::agent_api::agent_loop_runtime::{
    admit_captured_runtime_projection, capture_owned_preparation, OwnedPreparationCapture,
};
use crate::capability::{CapabilityEffect, CapabilityEffectError};
use crate::commands::{CommandOutput, SlashCommand};
use crate::llm::{LlmClient, LlmResponse, StreamEvent, ToolDefinition};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct NoModel(AtomicUsize);

#[async_trait]
impl LlmClient for NoModel {
    async fn complete(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("command component must not call a model")
    }

    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("command component must not call a model")
    }
}

#[derive(Default)]
struct Observed {
    order: Mutex<Vec<&'static str>>,
    contexts: Mutex<Vec<CommandContext>>,
    tasks: Mutex<Vec<Option<tokio::task::Id>>>,
    closes: AtomicUsize,
    sanitizes: AtomicUsize,
}

struct Sanitize(Arc<Observed>);

impl crate::security::SecurityProvider for Sanitize {
    fn sanitize_output(&self, text: &str) -> String {
        self.0.sanitizes.fetch_add(1, Ordering::SeqCst);
        self.0.order.lock().unwrap().push("sanitize");
        format!("sanitized:{text}")
    }
}

struct ProbeCommand {
    output: &'static str,
    observed: Arc<Observed>,
    mutate_history: Option<Arc<RwLock<Vec<Message>>>>,
}

impl SlashCommand for ProbeCommand {
    fn name(&self) -> &str {
        "owned_t004"
    }

    fn description(&self) -> &str {
        "actual owned command probe"
    }

    fn execute(&self, _: &str, ctx: &CommandContext) -> CommandOutput {
        self.observed.order.lock().unwrap().push("dispatch");
        self.observed.contexts.lock().unwrap().push(ctx.clone());
        self.observed
            .tasks
            .lock()
            .unwrap()
            .push(tokio::task::try_id());
        if let Some(history) = &self.mutate_history {
            history
                .write()
                .unwrap()
                .push(Message::assistant("callback"));
        }
        CommandOutput::text(self.output)
    }
}

struct ActualEffect {
    file: std::path::PathBuf,
    fail: bool,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
    observed: Arc<Observed>,
}

#[async_trait]
impl CapabilityEffect for ActualEffect {
    fn name(&self) -> &str {
        "owned-command-real-effect"
    }

    async fn close(self: Box<Self>) -> std::result::Result<(), CapabilityEffectError> {
        self.observed.closes.fetch_add(1, Ordering::SeqCst);
        self.observed.order.lock().unwrap().push("close-enter");
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        if self.fail {
            return Err(CapabilityEffectError::new("actual command close fault"));
        }
        tokio::fs::remove_file(&self.file)
            .await
            .map_err(|error| CapabilityEffectError::new(error.to_string()))?;
        self.observed.order.lock().unwrap().push("close-end");
        Ok(())
    }
}

async fn fixture(
    label: &str,
    observed: Arc<Observed>,
) -> (
    super::super::Agent,
    Arc<AgentSession>,
    tempfile::TempDir,
    Arc<NoModel>,
) {
    let root = tempfile::tempdir().unwrap();
    let client = Arc::new(NoModel::default());
    let agent = super::super::Agent::from_config(super::super::tests::test_config())
        .await
        .unwrap();
    let mut options = super::super::SessionOptions::new().with_session_id(label);
    options.security_provider = Some(Arc::new(Sanitize(observed)));
    let session = Arc::new(
        agent
            .build_session(root.path().display().to_string(), client.clone(), &options)
            .unwrap(),
    );
    (agent, session, root, client)
}

fn captured(session: &AgentSession) -> OwnedPreparationCapture {
    capture_owned_preparation(session, session.session_cancel.child_token(), true).unwrap()
}

async fn admit_command(
    capture: OwnedPreparationCapture,
) -> (PinnedRuntimeProjection, SessionCapabilityRun) {
    let OwnedPreparationCapture {
        command,
        boundary,
        cancellation,
        workspace,
        ..
    } = capture;
    let (runtime, guard, _) =
        admit_captured_runtime_projection(command.unwrap(), &boundary, cancellation, &workspace)
            .await
            .unwrap();
    (runtime, guard)
}

async fn wait_entered(entered: &Semaphore) {
    tokio::time::timeout(std::time::Duration::from_secs(5), entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}

#[tokio::test]
async fn owned_command_same_worker_waits_for_actual_close_before_output() {
    let observed = Arc::new(Observed::default());
    let (_agent, session, root, client) = fixture("owned-command-close", observed.clone()).await;
    session
        .register_command(Arc::new(ProbeCommand {
            output: "original",
            observed: observed.clone(),
            mutate_history: None,
        }))
        .unwrap();
    let inputs = OwnedCommandInputs::capture_original(&session);
    let (runtime, guard) = admit_command(captured(&session)).await;
    let original_scope = guard.run_scope().id().clone();
    let file = root.path().join("actual-command-resource");
    std::fs::write(&file, b"original resource").unwrap();
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    guard
        .run_scope()
        .register_effect(ActualEffect {
            file: file.clone(),
            fail: false,
            entered: entered.clone(),
            release: release.clone(),
            observed: observed.clone(),
        })
        .unwrap();
    let (tx, mut events) = mpsc::channel(2);
    // The owned aggregate is checked by this actual spawn; no Session borrow
    // or second output task is captured. This is component, not owner release.
    let worker = tokio::spawn(async move {
        let worker_id = tokio::task::id();
        let (same_guard, actual_close, text) =
            prepare_owned(&inputs, "/owned_t004", &runtime, guard)
                .await
                .into_original_observation();
        let actual_close = actual_close.unwrap();
        send_text_output(&tx, text.unwrap()).await;
        (same_guard, actual_close, worker_id)
    });
    wait_entered(&entered).await;
    assert_eq!(*observed.order.lock().unwrap(), ["dispatch", "close-enter"]);
    assert_eq!(observed.sanitizes.load(Ordering::SeqCst), 0);
    assert!(matches!(
        events.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert!(!worker.is_finished());
    assert!(file.exists());
    release.add_permits(1);
    match events.recv().await.unwrap() {
        AgentEvent::TextDelta { text } => assert_eq!(text, "sanitized:original"),
        other => panic!("expected original TextDelta, got {other:?}"),
    }
    match events.recv().await.unwrap() {
        AgentEvent::End { text, usage, .. } => {
            assert_eq!(text, "sanitized:original");
            assert_eq!(usage.prompt_tokens, 0);
            assert_eq!(usage.completion_tokens, 0);
            assert_eq!(usage.total_tokens, 0);
        }
        other => panic!("expected original End, got {other:?}"),
    }
    let (same_guard, actual_close, worker_id) = worker.await.unwrap();
    assert_eq!(same_guard.run_scope().id(), &original_scope);
    assert!(!same_guard.run_scope().is_active());
    assert!(actual_close.is_clean());
    assert_eq!(*observed.tasks.lock().unwrap(), [Some(worker_id)]);
    assert_eq!(observed.closes.load(Ordering::SeqCst), 1);
    assert_eq!(observed.sanitizes.load(Ordering::SeqCst), 1);
    assert_eq!(
        *observed.order.lock().unwrap(),
        ["dispatch", "close-enter", "close-end", "sanitize"]
    );
    assert!(!file.exists());
    assert!(events.recv().await.is_none());
    assert_eq!(client.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn noncommand_preprobe_none_has_no_command_admission_and_fact_closes_its_own_guard() {
    let observed = Arc::new(Observed::default());
    let (_agent, session, _root, client) = fixture("owned-command-none", observed.clone()).await;
    let OwnedPreparationCapture {
        fact,
        command,
        boundary,
        cancellation,
        workspace,
        ..
    } = captured(&session);
    assert!(!CommandRegistry::is_command("ordinary input"));
    assert!(command.is_some(), "captured pin is not an admission");
    // Actual public compatibility entrances return None before consulting a
    // closed Session. A slash call positively checks that admission refuses.
    session.close().await;
    assert!(dispatch_blocking(&session, "ordinary input", None)
        .await
        .unwrap()
        .is_none());
    assert!(dispatch_streaming(&session, "ordinary input")
        .await
        .unwrap()
        .is_none());
    assert!(dispatch_blocking(&session, "/help", None).await.is_err());
    drop(command);
    drop(fact);
    drop(boundary);
    drop(cancellation);
    drop(workspace);
    assert!(observed.contexts.lock().unwrap().is_empty());
    assert_eq!(observed.closes.load(Ordering::SeqCst), 0);
    assert_eq!(observed.sanitizes.load(Ordering::SeqCst), 0);

    let (_agent, session, _root, second_client) =
        fixture("owned-command-fact-fallthrough", observed.clone()).await;
    let OwnedPreparationCapture {
        fact,
        command,
        boundary,
        cancellation,
        workspace,
        ..
    } = captured(&session);
    assert!(!CommandRegistry::is_command("ordinary input"));
    // No command guard was constructed. Admit only the real independent fact
    // pin, then close that actual returned guard; None is not a close witness.
    drop(command);
    let (_, fact_guard, _) =
        admit_captured_runtime_projection(fact, &boundary, cancellation, &workspace)
            .await
            .unwrap();
    assert!(fact_guard.run_scope().is_active());
    assert!(fact_guard.close().await.unwrap().is_clean());
    assert!(!fact_guard.run_scope().is_active());
    assert_eq!(client.0.load(Ordering::SeqCst), 0);
    assert_eq!(second_client.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn owned_command_close_failure_retains_actual_error_and_same_guard_without_output() {
    let observed = Arc::new(Observed::default());
    let (_agent, session, root, client) =
        fixture("owned-command-close-fault", observed.clone()).await;
    session
        .register_command(Arc::new(ProbeCommand {
            output: "must not escape",
            observed: observed.clone(),
            mutate_history: None,
        }))
        .unwrap();
    let inputs = OwnedCommandInputs::capture_original(&session);
    let (runtime, guard) = admit_command(captured(&session)).await;
    let original_scope = guard.run_scope().id().clone();
    let file = root.path().join("actual-refused-command-resource");
    std::fs::write(&file, b"resource remains on close error").unwrap();
    guard
        .run_scope()
        .register_effect(ActualEffect {
            file: file.clone(),
            fail: true,
            entered: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(1)),
            observed: observed.clone(),
        })
        .unwrap();
    let (same_guard, actual_close, text) = prepare_owned(&inputs, "/owned_t004", &runtime, guard)
        .await
        .into_original_observation();
    assert_eq!(same_guard.run_scope().id(), &original_scope);
    assert!(matches!(
        actual_close,
        Err(CapabilityRuntimeError::RunCloseIncomplete {
            child_scopes_failed: 1,
            effects_failed: 0,
            ..
        })
    ));
    assert!(text.is_none());
    assert_eq!(observed.sanitizes.load(Ordering::SeqCst), 0);
    assert_eq!(observed.closes.load(Ordering::SeqCst), 1);
    assert!(
        file.exists(),
        "Err is not zero effects or successful cleanup"
    );
    let actual_run_close = same_guard.run_scope().close().await.unwrap();
    assert_eq!(actual_run_close.effects_failed, 1);
    assert_eq!(
        observed.closes.load(Ordering::SeqCst),
        1,
        "no replay of effect close"
    );
    assert_eq!(client.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn owned_command_frozen_registry_and_original_history_preserve_legacy_results() {
    let observed = Arc::new(Observed::default());
    let (_agent, session, _root, client) =
        fixture("owned-command-captured", observed.clone()).await;
    session
        .history
        .write()
        .unwrap()
        .push(Message::user("original"));
    session
        .register_command(Arc::new(ProbeCommand {
            output: "captured",
            observed: observed.clone(),
            mutate_history: Some(Arc::clone(&session.history)),
        }))
        .unwrap();
    let inputs = OwnedCommandInputs::capture_original(&session);
    let capture = captured(&session);
    let replacement = Arc::new(Observed::default());
    session
        .register_command(Arc::new(ProbeCommand {
            output: "replacement",
            observed: replacement.clone(),
            mutate_history: None,
        }))
        .unwrap();
    let (runtime, guard) = admit_command(capture).await;
    let (same_guard, actual_close, text) = prepare_owned(&inputs, "/owned_t004", &runtime, guard)
        .await
        .into_original_observation();
    assert!(actual_close.unwrap().is_clean());
    assert!(!same_guard.run_scope().is_active());
    assert!(replacement.contexts.lock().unwrap().is_empty());
    let ctx = observed.contexts.lock().unwrap()[0].clone();
    assert_eq!(ctx.session_id, session.session_id);
    assert_eq!(ctx.workspace, session.workspace.display().to_string());
    assert_eq!(ctx.model, session.model_name);
    assert_eq!(ctx.history_len, 1);
    assert_eq!(ctx.total_tokens, 0);
    assert_eq!(ctx.total_cost, 0.0);
    let result = inputs.blocking_result(text.unwrap(), None);
    assert_eq!(result.text, "sanitized:captured");
    assert_eq!(
        result.messages.len(),
        2,
        "read original history after callback/close"
    );
    assert_eq!(result.tool_calls_count, 0);
    assert_eq!(result.usage.total_tokens, 0);
    assert_eq!(result.run_admission, "ordinary");
    assert!(result.forced_stop.is_none());
    let history = vec![Message::user("explicit original history")];
    let result = dispatch_blocking(&session, "/owned_t004", Some(&history))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.text, "sanitized:replacement");
    assert_eq!(
        serde_json::to_value(&result.messages).unwrap(),
        serde_json::to_value(&history).unwrap()
    );
    let (mut events, output_worker) = dispatch_streaming(&session, "/owned_t004")
        .await
        .unwrap()
        .unwrap();
    match events.recv().await.unwrap() {
        AgentEvent::TextDelta { text } => assert_eq!(text, "sanitized:replacement"),
        other => panic!("expected compatibility TextDelta, got {other:?}"),
    }
    match events.recv().await.unwrap() {
        AgentEvent::End { text, .. } => assert_eq!(text, "sanitized:replacement"),
        other => panic!("expected compatibility End, got {other:?}"),
    }
    output_worker.await.unwrap();
    assert!(events.recv().await.is_none());
    assert_eq!(replacement.contexts.lock().unwrap().len(), 2);
    assert_eq!(client.0.load(Ordering::SeqCst), 0);
}
