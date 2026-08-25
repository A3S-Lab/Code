use super::*;

use crate::capability::{
    CapabilityId, CapabilityProjectionError, UiAsset, UiAssetKind, UiBinding, UiBindingSpec,
    UiDocument,
};

fn ui_binding(public_name: &str, version: &str) -> Arc<UiBinding> {
    Arc::new(
        UiBinding::new(UiBindingSpec {
            public_name: public_name.to_owned(),
            title: format!("Projected UI {version}"),
            description: format!("Exact document for {version}."),
            icon: "panel-top".to_owned(),
            order: 40,
            document: UiDocument::new(
                UiAsset::new(
                    UiAssetKind::Html,
                    format!("<!doctype html><main data-version=\"{version}\">{version}</main>"),
                )
                .unwrap(),
                [UiAsset::new(
                    UiAssetKind::Style,
                    format!("main {{ --generation: '{version}'; }}"),
                )
                .unwrap()],
                [UiAsset::new(
                    UiAssetKind::Script,
                    format!("globalThis.generation = '{version}';"),
                )
                .unwrap()],
            )
            .unwrap(),
        })
        .unwrap(),
    )
}

fn use_ui_set(
    code_generation: u64,
    upstream: UseCapabilityGeneration,
    public_name: &str,
    binding: &UiBinding,
) -> (Arc<CapabilitySet>, CapabilityId, CapabilityId) {
    let source = CapabilitySource::use_package(
        upstream.clone(),
        UsePackageGeneration::new(
            "acme/ui-package",
            "use/acme-ui-package",
            "ui-package",
            "1.0.0",
            upstream.generation(),
            digest('d'),
            digest('e'),
        )
        .unwrap(),
    )
    .unwrap();
    let backend = CapabilityDescriptor::new(
        &source,
        CapabilityKind::Tool,
        "ui-backend",
        "ui-backend",
        digest('b'),
        [],
    )
    .unwrap();
    let backend_id = backend.id().clone();
    let ui = CapabilityDescriptor::new(
        &source,
        CapabilityKind::Ui,
        "projected-ui",
        public_name,
        binding.surface_digest().clone(),
        [backend_id.clone()],
    )
    .unwrap();
    let ui_id = ui.id().clone();
    let contribution = CapabilityContribution::new(source, [backend, ui]).unwrap();
    let set = CapabilitySet::from_use_projection(
        CodeCatalogGeneration::new(code_generation),
        upstream,
        [contribution],
    )
    .unwrap();
    (set, backend_id, ui_id)
}

fn ui_batch(
    session: &AgentSession,
    upstream: UseCapabilityGeneration,
    binding: Arc<UiBinding>,
    acquired: &Arc<AtomicUsize>,
    dropped: &Arc<AtomicUsize>,
) -> SessionCapabilityBatch {
    let code_generation = session
        .capability_catalog_stamp()
        .generation()
        .checked_next()
        .unwrap()
        .get();
    let (set, backend_id, ui_id) = use_ui_set(
        code_generation,
        upstream.clone(),
        binding.public_name(),
        &binding,
    );
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, acquired, dropped))
            .unwrap();
    batch
        .stage_value(
            backend_id,
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "ui-backend".to_owned(),
                version: "ui-backend",
                executions: Arc::new(Mutex::new(Vec::new())),
            })),
        )
        .unwrap();
    batch
        .stage_value(ui_id, CapabilityValue::Ui(binding))
        .unwrap();
    batch
}

#[test]
fn projected_ui_public_contract_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<UiAsset>();
    assert_send_sync::<UiDocument>();
    assert_send_sync::<UiBinding>();
    assert_send_sync::<crate::ProjectedUiHandle>();
}

#[test]
fn ui_assets_reject_digest_drift_before_publication() {
    let error = UiAsset::new_verified(
        UiAssetKind::Html,
        "<!doctype html><main>drift</main>",
        digest('a'),
    )
    .expect_err("host bytes must match their reviewed asset digest");

    assert!(matches!(
        error,
        crate::capability::UiBindingError::AssetDigestMismatch { .. }
    ));
}

#[tokio::test]
async fn projected_ui_is_generation_frozen_across_atomic_cutover() {
    let session = test_session("projected-ui-cutover").await;
    let first_acquired = Arc::new(AtomicUsize::new(0));
    let first_dropped = Arc::new(AtomicUsize::new(0));
    let second_acquired = Arc::new(AtomicUsize::new(0));
    let second_dropped = Arc::new(AtomicUsize::new(0));
    let first_binding = ui_binding("projected-ui", "v1");
    let second_binding = ui_binding("projected-ui", "v2");

    session
        .apply_capability_batch(
            ui_batch(
                &session,
                use_generation(1, 'a'),
                Arc::clone(&first_binding),
                &first_acquired,
                &first_dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let old = session
        .projected_ui("projected-ui")
        .await
        .unwrap()
        .expect("generation one UI must be visible");
    assert_eq!(old.catalog_generation().get(), 1);
    assert_eq!(old.use_generation().unwrap().generation(), 1);
    assert_eq!(old.dependencies().len(), 1);
    assert_eq!(old.dependencies()[0].kind(), CapabilityKind::Tool);
    assert!(old.document().entry().content().contains("v1"));
    assert_eq!(old.surface_digest(), first_binding.surface_digest());
    assert_eq!(first_acquired.load(Ordering::SeqCst), 1);

    session
        .apply_capability_batch(
            ui_batch(
                &session,
                use_generation(2, 'c'),
                Arc::clone(&second_binding),
                &second_acquired,
                &second_dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();

    assert!(old.document().entry().content().contains("v1"));
    assert_eq!(old.surface_digest(), first_binding.surface_digest());
    assert_eq!(first_dropped.load(Ordering::SeqCst), 0);
    assert_eq!(second_acquired.load(Ordering::SeqCst), 0);
    old.close().await.unwrap();
    assert_eq!(first_dropped.load(Ordering::SeqCst), 1);

    let new = session
        .projected_ui("projected-ui")
        .await
        .unwrap()
        .expect("generation two UI must be visible");
    assert_eq!(new.catalog_generation().get(), 2);
    assert_eq!(new.use_generation().unwrap().generation(), 2);
    assert!(new.document().entry().content().contains("v2"));
    assert_eq!(new.surface_digest(), second_binding.surface_digest());
    assert_eq!(second_acquired.load(Ordering::SeqCst), 1);
    new.close().await.unwrap();
    assert_eq!(second_dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn projected_ui_name_is_bound_and_missing_lookup_acquires_no_lease() {
    let session = test_session("projected-ui-name-binding").await;
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let binding = ui_binding("different-ui", "v1");
    let upstream = use_generation(1, 'a');
    let (set, backend_id, ui_id) = use_ui_set(1, upstream.clone(), "projected-ui", &binding);
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            backend_id,
            CapabilityValue::Tool(Arc::new(VersionedTool {
                name: "ui-backend".to_owned(),
                version: "ui-backend",
                executions: Arc::new(Mutex::new(Vec::new())),
            })),
        )
        .unwrap();
    batch
        .stage_value(ui_id, CapabilityValue::Ui(binding))
        .unwrap();
    let before = session.capability_catalog_stamp();

    let error = session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .expect_err("the UI binding name must match its public descriptor name");
    assert!(matches!(
        error,
        CapabilityRuntimeError::Projection(CapabilityProjectionError::PublicNameMismatch {
            ref expected,
            ref actual,
            ..
        }) if expected == "projected-ui" && actual == "different-ui"
    ));
    assert_eq!(session.capability_catalog_stamp(), before);
    assert_eq!(acquired.load(Ordering::SeqCst), 0);
    assert_eq!(dropped.load(Ordering::SeqCst), 0);

    assert!(session.projected_ui("missing-ui").await.unwrap().is_none());
    assert_eq!(acquired.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn session_close_cancels_projected_ui_without_releasing_its_lease_early() {
    let session = test_session("projected-ui-session-close").await;
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    session
        .apply_capability_batch(
            ui_batch(
                &session,
                use_generation(1, 'a'),
                ui_binding("projected-ui", "v1"),
                &acquired,
                &dropped,
            ),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let handle = session.projected_ui("projected-ui").await.unwrap().unwrap();
    assert!(!handle.is_cancelled());
    assert_eq!(acquired.load(Ordering::SeqCst), 1);

    session.close().await;

    assert!(handle.is_cancelled());
    assert_eq!(dropped.load(Ordering::SeqCst), 0);
    handle.close().await.unwrap();
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
