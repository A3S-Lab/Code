use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use a3s_code_core::capability::{
    CapabilityAdapterError, CapabilityCatalog, CapabilityContribution, CapabilityDescriptor,
    CapabilityEffect, CapabilityEffectError, CapabilityId, CapabilityKind, CapabilityProjection,
    CapabilityProjectionAdapter, CapabilityProjectionError, CapabilityReadinessPlan, CapabilitySet,
    CapabilitySource, CapabilityValue, CodeCatalogGeneration, PreparedCapability, Sha256Digest,
    UseCapabilityGeneration, UsePackageGeneration, MAX_CAPABILITIES,
    MAX_CAPABILITY_READINESS_WAVES,
};
use a3s_code_core::tools::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
}

fn host_source(name: &str) -> CapabilitySource {
    CapabilitySource::host(name, digest('a')).unwrap()
}

fn id(source: &CapabilitySource, name: &str) -> CapabilityId {
    CapabilityId::new(source, CapabilityKind::Tool, name).unwrap()
}

fn descriptor(
    source: &CapabilitySource,
    name: &str,
    dependencies: impl IntoIterator<Item = CapabilityId>,
) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        source,
        CapabilityKind::Tool,
        name,
        name,
        digest('b'),
        dependencies,
    )
    .unwrap()
}

fn set(
    generation: u64,
    source: CapabilitySource,
    descriptors: impl IntoIterator<Item = CapabilityDescriptor>,
) -> Arc<CapabilitySet> {
    CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(generation),
        [CapabilityContribution::new(source, descriptors).unwrap()],
    )
    .unwrap()
}

fn empty_catalog() -> CapabilityCatalog {
    let projection =
        CapabilityProjection::new(CapabilitySet::empty().unwrap(), BTreeMap::new()).unwrap();
    CapabilityCatalog::new(projection)
}

fn names(wave: &[CapabilityId]) -> Vec<&str> {
    wave.iter().map(CapabilityId::local_id).collect()
}

#[test]
fn readiness_plan_is_canonical_and_uses_minimal_dependency_waves() {
    let source = host_source("readiness-order");
    let root_a = id(&source, "root-a");
    let root_b = id(&source, "root-b");
    let left = id(&source, "left");
    let right = id(&source, "right");
    let descriptors = vec![
        descriptor(
            &source,
            "top",
            [left.clone(), right.clone(), root_b.clone()],
        ),
        descriptor(&source, "right", [root_a.clone()]),
        descriptor(&source, "root-b", []),
        descriptor(&source, "left", [root_a.clone()]),
        descriptor(&source, "root-a", []),
    ];
    let forward = set(1, source.clone(), descriptors.clone());
    let reverse = set(1, source, descriptors.into_iter().rev());

    let forward_plan = CapabilityReadinessPlan::from_set(&forward).unwrap();
    let reverse_plan = CapabilityReadinessPlan::from_set(&reverse).unwrap();

    assert_eq!(forward_plan, reverse_plan);
    assert_eq!(
        forward_plan.schema(),
        "a3s.code.capability-readiness-plan.v1"
    );
    assert_eq!(forward_plan.generation(), CodeCatalogGeneration::new(1));
    assert_eq!(forward_plan.digest(), forward.digest());
    assert_eq!(forward_plan.edge_count(), 5);
    assert_eq!(forward_plan.depth(), 3);
    assert_eq!(forward_plan.max_wave_width(), 2);
    assert_eq!(names(&forward_plan.waves()[0]), ["root-a", "root-b"]);
    assert_eq!(names(&forward_plan.waves()[1]), ["left", "right"]);
    assert_eq!(names(&forward_plan.waves()[2]), ["top"]);
    assert_eq!(
        names(forward_plan.activation_order()),
        ["root-a", "root-b", "left", "right", "top"]
    );
}

#[test]
fn empty_surface_has_a_valid_empty_readiness_plan() {
    let set = CapabilitySet::empty().unwrap();
    let plan = CapabilityReadinessPlan::from_set(&set).unwrap();

    assert!(plan.is_empty());
    assert_eq!(plan.capability_count(), 0);
    assert_eq!(plan.edge_count(), 0);
    assert_eq!(plan.depth(), 0);
    assert_eq!(plan.max_wave_width(), 0);
    assert!(plan.waves().is_empty());
    assert!(plan.activation_order().is_empty());
}

#[test]
fn dependency_cycles_fail_before_a_runtime_projection_or_transaction_can_exist() {
    let source = host_source("readiness-cycle");
    let alpha = id(&source, "alpha");
    let beta = id(&source, "beta");
    let cyclic = set(
        1,
        source.clone(),
        [
            descriptor(&source, "alpha", [beta.clone()]),
            descriptor(&source, "beta", [alpha.clone()]),
            descriptor(&source, "downstream", [beta]),
        ],
    );

    let expected = CapabilityProjectionError::DependencyCycle {
        first_blocked: alpha.to_string(),
        blocked_count: 3,
    };
    assert_eq!(
        CapabilityReadinessPlan::from_set(&cyclic).unwrap_err(),
        expected
    );
    assert_eq!(
        CapabilityProjection::new(Arc::clone(&cyclic), BTreeMap::new()).unwrap_err(),
        expected
    );
    assert_eq!(empty_catalog().begin(cyclic).unwrap_err(), expected);

    let source = host_source("readiness-cycle-three");
    let alpha = id(&source, "alpha");
    let beta = id(&source, "beta");
    let gamma = id(&source, "gamma");
    let cyclic = set(
        1,
        source.clone(),
        [
            descriptor(&source, "gamma", [alpha.clone()]),
            descriptor(&source, "alpha", [beta.clone()]),
            descriptor(&source, "beta", [gamma]),
        ],
    );
    assert!(matches!(
        CapabilityReadinessPlan::from_set(&cyclic),
        Err(CapabilityProjectionError::DependencyCycle {
            first_blocked,
            blocked_count: 3,
        }) if first_blocked == alpha.to_string()
    ));
}

#[derive(Clone)]
struct NamedTool(String);

#[async_trait]
impl Tool for NamedTool {
    fn name(&self) -> &str {
        &self.0
    }

    fn description(&self) -> &str {
        "readiness test tool"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    async fn execute(
        &self,
        _args: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::success("ok"))
    }
}

fn tool(name: &str) -> CapabilityValue {
    CapabilityValue::Tool(Arc::new(NamedTool(name.to_owned())))
}

struct RecordingEffect {
    name: String,
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl CapabilityEffect for RecordingEffect {
    fn name(&self) -> &str {
        &self.name
    }

    async fn close(self: Box<Self>) -> Result<(), CapabilityEffectError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("close:{}", self.name));
        Ok(())
    }
}

struct RecordingAdapter {
    name: String,
    log: Arc<Mutex<Vec<String>>>,
    starts: Arc<AtomicUsize>,
    fail: bool,
    effect: bool,
}

impl RecordingAdapter {
    fn ready(name: &str, log: &Arc<Mutex<Vec<String>>>, starts: &Arc<AtomicUsize>) -> Self {
        Self {
            name: name.to_owned(),
            log: Arc::clone(log),
            starts: Arc::clone(starts),
            fail: false,
            effect: true,
        }
    }

    fn failing(name: &str, log: &Arc<Mutex<Vec<String>>>, starts: &Arc<AtomicUsize>) -> Self {
        Self {
            name: name.to_owned(),
            log: Arc::clone(log),
            starts: Arc::clone(starts),
            fail: true,
            effect: false,
        }
    }
}

#[async_trait]
impl CapabilityProjectionAdapter for RecordingAdapter {
    async fn prepare(
        self: Box<Self>,
        _cancellation: CancellationToken,
    ) -> Result<PreparedCapability, CapabilityAdapterError> {
        self.starts.fetch_add(1, Ordering::Relaxed);
        self.log
            .lock()
            .unwrap()
            .push(format!("start:{}", self.name));
        if self.fail {
            return Err(CapabilityAdapterError::new("expected readiness failure"));
        }
        let mut prepared = PreparedCapability::new(tool(&self.name));
        if self.effect {
            prepared.push_effect(RecordingEffect {
                name: self.name.clone(),
                log: Arc::clone(&self.log),
            })?;
        }
        Ok(prepared)
    }
}

#[tokio::test]
async fn missing_staged_adapter_fails_before_any_adapter_starts() {
    let source = host_source("readiness-completeness");
    let base = id(&source, "base");
    let target = set(
        1,
        source.clone(),
        [
            descriptor(&source, "base", []),
            descriptor(&source, "consumer", [base.clone()]),
        ],
    );
    let starts = Arc::new(AtomicUsize::new(0));
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut txn = empty_catalog().begin(target).unwrap();
    txn.stage(base, RecordingAdapter::ready("base", &log, &starts))
        .unwrap();

    assert!(matches!(
        txn.prepare(CancellationToken::new()).await,
        Err(CapabilityProjectionError::MissingStagedCapability { capability })
            if capability.ends_with(":tool:consumer")
    ));
    assert_eq!(starts.load(Ordering::Relaxed), 0);
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test]
async fn preparation_follows_surface_dependencies_not_capability_id_order() {
    let source = host_source("readiness-prepare-order");
    let base = id(&source, "z-base");
    let consumer = id(&source, "a-consumer");
    let target = set(
        1,
        source.clone(),
        [
            descriptor(&source, "a-consumer", [base.clone()]),
            descriptor(&source, "z-base", []),
        ],
    );
    let starts = Arc::new(AtomicUsize::new(0));
    let log = Arc::new(Mutex::new(Vec::new()));
    let catalog = empty_catalog();
    let mut txn = catalog.begin(target).unwrap();
    txn.stage(
        consumer,
        RecordingAdapter::ready("a-consumer", &log, &starts),
    )
    .unwrap();
    txn.stage(base, RecordingAdapter::ready("z-base", &log, &starts))
        .unwrap();

    txn.prepare(CancellationToken::new())
        .await
        .unwrap()
        .validate()
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(&*log.lock().unwrap(), &["start:z-base", "start:a-consumer"]);
}

#[tokio::test]
async fn dependency_failure_blocks_dependents_and_rolls_back_ready_prerequisites() {
    let source = host_source("readiness-failure");
    let root = id(&source, "z-root");
    let middle = id(&source, "m-middle");
    let consumer = id(&source, "a-consumer");
    let target = set(
        1,
        source.clone(),
        [
            descriptor(&source, "a-consumer", [middle.clone()]),
            descriptor(&source, "m-middle", [root.clone()]),
            descriptor(&source, "z-root", []),
        ],
    );
    let starts = Arc::new(AtomicUsize::new(0));
    let log = Arc::new(Mutex::new(Vec::new()));
    let catalog = empty_catalog();
    let mut txn = catalog.begin(target).unwrap();
    txn.stage(
        consumer,
        RecordingAdapter::ready("a-consumer", &log, &starts),
    )
    .unwrap();
    txn.stage(middle, RecordingAdapter::failing("m-middle", &log, &starts))
        .unwrap();
    txn.stage(root, RecordingAdapter::ready("z-root", &log, &starts))
        .unwrap();

    assert!(matches!(
        txn.prepare(CancellationToken::new()).await,
        Err(CapabilityProjectionError::PrepareFailed { capability, .. })
            if capability.ends_with(":tool:m-middle")
    ));
    assert_eq!(starts.load(Ordering::Relaxed), 2);
    assert_eq!(&*log.lock().unwrap(), &["start:z-root", "start:m-middle"]);
    let report = catalog.drain_cleanup().await;
    assert_eq!(report.effects_closed, 1);
    assert_eq!(
        &*log.lock().unwrap(),
        &["start:z-root", "start:m-middle", "close:z-root"]
    );
}

#[tokio::test]
async fn projection_plan_retains_one_use_cursor_across_package_surface_edges() {
    let use_generation = UseCapabilityGeneration::new(9, digest('c'), digest('d'));
    let base_source = CapabilitySource::use_package(
        use_generation.clone(),
        UsePackageGeneration::new(
            "acme/base",
            "use/acme-base",
            "base",
            "1.0.0",
            4,
            digest('e'),
            digest('f'),
        )
        .unwrap(),
    )
    .unwrap();
    let consumer_source = CapabilitySource::use_package(
        use_generation.clone(),
        UsePackageGeneration::new(
            "acme/consumer",
            "use/acme-consumer",
            "consumer",
            "2.0.0",
            7,
            digest('1'),
            digest('2'),
        )
        .unwrap(),
    )
    .unwrap();
    let base = descriptor(&base_source, "base", []);
    let base_id = base.id().clone();
    let consumer = descriptor(&consumer_source, "consumer", [base_id.clone()]);
    let consumer_id = consumer.id().clone();
    let target = CapabilitySet::from_use_projection(
        CodeCatalogGeneration::new(1),
        use_generation.clone(),
        [
            CapabilityContribution::new(consumer_source, [consumer]).unwrap(),
            CapabilityContribution::new(base_source, [base]).unwrap(),
        ],
    )
    .unwrap();
    let catalog = empty_catalog();
    let mut txn = catalog.begin(Arc::clone(&target)).unwrap();
    txn.stage_value(consumer_id, tool("consumer")).unwrap();
    txn.stage_value(base_id, tool("base")).unwrap();
    txn.prepare(CancellationToken::new())
        .await
        .unwrap()
        .validate()
        .unwrap()
        .commit()
        .unwrap();

    let lease = catalog.pin();
    let projection = lease.projection();
    let plan = projection.readiness_plan();
    assert_eq!(plan.generation(), target.generation());
    assert_eq!(plan.digest(), target.digest());
    assert_eq!(plan.depth(), 2);
    assert_eq!(
        projection.set().use_capability_generation(),
        Some(&use_generation)
    );
}

#[test]
fn readiness_planning_is_iterative_at_the_configured_width_and_depth_bounds() {
    let wide_source = host_source("readiness-wide");
    let wide_descriptors = (0..MAX_CAPABILITIES)
        .map(|index| descriptor(&wide_source, &format!("node-{index:04}"), []))
        .collect::<Vec<_>>();
    let wide_set = set(1, wide_source, wide_descriptors);
    let wide = CapabilityReadinessPlan::from_set(&wide_set).unwrap();
    assert_eq!(wide.capability_count(), MAX_CAPABILITIES);
    assert_eq!(wide.depth(), 1);
    assert_eq!(wide.max_wave_width(), MAX_CAPABILITIES);

    let deep_source = host_source("readiness-deep");
    let deep_ids = (0..MAX_CAPABILITIES)
        .map(|index| id(&deep_source, &format!("node-{index:04}")))
        .collect::<Vec<_>>();
    let deep_descriptors = deep_ids
        .iter()
        .enumerate()
        .map(|(index, current)| {
            descriptor(
                &deep_source,
                current.local_id(),
                index
                    .checked_sub(1)
                    .map(|previous| deep_ids[previous].clone()),
            )
        })
        .collect::<Vec<_>>();
    let deep_set = set(1, deep_source, deep_descriptors);
    let deep = CapabilityReadinessPlan::from_set(&deep_set).unwrap();
    assert_eq!(deep.capability_count(), MAX_CAPABILITIES);
    assert_eq!(deep.depth(), MAX_CAPABILITY_READINESS_WAVES);
    assert_eq!(deep.max_wave_width(), 1);
    assert_eq!(deep.activation_order().first(), deep_ids.first());
    assert_eq!(deep.activation_order().last(), deep_ids.last());
}

#[test]
fn readiness_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<CapabilityReadinessPlan>();
    assert_send_sync::<CapabilityProjection>();
    assert_send_sync::<a3s_code_core::capability::CapabilityProjectionLease>();
}
