use super::*;

use a3s_flow::{
    FlowEngine, FlowRuntime, RuntimeBuildCompatibility, RuntimeBuildId, RuntimeCommand,
    StepInvocation, WorkflowInvocation, WorkflowSpec,
};
use serde_json::json;

use crate::capability::{CapabilityProjectionError, FlowBinding};

struct VersionedFlowRuntime {
    version: &'static str,
    executions: Arc<Mutex<Vec<&'static str>>>,
}

struct BlockingFlowRuntime {
    started: Arc<Semaphore>,
    cancelled: Arc<AtomicUsize>,
}

struct CancellationProbe(Arc<AtomicUsize>);

impl Drop for CancellationProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl FlowRuntime for VersionedFlowRuntime {
    async fn run_workflow(
        &self,
        invocation: WorkflowInvocation,
    ) -> a3s_flow::Result<RuntimeCommand> {
        self.executions.lock().unwrap().push(self.version);
        Ok(invocation.context().complete(json!({
            "version": self.version,
            "input": invocation.input,
        })))
    }

    async fn run_step(&self, _invocation: StepInvocation) -> a3s_flow::Result<serde_json::Value> {
        Err(a3s_flow::FlowError::Runtime(
            "the versioned Flow fixture does not schedule steps".to_owned(),
        ))
    }
}

#[async_trait]
impl FlowRuntime for BlockingFlowRuntime {
    async fn run_workflow(
        &self,
        _invocation: WorkflowInvocation,
    ) -> a3s_flow::Result<RuntimeCommand> {
        let _probe = CancellationProbe(Arc::clone(&self.cancelled));
        self.started.add_permits(1);
        std::future::pending().await
    }

    async fn run_step(&self, _invocation: StepInvocation) -> a3s_flow::Result<serde_json::Value> {
        Err(a3s_flow::FlowError::Runtime(
            "the blocking Flow fixture does not schedule steps".to_owned(),
        ))
    }
}

pub(super) fn flow_binding(
    name: &str,
    version: &'static str,
    executions: &Arc<Mutex<Vec<&'static str>>>,
) -> Arc<FlowBinding> {
    let build = RuntimeBuildId::new(format!("{name}-{version}")).unwrap();
    let runtime: Arc<dyn FlowRuntime> = Arc::new(VersionedFlowRuntime {
        version,
        executions: Arc::clone(executions),
    });
    let engine = FlowEngine::builder(runtime)
        .with_runtime_build_compatibility(RuntimeBuildCompatibility::new(build.clone()))
        .build();
    Arc::new(
        FlowBinding::new(
            WorkflowSpec::rust_embedded(name, version, "fixture", "run").with_runtime_build(build),
            engine,
        )
        .unwrap(),
    )
}

#[test]
fn flow_binding_rejects_a_spec_the_engine_cannot_replay() {
    let executions = Arc::new(Mutex::new(Vec::new()));
    let runtime: Arc<dyn FlowRuntime> = Arc::new(VersionedFlowRuntime {
        version: "v2",
        executions,
    });
    let engine = FlowEngine::builder(runtime)
        .with_runtime_build_compatibility(RuntimeBuildCompatibility::new(
            RuntimeBuildId::new("worker-v1").unwrap(),
        ))
        .build();
    let spec = WorkflowSpec::rust_embedded("projected-flow", "v2", "fixture", "run")
        .with_runtime_build(RuntimeBuildId::new("worker-v2").unwrap());

    let error = FlowBinding::new(spec, engine)
        .expect_err("a projected Flow must be executable by its exact engine");

    assert!(matches!(error, a3s_flow::FlowError::InvalidWorkflow(_)));
}

#[test]
fn projected_flow_public_contract_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<FlowBinding>();
    assert_send_sync::<crate::ProjectedFlowHandle>();
}

#[tokio::test]
async fn projected_flow_is_run_frozen_across_atomic_cutover() {
    let session = test_session("projected-flow-cutover").await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    let first_acquired = Arc::new(AtomicUsize::new(0));
    let first_dropped = Arc::new(AtomicUsize::new(0));
    let second_acquired = Arc::new(AtomicUsize::new(0));
    let second_dropped = Arc::new(AtomicUsize::new(0));

    let first_upstream = use_generation(1, 'a');
    let (first_set, first_ids) = use_kind_set(
        1,
        first_upstream.clone(),
        CapabilityKind::Flow,
        &[("projected-flow", 'b')],
    );
    let mut first = SessionCapabilityBatch::from_use_projection(
        first_set,
        provider(first_upstream, &first_acquired, &first_dropped),
    )
    .unwrap();
    first
        .stage_value(
            first_ids["projected-flow"].clone(),
            CapabilityValue::Flow(flow_binding("projected-flow", "v1", &executions)),
        )
        .unwrap();
    session
        .apply_capability_batch(first, CancellationToken::new())
        .await
        .unwrap();

    let old = session
        .projected_flow("projected-flow")
        .await
        .unwrap()
        .expect("generation one Flow must be visible");
    assert_eq!(old.spec().version, "v1");
    assert_eq!(first_acquired.load(Ordering::SeqCst), 1);
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);

    let second_upstream = use_generation(2, 'c');
    let (second_set, second_ids) = use_kind_set(
        2,
        second_upstream.clone(),
        CapabilityKind::Flow,
        &[("projected-flow", 'd')],
    );
    let mut second = SessionCapabilityBatch::from_use_projection(
        second_set,
        provider(second_upstream, &second_acquired, &second_dropped),
    )
    .unwrap();
    second
        .stage_value(
            second_ids["projected-flow"].clone(),
            CapabilityValue::Flow(flow_binding("projected-flow", "v2", &executions)),
        )
        .unwrap();
    session
        .apply_capability_batch(second, CancellationToken::new())
        .await
        .unwrap();

    assert_eq!(old.spec().version, "v1");
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(second_acquired.load(Ordering::SeqCst), 0);
    let old_run_id = old.start(json!({ "generation": 1 })).await.unwrap();
    let old_snapshot = old.snapshot(&old_run_id).await.unwrap();
    assert_eq!(
        old_snapshot.output,
        Some(json!({
            "version": "v1",
            "input": { "generation": 1 },
        }))
    );
    old.close().await.unwrap();
    assert_eq!(first_dropped.load(Ordering::SeqCst), 1);

    let new = session
        .projected_flow("projected-flow")
        .await
        .unwrap()
        .expect("generation two Flow must be visible");
    assert_eq!(new.spec().version, "v2");
    assert_eq!(second_acquired.load(Ordering::SeqCst), 1);
    let new_run_id = new.start(json!({ "generation": 2 })).await.unwrap();
    let new_snapshot = new.snapshot(&new_run_id).await.unwrap();
    assert_eq!(
        new_snapshot.output,
        Some(json!({
            "version": "v2",
            "input": { "generation": 2 },
        }))
    );
    new.close().await.unwrap();
    assert_eq!(second_dropped.load(Ordering::SeqCst), 1);
    assert_eq!(&*executions.lock().unwrap(), &["v1", "v2"]);
}

#[tokio::test]
async fn projected_flow_name_is_bound_and_missing_lookup_acquires_no_lease() {
    let session = test_session("projected-flow-name-binding").await;
    let executions = Arc::new(Mutex::new(Vec::new()));
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Flow,
        &[("projected-flow", 'b')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-flow"].clone(),
            CapabilityValue::Flow(flow_binding("different-flow", "v1", &executions)),
        )
        .unwrap();
    let before = session.capability_catalog_stamp();

    let error = session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .expect_err("the Flow spec name must match its public descriptor name");

    assert!(matches!(
        error,
        CapabilityRuntimeError::Projection(CapabilityProjectionError::PublicNameMismatch {
            ref expected,
            ref actual,
            ..
        }) if expected == "projected-flow" && actual == "different-flow"
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    assert_eq!(acquired.load(Ordering::SeqCst), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);

    assert!(session
        .projected_flow("missing-flow")
        .await
        .unwrap()
        .is_none());
    assert_eq!(acquired.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn session_close_cancels_an_active_projected_flow_and_releases_its_lease() {
    let session = test_session("projected-flow-cancellation").await;
    let started = Arc::new(Semaphore::new(0));
    let cancelled = Arc::new(AtomicUsize::new(0));
    let runtime: Arc<dyn FlowRuntime> = Arc::new(BlockingFlowRuntime {
        started: Arc::clone(&started),
        cancelled: Arc::clone(&cancelled),
    });
    let binding = Arc::new(
        FlowBinding::new(
            WorkflowSpec::rust_embedded("projected-flow", "v1", "fixture", "run"),
            FlowEngine::in_memory(runtime),
        )
        .unwrap(),
    );
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let (set, ids) = use_kind_set(
        1,
        upstream.clone(),
        CapabilityKind::Flow,
        &[("projected-flow", 'b')],
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            ids["projected-flow"].clone(),
            CapabilityValue::Flow(binding),
        )
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();
    let handle = session
        .projected_flow("projected-flow")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(acquired.load(Ordering::SeqCst), 1);

    let active = tokio::spawn(async move {
        let result = handle.start(json!({})).await;
        (handle, result)
    });
    started.acquire().await.unwrap().forget();
    session.close().await;
    let (handle, error) = tokio::time::timeout(Duration::from_secs(5), active)
        .await
        .expect("projected Flow must settle after Session close")
        .unwrap();

    assert!(matches!(
        error,
        Err(crate::error::CodeError::Capability(
            CapabilityRuntimeError::Cancelled
        ))
    ));
    assert_eq!(cancelled.load(Ordering::SeqCst), 1);
    handle.close().await.unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
