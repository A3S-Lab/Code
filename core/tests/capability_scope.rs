use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use a3s_code_core::capability::{
    CapabilityCeiling, CapabilityContribution, CapabilityDescriptor, CapabilityEffect,
    CapabilityEffectError, CapabilityExecutionCeiling, CapabilityId, CapabilityKind,
    CapabilityScope, CapabilityScopeError, CapabilitySet, CapabilitySource, CodeCatalogGeneration,
    GovernanceCapabilityCeiling, RetainedUseGeneration, Run, ScopeClosePolicy, ScopeKind, Session,
    Sha256Digest, UseCapabilityGeneration, UsePackageGeneration, WorkspaceCapabilityCeiling,
};
use async_trait::async_trait;

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
}

struct HostCatalog {
    set: Arc<CapabilitySet>,
    read: CapabilityId,
    write: CapabilityId,
}

fn host_catalog() -> HostCatalog {
    let source = CapabilitySource::host("a3s-code", digest('a')).unwrap();
    let read = CapabilityDescriptor::new(
        &source,
        CapabilityKind::Tool,
        "read",
        "read",
        digest('b'),
        [],
    )
    .unwrap();
    let write = CapabilityDescriptor::new(
        &source,
        CapabilityKind::Tool,
        "write",
        "write",
        digest('c'),
        [],
    )
    .unwrap();
    let read_id = read.id().clone();
    let write_id = write.id().clone();
    let set = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(1),
        [CapabilityContribution::new(source, [read, write]).unwrap()],
    )
    .unwrap();
    HostCatalog {
        set,
        read: read_id,
        write: write_id,
    }
}

fn execution(max_tool_rounds: usize, max_parallel_tasks: usize) -> CapabilityExecutionCeiling {
    CapabilityExecutionCeiling::new(
        max_tool_rounds,
        max_parallel_tasks,
        Some(1_000),
        Some(2_000),
        Some(5_000),
    )
    .unwrap()
}

fn ceiling(
    set: &CapabilitySet,
    capabilities: impl IntoIterator<Item = CapabilityId>,
    workspace: WorkspaceCapabilityCeiling,
    governance: GovernanceCapabilityCeiling,
    execution: CapabilityExecutionCeiling,
) -> CapabilityCeiling {
    CapabilityCeiling::new(set, capabilities, workspace, governance, execution).unwrap()
}

#[tokio::test]
async fn child_scopes_reject_every_authority_expansion_dimension() {
    let catalog = host_catalog();
    let parent = ceiling(
        &catalog.set,
        [catalog.read.clone()],
        WorkspaceCapabilityCeiling::all().with_write(false),
        GovernanceCapabilityCeiling::none_required()
            .require_permission_guard()
            .require_security_guard(),
        execution(10, 2),
    );
    let session =
        CapabilityScope::<Session>::new_session("session-1", Arc::clone(&catalog.set), parent)
            .unwrap();

    let capability_expansion = ceiling(
        &catalog.set,
        [catalog.read.clone(), catalog.write.clone()],
        WorkspaceCapabilityCeiling::all().with_write(false),
        GovernanceCapabilityCeiling::none_required()
            .require_permission_guard()
            .require_security_guard(),
        execution(10, 2),
    );
    assert!(matches!(
        session.admit_run("run-capability", capability_expansion),
        Err(CapabilityScopeError::CeilingExpansion {
            dimension: "capabilities"
        })
    ));

    let workspace_expansion = ceiling(
        &catalog.set,
        [catalog.read.clone()],
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required()
            .require_permission_guard()
            .require_security_guard(),
        execution(10, 2),
    );
    assert!(matches!(
        session.admit_run("run-workspace", workspace_expansion),
        Err(CapabilityScopeError::CeilingExpansion {
            dimension: "workspace.write"
        })
    ));

    let governance_expansion = ceiling(
        &catalog.set,
        [catalog.read.clone()],
        WorkspaceCapabilityCeiling::all().with_write(false),
        GovernanceCapabilityCeiling::none_required(),
        execution(10, 2),
    );
    assert!(matches!(
        session.admit_run("run-governance", governance_expansion),
        Err(CapabilityScopeError::CeilingExpansion {
            dimension: "governance.permission_guard"
        })
    ));

    let execution_expansion = ceiling(
        &catalog.set,
        [catalog.read.clone()],
        WorkspaceCapabilityCeiling::all().with_write(false),
        GovernanceCapabilityCeiling::none_required()
            .require_permission_guard()
            .require_security_guard(),
        execution(11, 2),
    );
    assert!(matches!(
        session.admit_run("run-execution", execution_expansion),
        Err(CapabilityScopeError::CeilingExpansion {
            dimension: "execution.max_tool_rounds"
        })
    ));

    let narrower = ceiling(
        &catalog.set,
        [],
        WorkspaceCapabilityCeiling::none(),
        GovernanceCapabilityCeiling::none_required()
            .require_permission_guard()
            .require_security_guard()
            .require_budget_guard(),
        execution(5, 1),
    );
    let run = session.admit_run("run-narrow", narrower).unwrap();
    assert!(run.ceiling().is_empty());
    run.close().await.unwrap();
    session.close().await.unwrap();
}

#[tokio::test]
async fn borrowed_leases_filter_visibility_and_fail_after_cancellation() {
    let catalog = host_catalog();
    let session_ceiling = CapabilityCeiling::all(
        &catalog.set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        execution(20, 4),
    )
    .unwrap();
    let session = CapabilityScope::<Session>::new_session(
        "session-1",
        Arc::clone(&catalog.set),
        session_ceiling,
    )
    .unwrap();
    let run_ceiling = ceiling(
        &catalog.set,
        [catalog.read.clone()],
        WorkspaceCapabilityCeiling::all().with_write(false),
        GovernanceCapabilityCeiling::none_required().require_permission_guard(),
        execution(10, 2),
    );
    let run = session.admit_run("run-1", run_ceiling.clone()).unwrap();
    assert!(matches!(
        session.admit_run("run-1", run_ceiling),
        Err(CapabilityScopeError::DuplicateChildScope { .. })
    ));
    let lease = run.lease().unwrap();

    assert_eq!(lease.scope_id().as_str(), "session/session-1/run/run-1");
    assert!(lease.contains(&catalog.read).unwrap());
    assert!(!lease.contains(&catalog.write).unwrap());
    assert_eq!(lease.iter().unwrap().count(), 1);

    run.cancel();
    assert!(matches!(
        lease.contains(&catalog.read),
        Err(CapabilityScopeError::ScopeInactive { .. })
    ));
    run.close().await.unwrap();
    session.close().await.unwrap();
}

struct FakeUseLease {
    generation: UseCapabilityGeneration,
    log: Arc<Mutex<Vec<String>>>,
    label: &'static str,
}

impl RetainedUseGeneration for FakeUseLease {
    fn use_generation(&self) -> &UseCapabilityGeneration {
        &self.generation
    }
}

impl Drop for FakeUseLease {
    fn drop(&mut self) {
        self.log.lock().unwrap().push(self.label.to_owned());
    }
}

fn use_catalog() -> (
    Arc<CapabilitySet>,
    UseCapabilityGeneration,
    CapabilityCeiling,
) {
    let generation = UseCapabilityGeneration::new(7, digest('d'), digest('e'));
    let source = CapabilitySource::use_package(
        generation.clone(),
        UsePackageGeneration::new(
            "acme/guide",
            "use/acme-guide",
            "guide",
            "1.0.0",
            11,
            digest('f'),
            digest('1'),
        )
        .unwrap(),
    )
    .unwrap();
    let guide = CapabilityDescriptor::new(
        &source,
        CapabilityKind::Skill,
        "guide",
        "guide",
        digest('2'),
        [],
    )
    .unwrap();
    let set = CapabilitySet::from_use_projection(
        CodeCatalogGeneration::new(2),
        generation.clone(),
        [CapabilityContribution::new(source, [guide]).unwrap()],
    )
    .unwrap();
    let ceiling = CapabilityCeiling::all(
        &set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required()
            .require_permission_guard()
            .require_security_guard(),
        execution(10, 2),
    )
    .unwrap();
    (set, generation, ceiling)
}

struct RecordingEffect {
    name: &'static str,
    log: Arc<Mutex<Vec<String>>>,
    pending: bool,
}

struct BlockingEffect {
    name: &'static str,
    started: Option<tokio::sync::oneshot::Sender<()>>,
    release: tokio::sync::oneshot::Receiver<()>,
    log: Arc<Mutex<Vec<String>>>,
}

struct FailingEffect {
    name: &'static str,
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl CapabilityEffect for FailingEffect {
    fn name(&self) -> &str {
        self.name
    }

    async fn close(self: Box<Self>) -> Result<(), CapabilityEffectError> {
        self.log.lock().unwrap().push(self.name.to_owned());
        Err(CapabilityEffectError::new("expected close failure"))
    }
}

#[async_trait]
impl CapabilityEffect for BlockingEffect {
    fn name(&self) -> &str {
        self.name
    }

    async fn close(mut self: Box<Self>) -> Result<(), CapabilityEffectError> {
        if let Some(started) = self.started.take() {
            let _ = started.send(());
        }
        let _ = self.release.await;
        self.log.lock().unwrap().push(self.name.to_owned());
        Ok(())
    }
}

#[async_trait]
impl CapabilityEffect for RecordingEffect {
    fn name(&self) -> &str {
        self.name
    }

    async fn close(self: Box<Self>) -> Result<(), CapabilityEffectError> {
        if self.pending {
            std::future::pending::<()>().await;
        }
        self.log.lock().unwrap().push(self.name.to_owned());
        Ok(())
    }
}

#[tokio::test]
async fn run_admission_requires_and_releases_the_exact_use_generation() {
    let (set, generation, root_ceiling) = use_catalog();
    let session = CapabilityScope::<Session>::new_session(
        "session-1",
        Arc::clone(&set),
        root_ceiling.clone(),
    )
    .unwrap();
    assert!(matches!(
        session.admit_run("missing", root_ceiling.clone()),
        Err(CapabilityScopeError::MissingUseGenerationLease)
    ));

    let log = Arc::new(Mutex::new(Vec::new()));
    let mismatch = FakeUseLease {
        generation: UseCapabilityGeneration::new(7, digest('9'), digest('e')),
        log: Arc::clone(&log),
        label: "mismatch",
    };
    assert!(matches!(
        session.admit_use_run("mismatch", root_ceiling.clone(), mismatch),
        Err(CapabilityScopeError::UseGenerationLeaseMismatch {
            revision_mismatch: true,
            ..
        })
    ));
    assert_eq!(&*log.lock().unwrap(), &["mismatch"]);

    let exact = FakeUseLease {
        generation: generation.clone(),
        log: Arc::clone(&log),
        label: "use",
    };
    let run = session.admit_use_run("exact", root_ceiling, exact).unwrap();
    run.register_effect(RecordingEffect {
        name: "runtime.effect",
        log: Arc::clone(&log),
        pending: false,
    })
    .unwrap();
    assert_eq!(run.use_generation(), Some(&generation));

    let report = run.close().await.unwrap();
    assert_eq!(report.effects_closed, 1);
    assert_eq!(report.generation_leases_released, 1);
    assert_eq!(
        &*log.lock().unwrap(),
        &["mismatch", "runtime.effect", "use"]
    );
    assert_eq!(run.close().await.unwrap(), report);
    session.close().await.unwrap();
}

#[tokio::test]
async fn supervisor_cancels_tasks_then_closes_effects_in_reverse_order() {
    let catalog = host_catalog();
    let root = CapabilityCeiling::all(
        &catalog.set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        execution(20, 4),
    )
    .unwrap();
    let session =
        CapabilityScope::<Session>::new_session("session-1", catalog.set, root.clone()).unwrap();
    let run = session.admit_run("run-1", root.clone()).unwrap();
    let turn = run.turn("turn-1", root).unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));

    for name in ["effect.one", "effect.two", "effect.three"] {
        turn.register_effect(RecordingEffect {
            name,
            log: Arc::clone(&log),
            pending: false,
        })
        .unwrap();
    }
    let cancellation = turn.cancellation();
    let task_log = Arc::clone(&log);
    turn.spawn_task("turn.worker", async move {
        cancellation.cancelled().await;
        task_log.lock().unwrap().push("task".to_owned());
        Ok(())
    })
    .unwrap();

    let report = turn.close().await.unwrap();
    assert!(report.is_clean());
    assert_eq!(report.tasks_completed, 1);
    assert_eq!(report.effects_closed, 3);
    assert_eq!(
        &*log.lock().unwrap(),
        &["task", "effect.three", "effect.two", "effect.one"]
    );
    assert_eq!(turn.close().await.unwrap(), report);
    assert!(matches!(
        turn.lease(),
        Err(CapabilityScopeError::ScopeInactive { .. })
    ));
    assert!(matches!(
        turn.register_effect(RecordingEffect {
            name: "effect.late",
            log: Arc::clone(&log),
            pending: false,
        }),
        Err(CapabilityScopeError::SupervisorClosed { .. })
    ));

    run.close().await.unwrap();
    session.close().await.unwrap();
}

#[tokio::test]
async fn close_driver_survives_cancellation_of_the_first_waiter() {
    let catalog = host_catalog();
    let root = CapabilityCeiling::all(
        &catalog.set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        execution(20, 4),
    )
    .unwrap();
    let scope =
        Arc::new(CapabilityScope::<Session>::new_session("session-1", catalog.set, root).unwrap());
    let log = Arc::new(Mutex::new(Vec::new()));
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    scope
        .register_effect(BlockingEffect {
            name: "blocking.effect",
            started: Some(started_tx),
            release: release_rx,
            log: Arc::clone(&log),
        })
        .unwrap();

    let first_scope = Arc::clone(&scope);
    let first_waiter = tokio::spawn(async move { first_scope.close().await });
    started_rx.await.unwrap();
    first_waiter.abort();
    assert!(first_waiter.await.unwrap_err().is_cancelled());
    release_tx.send(()).unwrap();

    let report = scope.close().await.unwrap();
    assert!(report.is_clean());
    assert_eq!(report.effects_closed, 1);
    assert_eq!(&*log.lock().unwrap(), &["blocking.effect"]);
}

#[tokio::test]
async fn one_effect_failure_does_not_skip_older_reverse_teardown() {
    let catalog = host_catalog();
    let root = CapabilityCeiling::all(
        &catalog.set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        execution(20, 4),
    )
    .unwrap();
    let scope = CapabilityScope::<Session>::new_session("session-1", catalog.set, root).unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    scope
        .register_effect(RecordingEffect {
            name: "effect.oldest",
            log: Arc::clone(&log),
            pending: false,
        })
        .unwrap();
    scope
        .register_effect(FailingEffect {
            name: "effect.failing",
            log: Arc::clone(&log),
        })
        .unwrap();
    scope
        .register_effect(RecordingEffect {
            name: "effect.newest",
            log: Arc::clone(&log),
            pending: false,
        })
        .unwrap();

    let report = scope.close().await.unwrap();
    assert_eq!(report.effects_closed, 2);
    assert_eq!(report.effects_failed, 1);
    assert!(!report.is_clean());
    assert_eq!(
        &*log.lock().unwrap(),
        &["effect.newest", "effect.failing", "effect.oldest"]
    );
}

#[tokio::test]
async fn parent_close_recursively_settles_subtasks_before_releasing_use() {
    let (set, generation, ceiling) = use_catalog();
    let session =
        CapabilityScope::<Session>::new_session("session-1", set, ceiling.clone()).unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let run = session
        .admit_use_run(
            "run-1",
            ceiling.clone(),
            FakeUseLease {
                generation,
                log: Arc::clone(&log),
                label: "use",
            },
        )
        .unwrap();
    let turn = run.turn("turn-1", ceiling.clone()).unwrap();
    let subtask = turn.subtask("subtask-1", ceiling).unwrap();

    for (scope, name) in [
        (&session as &dyn EffectRegistrar, "session.effect"),
        (&run as &dyn EffectRegistrar, "run.effect"),
        (&turn as &dyn EffectRegistrar, "turn.effect"),
        (&subtask as &dyn EffectRegistrar, "subtask.effect"),
    ] {
        scope.register(RecordingEffect {
            name,
            log: Arc::clone(&log),
            pending: false,
        });
    }

    let report = session.close().await.unwrap();
    assert!(report.is_clean());
    assert_eq!(report.child_scopes_closed, 1);
    assert_eq!(report.effects_closed, 1);
    assert_eq!(
        &*log.lock().unwrap(),
        &[
            "subtask.effect",
            "turn.effect",
            "run.effect",
            "use",
            "session.effect"
        ]
    );
    assert!(!run.is_active());
    assert!(!turn.is_active());
    assert!(!subtask.is_active());
    assert_eq!(run.close().await.unwrap().generation_leases_released, 1);
}

trait EffectRegistrar {
    fn register(&self, effect: RecordingEffect);
}

impl<K: ScopeKind> EffectRegistrar for CapabilityScope<K> {
    fn register(&self, effect: RecordingEffect) {
        self.register_effect(effect).unwrap();
    }
}

#[tokio::test]
async fn close_is_bounded_when_tasks_and_effects_ignore_cancellation() {
    let catalog = host_catalog();
    let root = CapabilityCeiling::all(
        &catalog.set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        execution(20, 4),
    )
    .unwrap();
    let policy = ScopeClosePolicy::new(Duration::from_millis(30)).unwrap();
    let session = CapabilityScope::<Session>::new_session_with_close_policy(
        "session-1",
        catalog.set,
        root,
        policy,
    )
    .unwrap();
    session
        .spawn_task("stuck.task", std::future::pending())
        .unwrap();
    session
        .register_effect(RecordingEffect {
            name: "stuck.effect",
            log: Arc::new(Mutex::new(Vec::new())),
            pending: true,
        })
        .unwrap();

    let started = Instant::now();
    let report = session.close().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(report.tasks_timed_out, 1);
    assert_eq!(report.effects_timed_out, 1);
    assert!(!report.is_clean());
}

struct TaskDropProbe {
    dropped: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for TaskDropProbe {
    fn drop(&mut self) {
        self.dropped
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

#[tokio::test]
async fn dropping_a_parent_aborts_descendant_tasks_without_spawning_cleanup() {
    let catalog = host_catalog();
    let root = CapabilityCeiling::all(
        &catalog.set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        execution(20, 4),
    )
    .unwrap();
    let session =
        CapabilityScope::<Session>::new_session("session-1", catalog.set, root.clone()).unwrap();
    let run = session.admit_run("run-1", root).unwrap();
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let probe = TaskDropProbe {
        dropped: Arc::clone(&dropped),
    };
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    run.spawn_task("descendant.task", async move {
        let _probe = probe;
        let _ = started_tx.send(());
        std::future::pending::<Result<(), CapabilityEffectError>>().await
    })
    .unwrap();
    started_rx.await.unwrap();

    drop(session);
    for _ in 0..100 {
        if dropped.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(dropped.load(std::sync::atomic::Ordering::Acquire));
    assert!(!run.is_active());
    assert_eq!(run.close().await.unwrap().tasks_cancelled, 1);
}

#[test]
fn public_scope_types_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<CapabilityScope<Session>>();
    assert_send_sync::<CapabilityScope<Run>>();
    assert_send_sync::<CapabilityCeiling>();
}
