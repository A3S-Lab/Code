#![cfg(test)]

use super::*;

fn digest() -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", "ab".repeat(32))).unwrap()
}

fn selection(id: &str) -> UseCapabilityGeneration {
    UseCapabilityGeneration::selection(id, 1, digest()).unwrap()
}

fn source(id: &str) -> CapabilitySource {
    CapabilitySource::use_reconcile(
        selection(id),
        ReconciledPackageGeneration::new("acme/echo", 2, digest()).unwrap(),
    )
    .unwrap()
}

#[test]
fn selection_serializes_exact_identity_without_registry_or_manifest_evidence() {
    let source = source("studio-installed-plugins");
    let value = serde_json::to_value(&source).unwrap();
    assert_eq!(
        value["evidence"]["capability"]["selectionId"],
        "studio-installed-plugins"
    );
    assert_eq!(value["evidence"]["publication"]["generation"], 2);
    assert!(value["evidence"]["capability"]
        .get("registryRevision")
        .is_none());
    assert!(source.use_package_generation().is_none());
    assert_eq!(
        source.reconciled_package_generation().unwrap().package_id(),
        "acme/echo"
    );
}

#[test]
fn different_selection_id_cannot_enter_one_capability_projection() {
    let source = source("other");
    let descriptor =
        CapabilityDescriptor::new(&source, CapabilityKind::Tool, "echo", "echo", digest(), [])
            .unwrap();
    let contribution = CapabilityContribution::new(source, [descriptor]).unwrap();
    assert!(matches!(
        CapabilitySet::from_use_projection(
            CodeCatalogGeneration::new(1),
            selection("workspace"),
            [contribution]
        ),
        Err(CapabilitySetError::MixedUseGeneration {
            origin_mismatch: true,
            ..
        })
    ));
}

#[test]
fn reconcile_source_requires_a_selection_and_cannot_claim_registry_package_evidence() {
    let registry = UseCapabilityGeneration::new(1, digest(), digest());
    let package = ReconciledPackageGeneration::new("acme/echo", 2, digest()).unwrap();
    assert!(CapabilitySource::use_reconcile(registry, package).is_err());
    let old_package = UsePackageGeneration::new(
        "acme/echo",
        "acme/echo",
        "echo",
        "1.0.0",
        2,
        digest(),
        digest(),
    )
    .unwrap();
    assert!(CapabilitySource::use_package(selection("workspace"), old_package).is_err());
    assert!(UseCapabilityGeneration::selection("../escape", 1, digest()).is_err());
    assert!(UseCapabilityGeneration::selection("workspace", 0, digest()).is_err());
}

#[test]
fn persisted_run_binding_checks_selection_origin_and_rejects_mixed_evidence() {
    let set = CapabilitySet::from_use_projection(
        CodeCatalogGeneration::new(1),
        selection("workspace"),
        [],
    )
    .unwrap();
    let ceiling = CapabilityCeiling::all(
        &set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        CapabilityExecutionCeiling::new(8, 2, None, None, None).unwrap(),
    )
    .unwrap();
    let binding = RunCapabilityBindingV1::from_set_and_ceiling(&set, &ceiling).unwrap();
    let mut value = serde_json::to_value(&binding).unwrap();
    assert!(value["useGeneration"].get("registryRevision").is_none());
    let restored: RunCapabilityBindingV1 = serde_json::from_value(value.clone()).unwrap();
    restored.ensure_matches(&set, &ceiling).unwrap();
    value["useGeneration"]["registryRevision"] = digest().to_string().into();
    assert!(serde_json::from_value::<RunCapabilityBindingV1>(value)
        .unwrap()
        .validate()
        .is_err());
    let mut changed = serde_json::to_value(binding).unwrap();
    changed["useGeneration"]["selectionId"] = "other".into();
    assert!(serde_json::from_value::<RunCapabilityBindingV1>(changed)
        .unwrap()
        .ensure_matches(&set, &ceiling)
        .is_err());
}

#[tokio::test]
async fn native_provider_without_spatial_binding_cannot_admit_even_a_directly_published_catalog() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    struct UnboundProvider {
        generation: UseCapabilityGeneration,
        acquired: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl UseGenerationLeaseProvider for UnboundProvider {
        fn use_generation(&self) -> &UseCapabilityGeneration {
            &self.generation
        }
        async fn acquire(
            &self,
            _cancellation: CancellationToken,
        ) -> Result<Box<dyn RetainedUseGeneration>, UseGenerationLeaseError> {
            self.acquired.fetch_add(1, Ordering::SeqCst);
            Err(UseGenerationLeaseError::new(
                "unbound provider must not acquire",
            ))
        }
    }
    let provider = Arc::new(UnboundProvider {
        generation: selection("workspace"),
        acquired: AtomicUsize::new(0),
    });
    let set = CapabilitySet::from_use_projection(
        CodeCatalogGeneration::new(1),
        provider.generation.clone(),
        [],
    )
    .unwrap();
    let batch = SessionCapabilityBatch::from_use_projection(set.clone(), provider.clone()).unwrap();
    let workspace = tempfile::tempdir().unwrap();
    assert!(matches!(
        batch.validate_workspace(workspace.path()),
        Err(CapabilityRuntimeError::WorkspaceBinding { .. })
    ));
    let catalog = CapabilityCatalog::new(
        CapabilityProjection::new(CapabilitySet::empty().unwrap(), []).unwrap(),
    );
    batch
        .prepare(&catalog, CancellationToken::new())
        .await
        .unwrap()
        .commit()
        .unwrap();
    let ceiling = CapabilityCeiling::all(
        &set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        CapabilityExecutionCeiling::new(8, 2, None, None, None).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        SessionCapabilityRun::admit(
            catalog.pin(),
            "session",
            "run",
            ceiling,
            CancellationToken::new(),
            workspace.path()
        )
        .await,
        Err(CapabilityRuntimeError::WorkspaceBinding { .. })
    ));
    assert_eq!(provider.acquired.load(Ordering::SeqCst), 0);
}
