#![cfg(test)]

use super::*;
use crate::capability::{
    CapabilityContribution, CapabilityDescriptor, CapabilityEffect, CapabilityEffectError,
    CapabilitySet, CapabilitySource, CodeCatalogGeneration, RetainedUseGeneration,
    SessionCapabilityBatch, Sha256Digest, UseCapabilityGeneration, UseGenerationLeaseError,
    UseGenerationLeaseProvider, UsePackageGeneration,
};
use crate::commands::{CommandContext, CommandOutput, SlashCommand};
use crate::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use crate::store::{FileSessionStore, MemorySessionStore, SessionStore};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, Weak};
use tokio::sync::{mpsc, Semaphore};
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
        anyhow::bail!("owned construction must not call a model")
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("owned construction must not call a model")
    }
}

async fn fixture(
    label: &str,
    options: super::super::SessionOptions,
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
    let session = Arc::new(
        agent
            .build_session(
                root.path().display().to_string(),
                client.clone(),
                &options.with_session_id(label),
            )
            .unwrap(),
    );
    (agent, session, root, client)
}

async fn observe_actual_acquire(provider: &Acquire) {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        provider.entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
}

fn capture(session: &AgentSession, command: bool) -> OwnedPreparationCapture {
    capture_owned_preparation(session, session.session_cancel.child_token(), command).unwrap()
}

fn owned_send_static<T: Send + 'static>(value: T) -> T {
    value
}

fn digest(c: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", c.to_string().repeat(64))).unwrap()
}

struct ProbeTool;
#[async_trait]
impl crate::tools::Tool for ProbeTool {
    fn name(&self) -> &str {
        "owned_probe"
    }
    fn description(&self) -> &str {
        "frozen constructor probe"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type":"object"})
    }
    async fn execute(
        &self,
        _: &serde_json::Value,
        _: &crate::tools::ToolContext,
    ) -> anyhow::Result<crate::tools::ToolOutput> {
        anyhow::bail!("constructor test must not execute tools")
    }
}

struct Acquire {
    generation: UseCapabilityGeneration,
    entered: Semaphore,
    release: Semaphore,
    wait: bool,
    fail: bool,
    returned: AtomicUsize,
    dropped: Arc<AtomicUsize>,
}

impl Acquire {
    fn new(wait: bool, fail: bool) -> Arc<Self> {
        Arc::new(Self {
            generation: UseCapabilityGeneration::new(1, digest('a'), digest('f')),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            wait,
            fail,
            returned: AtomicUsize::new(0),
            dropped: Arc::new(AtomicUsize::new(0)),
        })
    }
}

struct ActualLease {
    generation: UseCapabilityGeneration,
    dropped: Arc<AtomicUsize>,
}
impl RetainedUseGeneration for ActualLease {
    fn use_generation(&self) -> &UseCapabilityGeneration {
        &self.generation
    }
}
impl Drop for ActualLease {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl UseGenerationLeaseProvider for Acquire {
    fn use_generation(&self) -> &UseCapabilityGeneration {
        &self.generation
    }
    async fn acquire(
        &self,
        _: CancellationToken,
    ) -> Result<Box<dyn RetainedUseGeneration>, UseGenerationLeaseError> {
        self.entered.add_permits(1);
        if self.wait {
            self.release
                .acquire()
                .await
                .map_err(|e| UseGenerationLeaseError::new(e.to_string()))?
                .forget();
        }
        if self.fail {
            return Err(UseGenerationLeaseError::new("actual acquisition refused"));
        }
        self.returned.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(ActualLease {
            generation: self.generation.clone(),
            dropped: self.dropped.clone(),
        }))
    }
}

async fn install(
    session: &AgentSession,
    generation: u64,
    provider: Option<Arc<Acquire>>,
    command: Option<Arc<dyn SlashCommand>>,
) {
    let source = if let Some(provider) = &provider {
        CapabilitySource::use_package(
            provider.generation.clone(),
            UsePackageGeneration::new(
                "owned/constructor",
                "use/owned/constructor",
                "constructor",
                "1.0.0",
                1,
                digest('d'),
                digest('e'),
            )
            .unwrap(),
        )
        .unwrap()
    } else {
        CapabilitySource::host("owned-constructor", digest('a')).unwrap()
    };
    let tool = CapabilityDescriptor::new(
        &source,
        CapabilityKind::Tool,
        "owned_probe",
        "owned_probe",
        digest('b'),
        [],
    )
    .unwrap();
    let tool_id = tool.id().clone();
    let command_descriptor = command.as_ref().map(|_| {
        CapabilityDescriptor::new(
            &source,
            CapabilityKind::Command,
            "owned_command",
            "owned_command",
            digest('c'),
            [],
        )
        .unwrap()
    });
    let mut descriptors = vec![tool];
    if let Some(value) = &command_descriptor {
        descriptors.push(value.clone());
    }
    let contribution = CapabilityContribution::new(source, descriptors).unwrap();
    let set = if let Some(provider) = &provider {
        CapabilitySet::from_use_projection(
            CodeCatalogGeneration::new(generation),
            provider.generation.clone(),
            [contribution],
        )
        .unwrap()
    } else {
        CapabilitySet::from_contributions(CodeCatalogGeneration::new(generation), [contribution])
            .unwrap()
    };
    let mut batch = if let Some(provider) = provider {
        SessionCapabilityBatch::from_use_projection(set, provider).unwrap()
    } else {
        SessionCapabilityBatch::new(set).unwrap()
    };
    batch
        .stage_value(tool_id, CapabilityValue::Tool(Arc::new(ProbeTool)))
        .unwrap();
    if let (Some(descriptor), Some(command)) = (command_descriptor, command) {
        batch
            .stage_value(descriptor.id().clone(), CapabilityValue::Command(command))
            .unwrap();
    }
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
}

#[derive(Default)]
struct Guard(AtomicUsize);
#[async_trait]
impl crate::budget::BudgetGuard for Guard {
    async fn check_before_llm(&self, _: &str, _: usize) -> crate::budget::BudgetDecision {
        self.0.fetch_add(1, Ordering::SeqCst);
        crate::budget::BudgetDecision::Allow
    }
}

#[tokio::test]
async fn captured_effective_guard_preserves_none_base_runtime_and_arc_identity() {
    let (_agent, session, _root, client) =
        fixture("owned-guard", super::super::SessionOptions::new()).await;
    let none = capture(&session, false);
    assert!(none.construction.parent.budget_guard.is_none());
    assert!(!none.fact.ceiling.governance().budget_guard_required());
    drop(none);
    let observed_guard = Arc::new(Guard::default());
    let first: Arc<dyn crate::budget::BudgetGuard> = observed_guard.clone();
    session.set_budget_guard(Some(first.clone())).unwrap();
    let owned = capture(&session, true);
    let replacement: Arc<dyn crate::budget::BudgetGuard> = Arc::new(Guard::default());
    session.set_budget_guard(Some(replacement)).unwrap();
    assert!(Arc::ptr_eq(
        owned.construction.parent.budget_guard.as_ref().unwrap(),
        &first
    ));
    assert!(Arc::ptr_eq(
        owned
            .construction
            .inputs
            .config
            .budget_guard
            .as_ref()
            .unwrap(),
        &first
    ));
    assert!(owned.fact.ceiling.governance().budget_guard_required());
    assert!(owned
        .command
        .as_ref()
        .unwrap()
        .ceiling
        .governance()
        .budget_guard_required());
    assert_eq!(client.0.load(Ordering::SeqCst), 0);
    assert_eq!(observed_guard.0.load(Ordering::SeqCst), 0);
    let base: Arc<dyn crate::budget::BudgetGuard> = Arc::new(Guard::default());
    let (_agent, session, _root, _) = fixture(
        "owned-base-guard",
        super::super::SessionOptions::new().with_budget_guard(base.clone()),
    )
    .await;
    session.set_budget_guard(None).unwrap();
    let owned = capture(&session, false);
    assert!(Arc::ptr_eq(
        owned.construction.parent.budget_guard.as_ref().unwrap(),
        &base
    ));
    assert!(Arc::ptr_eq(
        owned
            .construction
            .inputs
            .config
            .budget_guard
            .as_ref()
            .unwrap(),
        &base
    ));
    assert!(owned.fact.ceiling.governance().budget_guard_required());
    drop(owned);
    let runtime: Arc<dyn crate::budget::BudgetGuard> = Arc::new(Guard::default());
    session.set_budget_guard(Some(runtime.clone())).unwrap();
    let owned = capture(&session, false);
    assert!(Arc::ptr_eq(
        owned.construction.parent.budget_guard.as_ref().unwrap(),
        &runtime
    ));
    assert!(Arc::ptr_eq(
        owned
            .construction
            .inputs
            .config
            .budget_guard
            .as_ref()
            .unwrap(),
        &runtime
    ));
}

#[tokio::test]
async fn captured_overrides_keep_clearing_and_original_permission_facets() {
    let base = crate::permissions::PermissionPolicy::new().allow("owned_probe");
    let (_agent, session, _root, _) = fixture(
        "owned-overrides",
        super::super::SessionOptions::new()
            .with_permission_policy(base)
            .with_planning_mode(crate::prompts::PlanningMode::Enabled)
            .with_prompt_slots(crate::prompts::SystemPromptSlots {
                style: Some(crate::prompts::AgentStyle::Plan),
                output_language: Some("en-US".into()),
                ..Default::default()
            }),
    )
    .await;
    session.set_agent_style(None).unwrap();
    session.set_output_language(None::<String>).unwrap();
    session
        .set_planning_mode(crate::prompts::PlanningMode::Disabled)
        .unwrap();
    session
        .set_permission_policy(crate::permissions::PermissionPolicy::new().deny("owned_probe"))
        .unwrap();
    let owned = capture(&session, false);
    session
        .set_agent_style(Some(crate::prompts::AgentStyle::Plan))
        .unwrap();
    session.set_output_language(Some("zh-CN")).unwrap();
    session
        .set_permission_policy(crate::permissions::PermissionPolicy::new().allow("owned_probe"))
        .unwrap();
    assert!(owned
        .construction
        .inputs
        .config
        .prompt_slots
        .style
        .is_none());
    assert!(owned
        .construction
        .inputs
        .config
        .prompt_slots
        .output_language
        .is_none());
    assert_eq!(
        owned.construction.inputs.config.planning_mode,
        crate::prompts::PlanningMode::Disabled
    );
    let args = serde_json::json!({});
    assert_eq!(
        owned.permission.check("owned_probe", &args),
        crate::permissions::PermissionDecision::Deny
    );
    assert_eq!(
        owned
            .construction
            .inputs
            .config
            .permission_policy
            .as_ref()
            .unwrap()
            .check("owned_probe", &args),
        crate::permissions::PermissionDecision::Allow
    );
    assert_eq!(
        owned
            .construction
            .parent
            .permission_policy
            .as_ref()
            .unwrap()
            .check("owned_probe", &args),
        crate::permissions::PermissionDecision::Allow
    );
}

struct LockedCommand {
    session: Weak<AgentSession>,
    active: AtomicBool,
    observations: AtomicUsize,
}
impl SlashCommand for LockedCommand {
    fn name(&self) -> &str {
        if self.active.load(Ordering::SeqCst) {
            let session = self.session.upgrade().unwrap();
            assert!(matches!(
                session.command_registry.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            ));
            self.observations.fetch_add(1, Ordering::SeqCst);
        }
        "owned_command"
    }
    fn description(&self) -> &str {
        "observe the real compatibility lock"
    }
    fn execute(&self, _: &str, _: &CommandContext) -> CommandOutput {
        CommandOutput::text("captured")
    }
}

#[tokio::test]
async fn two_real_pins_keep_generation_and_hold_original_command_guard_for_both_snapshots() {
    let (_agent, session, _root, _) =
        fixture("owned-two-pins", super::super::SessionOptions::new()).await;
    let command = Arc::new(LockedCommand {
        session: Arc::downgrade(&session),
        active: AtomicBool::new(false),
        observations: AtomicUsize::new(0),
    });
    install(&session, 1, None, Some(command.clone())).await;
    command.active.store(true, Ordering::SeqCst);
    let owned = capture(&session, true);
    command.active.store(false, Ordering::SeqCst);
    assert_eq!(command.observations.load(Ordering::SeqCst), 2);
    let command_pin = owned.command.as_ref().unwrap();
    assert_eq!(
        command_pin.projection.stamp(),
        owned.fact.projection.stamp()
    );
    assert_eq!(command_pin.binding, owned.fact.binding);
    let before = owned.fact.projection.stamp().clone();
    install(&session, 2, None, None).await;
    assert_ne!(session.capability_catalog.current_stamp(), before);
    assert_eq!(owned.fact.projection.stamp(), &before);
    assert_eq!(command_pin.projection.stamp(), &before);
    // A direct compatibility mutation after capture cannot change either map.
    session.command_registry().unregister("help");
    assert!(owned
        .fact
        .runtime
        .command_registry()
        .list()
        .iter()
        .any(|(n, _)| *n == "help"));
    assert!(command_pin
        .runtime
        .command_registry()
        .list()
        .iter()
        .any(|(n, _)| *n == "help"));
    // Both unique lease values remain independently consumable by original admit.
    let OwnedPreparationCapture {
        fact,
        command,
        construction,
        workspace,
        boundary,
        cancellation,
        ..
    } = owned;
    assert_eq!(workspace, construction.inputs.workspace);
    let (_, first, _) = admit_captured_runtime_projection(
        command.unwrap(),
        &boundary,
        cancellation.clone(),
        &workspace,
    )
    .await
    .unwrap();
    assert!(first.close().await.unwrap().is_clean());
    let (_, second, _) =
        admit_captured_runtime_projection(fact, &boundary, cancellation, &workspace)
            .await
            .unwrap();
    assert!(second.close().await.unwrap().is_clean());
}

#[tokio::test]
async fn original_boundary_refuses_closed_and_poisoned_capture_without_admission() {
    let (_agent, session, _root, _) =
        fixture("owned-closed", super::super::SessionOptions::new()).await;
    let provider = Acquire::new(false, false);
    install(&session, 1, Some(provider.clone()), None).await;
    let boundary = session.close_handle.execution_admission_boundary();
    session.close().await;
    assert!(boundary.closed_under_original_boundary());
    assert!(capture_owned_preparation(&session, CancellationToken::new(), true).is_err());
    assert_eq!(provider.entered.available_permits(), 0);
    let (_agent, session, _root, _) =
        fixture("owned-poison", super::super::SessionOptions::new()).await;
    let boundary = session.close_handle.execution_admission_boundary();
    let original = session.close_handle.clone();
    assert!(std::thread::spawn(move || {
        let _held = original.immediate_extension_mutation.lock().unwrap();
        panic!("poison original boundary");
    })
    .join()
    .is_err());
    assert!(boundary.closed_under_original_boundary());
    assert!(capture_owned_preparation(&session, CancellationToken::new(), false).is_err());
}

#[tokio::test]
async fn attempted_acquisition_error_and_cancellation_never_become_not_attempted() {
    for fail in [true, false] {
        let (_agent, session, _root, client) =
            fixture("owned-attempted", super::super::SessionOptions::new()).await;
        let provider = Acquire::new(true, fail);
        install(&session, 1, Some(provider.clone()), None).await;
        let OwnedPreparationCapture {
            fact,
            construction,
            boundary,
            cancellation,
            ..
        } = capture(&session, false);
        let cancel = cancellation.clone();
        let actual = tokio::spawn(async move {
            admit_captured_runtime_projection(
                fact,
                &boundary,
                cancellation,
                &construction.inputs.workspace,
            )
            .await
        });
        observe_actual_acquire(&provider).await;
        if fail {
            provider.release.add_permits(1);
        } else {
            cancel.cancel();
        }
        let error = match actual.await.unwrap() {
            Err(error) => error,
            Ok(_) => panic!("actual admission must refuse"),
        };
        let (primary, observation) = error.into_original_observation();
        if fail {
            assert!(matches!(
                primary,
                crate::error::CodeError::Capability(
                    CapabilityRuntimeError::UseLeaseAcquisition { .. }
                )
            ));
        } else {
            assert!(matches!(
                primary,
                crate::error::CodeError::Capability(CapabilityRuntimeError::Cancelled)
            ));
        }
        assert!(
            observation.is_none(),
            "attempted admission has no returned guard, never absence proof"
        );
        assert_eq!(provider.returned.load(Ordering::SeqCst), 0);
        assert_eq!(client.0.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn close_winning_actual_admission_retains_original_guard_and_clean_result() {
    let (_agent, session, _root, client) =
        fixture("owned-close-race", super::super::SessionOptions::new()).await;
    let provider = Acquire::new(true, false);
    install(&session, 1, Some(provider.clone()), None).await;
    let OwnedPreparationCapture {
        fact,
        construction,
        boundary,
        cancellation,
        ..
    } = owned_send_static(capture(&session, false));
    let actual = tokio::spawn(async move {
        admit_captured_runtime_projection(
            fact,
            &boundary,
            cancellation,
            &construction.inputs.workspace,
        )
        .await
    });
    observe_actual_acquire(&provider).await;
    // An actual accepted memory ticket holds close at its existing drain
    // boundary. There is no canonical Run/inbox in this construction-only
    // fixture, so a mutable control-slot lock is not an original close barrier.
    // This ticket is not an execution lease and no extraction is performed.
    let memory_boundary = session.memory().unwrap().enqueue_llm_extraction();
    let closing = session.close();
    tokio::pin!(closing);
    tokio::select! {
        biased;
        _ = &mut closing => panic!("actual close must drain its accepted memory ticket"),
        _ = std::future::ready(()) => {}
    }
    assert!(session.is_closed());
    assert!(!session.session_cancel.is_cancelled());
    provider.release.add_permits(1);
    let error = match actual.await.unwrap() {
        Err(error) => error,
        Ok(_) => panic!("closed original session must refuse"),
    };
    let (primary, observation) = error.into_original_observation();
    assert!(matches!(
        primary,
        crate::error::CodeError::Capability(CapabilityRuntimeError::SessionClosed)
    ));
    let (guard, actual_close) = observation.unwrap();
    assert!(actual_close.unwrap().is_clean());
    assert!(!guard.run_scope().is_active());
    assert_eq!(provider.returned.load(Ordering::SeqCst), 1);
    assert_eq!(provider.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(client.0.load(Ordering::SeqCst), 0);
    drop(memory_boundary);
    closing.await;
    assert!(session.session_cancel.is_cancelled());
}

struct RealEffect {
    file: std::path::PathBuf,
    fail: bool,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl CapabilityEffect for RealEffect {
    fn name(&self) -> &str {
        "owned-real-close"
    }
    async fn close(self: Box<Self>) -> Result<(), CapabilityEffectError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(CapabilityEffectError::new("actual effect close failed"));
        }
        tokio::fs::remove_file(&self.file)
            .await
            .map_err(|e| CapabilityEffectError::new(e.to_string()))
    }
}

#[tokio::test]
async fn actual_hook_assembly_failure_preserves_primary_and_real_close_ok_or_err() {
    for fail in [false, true] {
        let (_agent, session, root, client) =
            fixture("owned-hook-close", super::super::SessionOptions::new()).await;
        let provider = Acquire::new(false, false);
        install(&session, 1, Some(provider.clone()), None).await;
        let OwnedPreparationCapture {
            fact,
            construction,
            boundary,
            cancellation,
            ..
        } = capture(&session, false);
        let (runtime, guard, binding) = admit_captured_runtime_projection(
            fact,
            &boundary,
            cancellation,
            &construction.inputs.workspace,
        )
        .await
        .unwrap();
        let file = root.path().join("actual-owned-effect");
        std::fs::write(&file, b"actual acquired resource").unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        guard
            .run_scope()
            .register_effect(RealEffect {
                file: file.clone(),
                fail,
                calls: calls.clone(),
            })
            .unwrap();
        let original_scope = guard.run_scope().id().clone();
        let _already_attached = super::super::run_hook_executor::RunHookExecutor::new(
            None,
            runtime.hook_engine.clone(),
            guard.task_spawner(),
        )
        .ok()
        .unwrap();
        let OwnedLoopConstruction { inputs, parent } = construction;
        let failure =
            match assemble_owned_admitted_agent_loop(runtime, guard, binding, inputs, parent).await
            {
                Err(error) => error,
                Ok(_) => panic!("second dispatcher attach must fail"),
            };
        assert!(
            matches!(failure.primary_error(), crate::error::CodeError::Capability(CapabilityRuntimeError::RuntimeValueInvalid { kind: CapabilityKind::Hook, public_name, .. }) if public_name == "hook-registry")
        );
        let (_, actual) = failure.into_original_observation();
        let (same_guard, actual_close) = actual.unwrap();
        assert_eq!(same_guard.run_scope().id(), &original_scope);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        if fail {
            assert!(matches!(
                actual_close,
                Err(CapabilityRuntimeError::RunCloseIncomplete {
                    child_scopes_failed: 1,
                    effects_failed: 0,
                    ..
                })
            ));
            let original_run_report = same_guard.run_scope().close().await.unwrap();
            assert_eq!(original_run_report.effects_failed, 1);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert!(file.exists());
        } else {
            assert!(actual_close.unwrap().is_clean());
            assert!(!file.exists());
        }
        assert_eq!(client.0.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn owned_loop_future_is_send_static_and_builds_without_a_session_borrow() {
    fn send_static<F: std::future::Future + Send + 'static>(future: F) -> F {
        future
    }
    let (agent, session, _root, client) =
        fixture("owned-static", super::super::SessionOptions::new()).await;
    let OwnedPreparationCapture {
        fact,
        construction,
        boundary,
        cancellation,
        ..
    } = owned_send_static(capture(&session, false));
    let future = send_static(build_owned_pinned_agent_loop(
        fact,
        construction,
        boundary,
        cancellation,
    ));
    drop(session);
    let (agent_loop, guard) = future.await.unwrap();
    drop(agent_loop);
    assert!(guard.close().await.unwrap().is_clean());
    assert_eq!(client.0.load(Ordering::SeqCst), 0);
    drop(agent);
}

#[tokio::test]
async fn actual_command_none_is_pre_admission_and_slash_some_closes_before_output() {
    let (_agent, session, _root, client) =
        fixture("owned-command", super::super::SessionOptions::new()).await;
    let provider = Acquire::new(false, false);
    install(&session, 1, Some(provider.clone()), None).await;
    let owned = capture(&session, true);
    assert!(!CommandRegistry::is_command("ordinary input"));
    let ctx = CommandContext {
        session_id: owned.session_id.clone(),
        workspace: session.workspace.display().to_string(),
        model: "unused".into(),
        history_len: 0,
        total_tokens: 0,
        total_cost: 0.0,
        tool_names: vec![],
        mcp_servers: vec![],
        context_report: None,
    };
    assert!(owned
        .command
        .as_ref()
        .unwrap()
        .runtime
        .command_registry()
        .dispatch("ordinary input", &ctx)
        .is_none());
    assert_eq!(provider.returned.load(Ordering::SeqCst), 0);
    let OwnedPreparationCapture {
        command,
        construction,
        boundary,
        cancellation,
        ..
    } = owned;
    let (runtime, guard, _) = admit_captured_runtime_projection(
        command.unwrap(),
        &boundary,
        cancellation,
        &construction.inputs.workspace,
    )
    .await
    .unwrap();
    let output = runtime
        .command_registry()
        .dispatch("/unknown-owned-command", &ctx);
    assert!(output.is_some());
    assert!(guard.close().await.unwrap().is_clean());
    assert_eq!(provider.dropped.load(Ordering::SeqCst), 1);
    assert_eq!(client.0.load(Ordering::SeqCst), 0);
}

#[derive(Default)]
struct Export(Mutex<Vec<crate::session_checkpoint::SessionCheckpointExportV1>>);
#[async_trait]
impl crate::session_checkpoint::SessionCheckpointExportSink for Export {
    async fn export_checkpoint(
        &self,
        export: crate::session_checkpoint::SessionCheckpointExportV1,
    ) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(export);
        Ok(())
    }
}

struct RefusingExport(AtomicUsize);
#[async_trait]
impl crate::session_checkpoint::SessionCheckpointExportSink for RefusingExport {
    async fn export_checkpoint(
        &self,
        _: crate::session_checkpoint::SessionCheckpointExportV1,
    ) -> anyhow::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("actual captured export refused")
    }
}

fn checkpoint(session: &AgentSession, run_id: &str) -> crate::loop_checkpoint::LoopCheckpoint {
    crate::loop_checkpoint::LoopCheckpoint {
        schema_version: crate::loop_checkpoint::LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.into(),
        session_id: session.session_id.clone(),
        capability_binding: None,
        standing_context_digest: None,
        durable_source: None,
        turn: 1,
        messages: vec![Message::user("original constructor boundary")],
        total_usage: crate::llm::TokenUsage::default(),
        tool_calls_count: 0,
        verification_reports: vec![],
        convergence: Default::default(),
        checkpoint_ms: session.config.host_env.now_ms(),
    }
}

async fn constructor_store_scenario(
    store: Arc<dyn SessionStore>,
) -> crate::loop_checkpoint::LoopCheckpoint {
    let first = Arc::new(Export::default());
    let replacement = Arc::new(Export::default());
    let (_agent, session, _root, client) = fixture(
        "owned-store",
        super::super::SessionOptions::new()
            .with_session_store(store)
            .with_session_checkpoint_export_sink(first.clone())
            .with_auto_save(true),
    )
    .await;
    let owned = capture(&session, false);
    assert!(owned.cognitive_binding.is_none());
    let run_id = "owned-checkpoint-run";
    assert!(matches!(
        session
            .run_store
            .reserve_run_with_id(run_id.into(), &session.session_id, "constructor boundary")
            .await,
        crate::run::RunReservation::Created(_)
    ));
    session
        .set_session_checkpoint_export_sink(Some(replacement.clone()))
        .unwrap();
    let expected = checkpoint(&session, run_id);
    let OwnedCheckpointInputs {
        store,
        export,
        persistence,
    } = owned.checkpoint;
    let (sink, receiver) = super::super::runtime_checkpoints::runtime_checkpoint_channel_owned(
        store,
        export,
        persistence,
    );
    let mut receiver = owned_send_static(receiver);
    let sink = sink.unwrap();
    let actual = expected.clone();
    let sender = tokio::spawn(async move {
        sink.save_checkpoint(&actual).await;
    });
    let boundary = receiver.recv().await.unwrap();
    assert!(!sender.is_finished());
    receiver.commit(boundary).await;
    sender.await.unwrap();
    assert!(receiver.recv().await.is_none());
    assert_eq!(replacement.0.lock().unwrap().len(), 0);
    let exports = first.0.lock().unwrap();
    assert_eq!(exports.len(), 1);
    let restored = exports[0].clone().open().unwrap().logical_resume.unwrap();
    assert_eq!(
        serde_json::to_value(&restored).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert_eq!(client.0.load(Ordering::SeqCst), 0);
    expected
}

#[tokio::test]
async fn owned_checkpoint_constructor_uses_shared_memory_and_reopened_file_store_oracles() {
    let memory: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let expected = constructor_store_scenario(memory.clone()).await;
    let actual = memory
        .load_loop_checkpoint(&expected.run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert!(memory.load("owned-store").await.unwrap().is_some());
    let directory = tempfile::tempdir().unwrap();
    let file: Arc<dyn SessionStore> =
        Arc::new(FileSessionStore::new(directory.path()).await.unwrap());
    let expected = constructor_store_scenario(file).await;
    // The scenario consumes/drops its actual FileSessionStore and receiver;
    // readback comes from a newly opened adapter, not retained in-memory state.
    let reopened = FileSessionStore::new(directory.path()).await.unwrap();
    let actual = reopened
        .load_loop_checkpoint(&expected.run_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert!(reopened.load("owned-store").await.unwrap().is_some());
}

#[tokio::test]
async fn disabled_owned_checkpoint_constructor_creates_no_sender() {
    let (_agent, session, _root, _) =
        fixture("owned-no-committer", super::super::SessionOptions::new()).await;
    let OwnedCheckpointInputs {
        store,
        export,
        persistence,
    } = capture(&session, false).checkpoint;
    assert!(store.is_none());
    assert!(export.is_none());
    let (sink, mut receiver) = super::super::runtime_checkpoints::runtime_checkpoint_channel_owned(
        store,
        export,
        persistence,
    );
    assert!(sink.is_none());
    assert!(receiver.recv().await.is_none());
}

struct RefusingStore(AtomicUsize);
#[async_trait]
impl SessionStore for RefusingStore {
    async fn save(&self, _: &crate::store::SessionData) -> anyhow::Result<()> {
        anyhow::bail!("write refused")
    }
    async fn load(&self, _: &str) -> anyhow::Result<Option<crate::store::SessionData>> {
        Ok(None)
    }
    async fn delete(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    async fn list(&self) -> anyhow::Result<Vec<String>> {
        Ok(vec![])
    }
    async fn exists(&self, _: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
    async fn save_loop_checkpoint(
        &self,
        _: &str,
        _: &crate::loop_checkpoint::LoopCheckpoint,
    ) -> anyhow::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        anyhow::bail!("actual checkpoint save refused")
    }
}

#[tokio::test]
async fn original_writer_ack_after_actual_store_error_does_not_claim_durability() {
    let refusing = Arc::new(RefusingStore(AtomicUsize::new(0)));
    let (_agent, session, _root, _) = fixture(
        "owned-write-failure",
        super::super::SessionOptions::new().with_session_store(refusing.clone()),
    )
    .await;
    let value = checkpoint(&session, "owned-write-failure-run");
    let OwnedCheckpointInputs {
        store,
        export,
        persistence,
    } = capture(&session, false).checkpoint;
    let (sink, mut receiver) = super::super::runtime_checkpoints::runtime_checkpoint_channel_owned(
        store,
        export,
        persistence,
    );
    let actual = tokio::spawn(async move {
        sink.unwrap().save_checkpoint(&value).await;
    });
    let boundary = receiver.recv().await.unwrap();
    receiver.commit(boundary).await;
    actual.await.unwrap();
    assert_eq!(refusing.0.load(Ordering::SeqCst), 1);
    assert!(refusing
        .load("owned-write-failure")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn original_writer_ack_keeps_actual_export_error_distinct_from_saved_checkpoint() {
    let store = Arc::new(MemorySessionStore::new());
    let refusing = Arc::new(RefusingExport(AtomicUsize::new(0)));
    let (_agent, session, _root, client) = fixture(
        "owned-export-failure",
        super::super::SessionOptions::new()
            .with_session_store(store.clone())
            .with_session_checkpoint_export_sink(refusing.clone()),
    )
    .await;
    let run_id = "owned-export-failure-run";
    assert!(matches!(
        session
            .run_store
            .reserve_run_with_id(run_id.into(), &session.session_id, "actual export failure")
            .await,
        crate::run::RunReservation::Created(_)
    ));
    let value = checkpoint(&session, run_id);
    let OwnedCheckpointInputs {
        store: captured_store,
        export,
        persistence,
    } = capture(&session, false).checkpoint;
    let (sink, mut receiver) = super::super::runtime_checkpoints::runtime_checkpoint_channel_owned(
        captured_store,
        export,
        persistence,
    );
    let expected = value.clone();
    let actual = tokio::spawn(async move {
        sink.unwrap().save_checkpoint(&value).await;
    });
    let boundary = receiver.recv().await.unwrap();
    receiver.commit(boundary).await;
    actual.await.unwrap();
    assert_eq!(refusing.0.load(Ordering::SeqCst), 1);
    let saved = store.load_loop_checkpoint(run_id).await.unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(saved).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(client.0.load(Ordering::SeqCst), 0);
}
