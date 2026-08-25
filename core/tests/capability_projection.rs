use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a3s_code_core::capability::{
    CapabilityAdapterError, CapabilityCatalog, CapabilityContribution, CapabilityDescriptor,
    CapabilityEffect, CapabilityEffectError, CapabilityKind, CapabilityProjection,
    CapabilityProjectionAdapter, CapabilityProjectionError, CapabilitySet, CapabilitySource,
    CapabilityValue, CodeCatalogGeneration, KnowledgeSurfaceBinding, KnowledgeSurfaceBindingSpec,
    PreparedCapability, ScopeClosePolicy, Sha256Digest, UiAsset, UiAssetKind, UiBinding,
    UiBindingSpec, UiDocument, UseCapabilityGeneration, UsePackageGeneration,
    MAX_CAPABILITY_TRANSACTION_EFFECTS,
};
use a3s_code_core::subagent::AgentDefinition;
use a3s_code_core::tools::{Tool, ToolContext, ToolOutput};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
}

#[derive(Clone)]
struct NamedTool(String);

#[async_trait]
impl Tool for NamedTool {
    fn name(&self) -> &str {
        &self.0
    }

    fn description(&self) -> &str {
        "projection test tool"
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

fn tool(name: &str) -> Arc<dyn Tool> {
    Arc::new(NamedTool(name.to_owned()))
}

fn empty_catalog() -> CapabilityCatalog {
    let set = CapabilitySet::empty().unwrap();
    let projection = CapabilityProjection::new(set, BTreeMap::new()).unwrap();
    CapabilityCatalog::new(projection)
}

fn tool_set(
    generation: u64,
    names: &[(&str, char)],
) -> (
    Arc<CapabilitySet>,
    BTreeMap<String, a3s_code_core::capability::CapabilityId>,
) {
    let source = CapabilitySource::host("projection-tests", digest('a')).unwrap();
    let mut ids = BTreeMap::new();
    let descriptors = names
        .iter()
        .map(|(name, surface)| {
            let descriptor = CapabilityDescriptor::new(
                &source,
                CapabilityKind::Tool,
                *name,
                *name,
                digest(*surface),
                [],
            )
            .unwrap();
            ids.insert((*name).to_owned(), descriptor.id().clone());
            descriptor
        })
        .collect::<Vec<_>>();
    let contribution = CapabilityContribution::new(source, descriptors).unwrap();
    let set =
        CapabilitySet::from_contributions(CodeCatalogGeneration::new(generation), [contribution])
            .unwrap();
    (set, ids)
}

struct RecordingEffect {
    name: &'static str,
    log: Arc<Mutex<Vec<String>>>,
}

struct CountingEffect(Arc<AtomicUsize>);

#[async_trait]
impl CapabilityEffect for CountingEffect {
    fn name(&self) -> &str {
        "projection.counting-effect"
    }

    async fn close(self: Box<Self>) -> Result<(), CapabilityEffectError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[async_trait]
impl CapabilityEffect for RecordingEffect {
    fn name(&self) -> &str {
        self.name
    }

    async fn close(self: Box<Self>) -> Result<(), CapabilityEffectError> {
        self.log.lock().unwrap().push(self.name.to_owned());
        Ok(())
    }
}

struct ReadyAdapter {
    value: CapabilityValue,
    effects: Vec<Box<dyn CapabilityEffect>>,
}

#[async_trait]
impl CapabilityProjectionAdapter for ReadyAdapter {
    async fn prepare(
        self: Box<Self>,
        _cancellation: CancellationToken,
    ) -> Result<PreparedCapability, CapabilityAdapterError> {
        let ReadyAdapter { value, effects } = *self;
        let mut prepared = PreparedCapability::new(value);
        for effect in effects {
            prepared.push_boxed_effect(effect)?;
        }
        Ok(prepared)
    }
}

struct FailingAdapter;

#[async_trait]
impl CapabilityProjectionAdapter for FailingAdapter {
    async fn prepare(
        self: Box<Self>,
        _cancellation: CancellationToken,
    ) -> Result<PreparedCapability, CapabilityAdapterError> {
        Err(CapabilityAdapterError::new("expected prepare failure"))
    }
}

struct PendingAdapter {
    started: Option<tokio::sync::oneshot::Sender<()>>,
}

#[async_trait]
impl CapabilityProjectionAdapter for PendingAdapter {
    async fn prepare(
        mut self: Box<Self>,
        _cancellation: CancellationToken,
    ) -> Result<PreparedCapability, CapabilityAdapterError> {
        if let Some(started) = self.started.take() {
            let _ = started.send(());
        }
        std::future::pending().await
    }
}

fn ready(
    value: CapabilityValue,
    effect_name: &'static str,
    log: &Arc<Mutex<Vec<String>>>,
) -> ReadyAdapter {
    ReadyAdapter {
        value,
        effects: vec![Box::new(RecordingEffect {
            name: effect_name,
            log: Arc::clone(log),
        })],
    }
}

#[test]
fn immutable_projection_rejects_incomplete_or_mismatched_values() {
    let (set, ids) = tool_set(1, &[("read", 'b')]);
    let read_id = ids.get("read").unwrap().clone();
    let read = tool("read");
    let projection = CapabilityProjection::new(
        Arc::clone(&set),
        [(read_id.clone(), CapabilityValue::Tool(Arc::clone(&read)))],
    )
    .unwrap();
    assert!(std::ptr::eq(
        projection.tool(&read_id).unwrap(),
        read.as_ref()
    ));

    assert!(matches!(
        CapabilityProjection::new(Arc::clone(&set), BTreeMap::new()),
        Err(CapabilityProjectionError::MissingValue { .. })
    ));
    assert!(matches!(
        CapabilityProjection::new(
            Arc::clone(&set),
            [(
                read_id.clone(),
                CapabilityValue::Agent(Arc::new(AgentDefinition::new("read", "test")))
            )],
        ),
        Err(CapabilityProjectionError::KindMismatch { .. })
    ));
    assert!(matches!(
        CapabilityProjection::new(
            Arc::clone(&set),
            [(read_id, CapabilityValue::Tool(tool("other")))],
        ),
        Err(CapabilityProjectionError::PublicNameMismatch { .. })
    ));
}

#[test]
fn ui_projection_binds_path_free_content_digest_and_backend_edge_kinds() {
    let binding = Arc::new(
        UiBinding::new(UiBindingSpec {
            public_name: "panel".to_owned(),
            title: "Evidence panel".to_owned(),
            description: "Review exact generation evidence.".to_owned(),
            icon: "panel-top".to_owned(),
            order: 20,
            document: UiDocument::new(
                UiAsset::new(UiAssetKind::Html, "<!doctype html><main>exact</main>").unwrap(),
                [UiAsset::new(UiAssetKind::Style, "main { display: grid; }").unwrap()],
                [UiAsset::new(UiAssetKind::Script, "globalThis.ready = true;").unwrap()],
            )
            .unwrap(),
        })
        .unwrap(),
    );
    let ui_source = CapabilitySource::host("ui-tests", digest('c')).unwrap();
    let backend = CapabilityDescriptor::new(
        &ui_source,
        CapabilityKind::Tool,
        "panel-backend",
        "panel-backend",
        digest('d'),
        [],
    )
    .unwrap();
    let backend_id = backend.id().clone();
    let ui = CapabilityDescriptor::new(
        &ui_source,
        CapabilityKind::Ui,
        "panel",
        "panel",
        binding.surface_digest().clone(),
        [backend_id.clone()],
    )
    .unwrap();
    let ui_id = ui.id().clone();
    let ui_set = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(1),
        [CapabilityContribution::new(ui_source, [backend, ui]).unwrap()],
    )
    .unwrap();
    let projection = CapabilityProjection::new(
        ui_set,
        [
            (backend_id, CapabilityValue::Tool(tool("panel-backend"))),
            (ui_id.clone(), CapabilityValue::Ui(Arc::clone(&binding))),
        ],
    )
    .unwrap();
    assert!(std::ptr::eq(
        projection.ui(&ui_id).unwrap(),
        binding.as_ref()
    ));

    let wrong_source = CapabilitySource::host("ui-digest-tests", digest('e')).unwrap();
    let wrong_ui = CapabilityDescriptor::new(
        &wrong_source,
        CapabilityKind::Ui,
        "panel",
        "panel",
        digest('f'),
        [],
    )
    .unwrap();
    let wrong_ui_id = wrong_ui.id().clone();
    let wrong_set = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(1),
        [CapabilityContribution::new(wrong_source, [wrong_ui]).unwrap()],
    )
    .unwrap();
    assert!(matches!(
        CapabilityProjection::new(
            wrong_set,
            [(wrong_ui_id, CapabilityValue::Ui(Arc::clone(&binding)))]
        ),
        Err(CapabilityProjectionError::SurfaceDigestMismatch { .. })
    ));

    let unsupported_source = CapabilitySource::host("ui-edge-tests", digest('1')).unwrap();
    let agent = CapabilityDescriptor::new(
        &unsupported_source,
        CapabilityKind::Agent,
        "reviewer",
        "reviewer",
        digest('2'),
        [],
    )
    .unwrap();
    let agent_id = agent.id().clone();
    let ui = CapabilityDescriptor::new(
        &unsupported_source,
        CapabilityKind::Ui,
        "panel",
        "panel",
        binding.surface_digest().clone(),
        [agent_id.clone()],
    )
    .unwrap();
    let ui_id = ui.id().clone();
    let unsupported_set = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(1),
        [CapabilityContribution::new(unsupported_source, [agent, ui]).unwrap()],
    )
    .unwrap();
    assert!(matches!(
        CapabilityProjection::new(
            unsupported_set,
            [
                (
                    agent_id,
                    CapabilityValue::Agent(Arc::new(AgentDefinition::new("reviewer", "test")))
                ),
                (ui_id, CapabilityValue::Ui(binding)),
            ]
        ),
        Err(CapabilityProjectionError::UnsupportedUiDependencyKind {
            dependency_kind: CapabilityKind::Agent,
            ..
        })
    ));
}

#[test]
fn knowledge_surface_projection_is_multi_value_and_digest_bound() {
    let first = Arc::new(
        KnowledgeSurfaceBinding::new(KnowledgeSurfaceBindingSpec {
            public_name: "research:domain".to_owned(),
            format_version: "0.2".to_owned(),
            content_digest: digest('b'),
            projection_digests: vec![digest('d'), digest('c')],
        })
        .unwrap(),
    );
    assert_eq!(first.projection_digests(), &[digest('c'), digest('d')]);
    let second = Arc::new(
        KnowledgeSurfaceBinding::new(KnowledgeSurfaceBindingSpec {
            public_name: "operations:runbook".to_owned(),
            format_version: "0.2".to_owned(),
            content_digest: digest('e'),
            projection_digests: vec![digest('f')],
        })
        .unwrap(),
    );

    let source = CapabilitySource::host("knowledge-surface-tests", digest('1')).unwrap();
    let first_descriptor = CapabilityDescriptor::new(
        &source,
        CapabilityKind::KnowledgeSurface,
        "domain",
        first.public_name(),
        first.surface_digest().clone(),
        [],
    )
    .unwrap();
    let first_id = first_descriptor.id().clone();
    let second_descriptor = CapabilityDescriptor::new(
        &source,
        CapabilityKind::KnowledgeSurface,
        "runbook",
        second.public_name(),
        second.surface_digest().clone(),
        [],
    )
    .unwrap();
    let second_id = second_descriptor.id().clone();
    let set = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(1),
        [CapabilityContribution::new(source, [first_descriptor, second_descriptor]).unwrap()],
    )
    .unwrap();
    let projection = CapabilityProjection::new(
        Arc::clone(&set),
        [
            (
                first_id.clone(),
                CapabilityValue::KnowledgeSurface(Arc::clone(&first)),
            ),
            (
                second_id.clone(),
                CapabilityValue::KnowledgeSurface(Arc::clone(&second)),
            ),
        ],
    )
    .unwrap();
    assert!(std::ptr::eq(
        projection.knowledge_surface(&first_id).unwrap(),
        first.as_ref()
    ));
    assert!(std::ptr::eq(
        projection.knowledge_surface(&second_id).unwrap(),
        second.as_ref()
    ));

    let wrong_source = CapabilitySource::host("knowledge-digest-tests", digest('2')).unwrap();
    let wrong = CapabilityDescriptor::new(
        &wrong_source,
        CapabilityKind::KnowledgeSurface,
        "domain",
        first.public_name(),
        digest('3'),
        [],
    )
    .unwrap();
    let wrong_id = wrong.id().clone();
    let wrong_set = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(1),
        [CapabilityContribution::new(wrong_source, [wrong]).unwrap()],
    )
    .unwrap();
    assert!(matches!(
        CapabilityProjection::new(
            wrong_set,
            [(wrong_id, CapabilityValue::KnowledgeSurface(first))]
        ),
        Err(CapabilityProjectionError::SurfaceDigestMismatch { .. })
    ));
}

#[tokio::test]
async fn failed_prepare_and_cancellation_publish_nothing_and_rollback_in_reverse() {
    let catalog = empty_catalog();
    let (target, ids) = tool_set(1, &[("alpha", 'b'), ("beta", 'c'), ("gamma", 'd')]);
    let log = Arc::new(Mutex::new(Vec::new()));

    let mut failed = catalog.begin(Arc::clone(&target)).unwrap();
    failed
        .stage(
            ids["alpha"].clone(),
            ready(CapabilityValue::Tool(tool("alpha")), "alpha", &log),
        )
        .unwrap();
    failed
        .stage(
            ids["beta"].clone(),
            ready(CapabilityValue::Tool(tool("beta")), "beta", &log),
        )
        .unwrap();
    failed.stage(ids["gamma"].clone(), FailingAdapter).unwrap();
    assert!(matches!(
        failed.prepare(CancellationToken::new()).await,
        Err(CapabilityProjectionError::PrepareFailed { .. })
    ));
    assert_eq!(catalog.current_stamp().generation().get(), 0);
    let report = catalog.drain_cleanup().await;
    assert_eq!(report.rollback_batches, 1);
    assert_eq!(&*log.lock().unwrap(), &["beta", "alpha"]);

    let mut cancelled = catalog.begin(target).unwrap();
    cancelled
        .stage(
            ids["alpha"].clone(),
            ready(CapabilityValue::Tool(tool("alpha")), "cancel.alpha", &log),
        )
        .unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    cancelled
        .stage(
            ids["beta"].clone(),
            PendingAdapter {
                started: Some(started_tx),
            },
        )
        .unwrap();
    cancelled
        .stage(
            ids["gamma"].clone(),
            ready(CapabilityValue::Tool(tool("gamma")), "cancel.gamma", &log),
        )
        .unwrap();
    let cancellation = CancellationToken::new();
    let prepare = tokio::spawn({
        let cancellation = cancellation.clone();
        async move { cancelled.prepare(cancellation).await }
    });
    started_rx.await.unwrap();
    cancellation.cancel();
    assert!(matches!(
        prepare.await.unwrap(),
        Err(CapabilityProjectionError::Cancelled)
    ));
    assert_eq!(catalog.current_stamp().generation().get(), 0);
    catalog.drain_cleanup().await;
    assert_eq!(&*log.lock().unwrap(), &["beta", "alpha", "cancel.alpha"]);
}

#[tokio::test]
async fn failed_validation_rolls_back_every_prepared_effect_without_publication() {
    let catalog = empty_catalog();
    let (target, ids) = tool_set(1, &[("read", 'b')]);
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut txn = catalog.begin(target).unwrap();
    txn.stage(
        ids["read"].clone(),
        ready(
            CapabilityValue::Tool(tool("wrong-name")),
            "prepared.transport",
            &log,
        ),
    )
    .unwrap();

    let prepared = txn.prepare(CancellationToken::new()).await.unwrap();
    assert!(matches!(
        prepared.validate(),
        Err(CapabilityProjectionError::PublicNameMismatch { .. })
    ));
    assert_eq!(catalog.current_stamp().generation().get(), 0);
    let report = catalog.drain_cleanup().await;
    assert_eq!(report.effects_closed, 1);
    assert_eq!(&*log.lock().unwrap(), &["prepared.transport"]);
}

#[tokio::test]
async fn aggregate_effect_overflow_is_rejected_and_remains_recoverable() {
    let catalog = empty_catalog();
    let (target, ids) = tool_set(1, &[("read", 'b')]);
    let closed = Arc::new(AtomicUsize::new(0));
    let effects = (0..=MAX_CAPABILITY_TRANSACTION_EFFECTS)
        .map(|_| Box::new(CountingEffect(Arc::clone(&closed))) as Box<dyn CapabilityEffect>)
        .collect();
    let mut txn = catalog.begin(target).unwrap();
    txn.stage(
        ids["read"].clone(),
        ReadyAdapter {
            value: CapabilityValue::Tool(tool("read")),
            effects,
        },
    )
    .unwrap();

    assert!(matches!(
        txn.prepare(CancellationToken::new()).await,
        Err(CapabilityProjectionError::EffectBoundExceeded {
            max: MAX_CAPABILITY_TRANSACTION_EFFECTS
        })
    ));
    assert_eq!(catalog.current_stamp().generation().get(), 0);
    let report = catalog.drain_cleanup().await;
    assert_eq!(
        report.effects_closed,
        MAX_CAPABILITY_TRANSACTION_EFFECTS + 1
    );
    assert_eq!(
        closed.load(Ordering::Relaxed),
        MAX_CAPABILITY_TRANSACTION_EFFECTS + 1
    );
}

#[tokio::test]
async fn commit_race_publishes_exactly_one_complete_generation() {
    let catalog = Arc::new(empty_catalog());
    let (target, ids) = tool_set(1, &[("read", 'b')]);
    let first_tool = tool("read");
    let second_tool = tool("read");
    let log = Arc::new(Mutex::new(Vec::new()));

    let mut first = catalog.begin(Arc::clone(&target)).unwrap();
    first
        .stage(
            ids["read"].clone(),
            ready(
                CapabilityValue::Tool(Arc::clone(&first_tool)),
                "first",
                &log,
            ),
        )
        .unwrap();
    let first = first
        .prepare(CancellationToken::new())
        .await
        .unwrap()
        .validate()
        .unwrap();

    let mut second = catalog.begin(target).unwrap();
    second
        .stage(
            ids["read"].clone(),
            ready(
                CapabilityValue::Tool(Arc::clone(&second_tool)),
                "second",
                &log,
            ),
        )
        .unwrap();
    let second = second
        .prepare(CancellationToken::new())
        .await
        .unwrap()
        .validate()
        .unwrap();

    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let first_commit = tokio::spawn({
        let barrier = Arc::clone(&barrier);
        async move {
            barrier.wait().await;
            first.commit()
        }
    });
    let second_commit = tokio::spawn({
        let barrier = Arc::clone(&barrier);
        async move {
            barrier.wait().await;
            second.commit()
        }
    });
    barrier.wait().await;
    let first_result = first_commit.await.unwrap();
    let second_result = second_commit.await.unwrap();

    let (receipt, published_tool) = match (first_result, second_result) {
        (Ok(receipt), Err(CapabilityProjectionError::CommitConflict { .. })) => {
            (receipt, first_tool.as_ref())
        }
        (Err(CapabilityProjectionError::CommitConflict { .. }), Ok(receipt)) => {
            (receipt, second_tool.as_ref())
        }
        (first, second) => {
            panic!("exactly one commit must win: first={first:?}, second={second:?}")
        }
    };
    assert_eq!(receipt.committed().generation().get(), 1);

    let current = catalog.pin();
    assert_eq!(current.stamp(), receipt.committed());
    assert!(std::ptr::eq(
        current.projection().tool(&ids["read"]).unwrap(),
        published_tool
    ));
    let cleanup = catalog.drain_cleanup().await;
    assert_eq!(cleanup.rollback_batches, 1);
    assert_eq!(log.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn old_projection_and_effects_live_until_the_last_lease_is_released() {
    let catalog = empty_catalog();
    let log = Arc::new(Mutex::new(Vec::new()));
    let (first_set, first_ids) = tool_set(1, &[("read", 'b')]);
    let first_tool = tool("read");
    let mut first = catalog.begin(first_set).unwrap();
    first
        .stage(
            first_ids["read"].clone(),
            ready(
                CapabilityValue::Tool(Arc::clone(&first_tool)),
                "generation.one",
                &log,
            ),
        )
        .unwrap();
    first
        .prepare(CancellationToken::new())
        .await
        .unwrap()
        .validate()
        .unwrap()
        .commit()
        .unwrap();
    let old_run = catalog.pin();

    let (second_set, second_ids) = tool_set(2, &[("read", 'c')]);
    let second_tool = tool("read");
    let mut second = catalog.begin(second_set).unwrap();
    second
        .stage(
            second_ids["read"].clone(),
            ready(
                CapabilityValue::Tool(Arc::clone(&second_tool)),
                "generation.two",
                &log,
            ),
        )
        .unwrap();
    second
        .prepare(CancellationToken::new())
        .await
        .unwrap()
        .validate()
        .unwrap()
        .commit()
        .unwrap();

    assert_eq!(catalog.drain_cleanup().await.effects_closed, 0);
    assert!(std::ptr::eq(
        old_run.projection().tool(&first_ids["read"]).unwrap(),
        first_tool.as_ref()
    ));
    let new_run = catalog.pin();
    assert!(std::ptr::eq(
        new_run.projection().tool(&second_ids["read"]).unwrap(),
        second_tool.as_ref()
    ));

    drop(old_run);
    let cleanup = catalog
        .drain_cleanup_with_policy(ScopeClosePolicy::new(Duration::from_secs(1)).unwrap())
        .await;
    assert_eq!(cleanup.retired_batches, 1);
    assert_eq!(cleanup.effects_closed, 1);
    assert_eq!(&*log.lock().unwrap(), &["generation.one"]);
}

#[tokio::test]
async fn dropping_a_validated_transaction_is_a_recoverable_rollback() {
    let catalog = empty_catalog();
    let log = Arc::new(Mutex::new(Vec::new()));
    let (target, ids) = tool_set(1, &[("read", 'b')]);
    let mut txn = catalog.begin(target).unwrap();
    txn.stage(
        ids["read"].clone(),
        ready(CapabilityValue::Tool(tool("read")), "uncommitted", &log),
    )
    .unwrap();
    let validated = txn
        .prepare(CancellationToken::new())
        .await
        .unwrap()
        .validate()
        .unwrap();
    drop(validated);

    assert_eq!(catalog.current_stamp().generation().get(), 0);
    catalog.drain_cleanup().await;
    assert_eq!(&*log.lock().unwrap(), &["uncommitted"]);
}

#[test]
fn projection_catalog_and_pinned_values_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CapabilityCatalog>();
    assert_send_sync::<CapabilityProjection>();
    assert_send_sync::<a3s_code_core::capability::CapabilityProjectionLease>();
}

#[tokio::test]
async fn projected_catalog_retains_the_complete_use_cursor_without_owning_use_cutover() {
    let catalog = empty_catalog();
    let use_generation = UseCapabilityGeneration::new(17, digest('b'), digest('c'));
    let source = CapabilitySource::use_package(
        use_generation.clone(),
        UsePackageGeneration::new(
            "acme/tools",
            "use/acme-tools",
            "tools",
            "2.0.0",
            9,
            digest('d'),
            digest('e'),
        )
        .unwrap(),
    )
    .unwrap();
    let descriptor = CapabilityDescriptor::new(
        &source,
        CapabilityKind::Tool,
        "inspect",
        "inspect",
        digest('f'),
        [],
    )
    .unwrap();
    let id = descriptor.id().clone();
    let target = CapabilitySet::from_use_projection(
        CodeCatalogGeneration::new(1),
        use_generation.clone(),
        [CapabilityContribution::new(source, [descriptor]).unwrap()],
    )
    .unwrap();
    let mut txn = catalog.begin(target).unwrap();
    txn.stage_value(id, CapabilityValue::Tool(tool("inspect")))
        .unwrap();
    txn.prepare(CancellationToken::new())
        .await
        .unwrap()
        .validate()
        .unwrap()
        .commit()
        .unwrap();

    let lease = catalog.pin();
    assert_eq!(
        lease.projection().set().use_capability_generation(),
        Some(&use_generation)
    );
}
