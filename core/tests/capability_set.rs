use std::sync::Arc;

use a3s_code_core::capability::{
    CapabilityContribution, CapabilityDescriptor, CapabilityId, CapabilityKind, CapabilitySet,
    CapabilitySetError, CapabilitySource, CodeCatalogGeneration, Sha256Digest,
    UseCapabilityGeneration, UsePackageGeneration, MAX_CAPABILITY_DEPENDENCIES,
    MAX_CAPABILITY_DEPENDENCY_EDGES,
};

fn digest(byte: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).unwrap()
}

fn host_source() -> CapabilitySource {
    let source = CapabilitySource::host("a3s-code", digest('a')).unwrap();
    assert_eq!(source.revision(), Some(&digest('a')));
    source
}

fn use_source() -> CapabilitySource {
    CapabilitySource::use_package(
        UseCapabilityGeneration::new(7, digest('b'), digest('2')),
        UsePackageGeneration::new(
            "acme/guide",
            "use/acme-guide",
            "guide",
            "1.0.0",
            11,
            digest('c'),
            digest('d'),
        )
        .unwrap(),
    )
    .unwrap()
}

fn descriptor(
    source: &CapabilitySource,
    kind: CapabilityKind,
    local_id: &str,
    public_name: &str,
    surface_digest: Sha256Digest,
    dependencies: impl IntoIterator<Item = CapabilityId>,
) -> CapabilityDescriptor {
    CapabilityDescriptor::new(
        source,
        kind,
        local_id,
        public_name,
        surface_digest,
        dependencies,
    )
    .unwrap()
}

#[test]
fn typed_identities_reject_ambiguous_or_unbounded_input() {
    assert!(matches!(
        Sha256Digest::new("sha256:ABC"),
        Err(CapabilitySetError::InvalidDigest { .. })
    ));
    assert!(matches!(
        CapabilitySource::host("A3S Code", digest('a')),
        Err(CapabilitySetError::InvalidIdentifier { .. })
    ));
    assert!(matches!(
        UsePackageGeneration::new(
            "acme/guide",
            "use/acme-guide",
            "guide",
            "1.0.0",
            0,
            digest('c'),
            digest('d'),
        ),
        Err(CapabilitySetError::InvalidGeneration { .. })
    ));

    let source = host_source();
    assert!(matches!(
        CapabilityId::new(&source, CapabilityKind::Tool, "../read"),
        Err(CapabilitySetError::InvalidIdentifier { .. })
    ));

    let dependencies = (0..=MAX_CAPABILITY_DEPENDENCIES)
        .map(|index| {
            CapabilityId::new(
                &source,
                CapabilityKind::Tool,
                format!("dependency-{index:03}"),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        CapabilityDescriptor::new(
            &source,
            CapabilityKind::Tool,
            "read",
            "read",
            digest('e'),
            dependencies,
        ),
        Err(CapabilitySetError::BoundExceeded { .. })
    ));

    let self_id = CapabilityId::new(&source, CapabilityKind::Tool, "self").unwrap();
    assert!(matches!(
        CapabilityDescriptor::new(
            &source,
            CapabilityKind::Tool,
            "self",
            "self",
            digest('f'),
            [self_id],
        ),
        Err(CapabilitySetError::SelfDependency { .. })
    ));
}

#[test]
fn complete_source_batches_own_every_descriptor() {
    let host = host_source();
    let external = use_source();
    assert_eq!(
        external.use_package_generation().unwrap().component_id(),
        "use/acme-guide"
    );
    let read = descriptor(&host, CapabilityKind::Tool, "read", "read", digest('e'), []);

    assert!(matches!(
        CapabilityContribution::new(external, vec![read]),
        Err(CapabilitySetError::SourceMismatch { .. })
    ));
    assert!(matches!(
        CapabilityContribution::new(host, Vec::new()),
        Err(CapabilitySetError::EmptyContribution { .. })
    ));
}

#[test]
fn set_order_and_digest_are_independent_of_input_order() {
    let host = host_source();
    let external = use_source();
    let read = descriptor(&host, CapabilityKind::Tool, "read", "read", digest('e'), []);
    let search = descriptor(
        &host,
        CapabilityKind::Tool,
        "search",
        "search",
        digest('f'),
        [],
    );
    let guide = descriptor(
        &external,
        CapabilityKind::Skill,
        "guide",
        "guide",
        digest('1'),
        [search.id().clone(), read.id().clone()],
    );

    let forward = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(3),
        vec![
            CapabilityContribution::new(host.clone(), vec![read.clone(), search.clone()]).unwrap(),
            CapabilityContribution::new(external.clone(), vec![guide.clone()]).unwrap(),
        ],
    )
    .unwrap();
    let reverse = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(3),
        vec![
            CapabilityContribution::new(external.clone(), vec![guide.clone()]).unwrap(),
            CapabilityContribution::new(host.clone(), vec![search.clone(), read.clone()]).unwrap(),
        ],
    )
    .unwrap();

    assert_eq!(forward.digest(), reverse.digest());
    assert_eq!(
        forward
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>(),
        reverse
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>()
    );
    assert_eq!(forward.len(), 3);
    assert_eq!(forward.source_count(), 2);
    assert_eq!(
        forward
            .use_capability_generation()
            .unwrap()
            .registry_revision(),
        &digest('2')
    );
    assert_eq!(
        forward.digest().as_str(),
        "sha256:1a9ce41f86488715aa29e1f953aed4d8500aa2bf809d8bc25514dad7a8965119"
    );
    assert_eq!(
        forward.get(guide_id(&forward)).unwrap().public_name(),
        "guide"
    );

    let next_generation = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(4),
        vec![
            CapabilityContribution::new(host, vec![read, search]).unwrap(),
            CapabilityContribution::new(external, vec![guide]).unwrap(),
        ],
    )
    .unwrap();
    assert_ne!(forward.digest(), next_generation.digest());
}

fn guide_id(set: &CapabilitySet) -> &CapabilityId {
    set.iter()
        .find_map(|(id, descriptor)| (descriptor.public_name() == "guide").then_some(id))
        .unwrap()
}

#[test]
fn immutable_sets_are_send_sync_and_pinned_by_arc() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<CapabilitySet>();
    let set = CapabilitySet::empty().unwrap();
    let reader = Arc::clone(&set);
    assert!(Arc::ptr_eq(&set, &reader));
    assert!(reader.is_empty());
    assert_eq!(reader.schema(), "a3s.code.capability-set.v1");
    assert_eq!(reader.generation(), CodeCatalogGeneration::INITIAL);
    assert!(reader.digest().as_str().starts_with("sha256:"));
}

#[test]
fn empty_product_projection_retains_its_complete_use_cursor_identity() {
    let upstream = UseCapabilityGeneration::new(0, digest('b'), digest('2'));
    let projected = CapabilitySet::from_use_projection(
        CodeCatalogGeneration::new(1),
        upstream.clone(),
        Vec::<CapabilityContribution>::new(),
    )
    .unwrap();
    let unbound = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(1),
        Vec::<CapabilityContribution>::new(),
    )
    .unwrap();

    assert!(projected.is_empty());
    assert_eq!(projected.use_capability_generation(), Some(&upstream));
    assert_ne!(projected.digest(), unbound.digest());
}

#[test]
fn public_name_conflicts_fail_closed() {
    let host = host_source();
    let external = use_source();
    let host_read = descriptor(&host, CapabilityKind::Tool, "read", "read", digest('e'), []);
    let external_read = descriptor(
        &external,
        CapabilityKind::Tool,
        "plugin-read",
        "read",
        digest('f'),
        [],
    );

    let error = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(1),
        vec![
            CapabilityContribution::new(host, vec![host_read]).unwrap(),
            CapabilityContribution::new(external, vec![external_read]).unwrap(),
        ],
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CapabilitySetError::PublicNameConflict { .. }
    ));
}

#[test]
fn unresolved_dependencies_and_duplicate_sources_never_publish() {
    let host = host_source();
    let missing = CapabilityId::new(&use_source(), CapabilityKind::Tool, "missing").unwrap();
    let read = descriptor(
        &host,
        CapabilityKind::Tool,
        "read",
        "read",
        digest('e'),
        [missing],
    );
    let contribution = CapabilityContribution::new(host.clone(), vec![read]).unwrap();
    assert!(matches!(
        CapabilitySet::from_contributions(
            CodeCatalogGeneration::new(1),
            vec![contribution.clone()]
        ),
        Err(CapabilitySetError::MissingDependency { .. })
    ));
    assert!(matches!(
        CapabilitySet::from_contributions(
            CodeCatalogGeneration::new(1),
            vec![contribution.clone(), contribution]
        ),
        Err(CapabilitySetError::DuplicateSource { .. })
    ));
}

#[test]
fn package_contributions_cannot_mix_use_cursor_generations() {
    let first = use_source();
    let second = CapabilitySource::use_package(
        UseCapabilityGeneration::new(7, digest('9'), digest('2')),
        UsePackageGeneration::new(
            "acme/search",
            "use/acme-search",
            "search",
            "1.0.0",
            12,
            digest('3'),
            digest('4'),
        )
        .unwrap(),
    )
    .unwrap();
    let guide = descriptor(
        &first,
        CapabilityKind::Skill,
        "guide",
        "guide",
        digest('5'),
        [],
    );
    let search = descriptor(
        &second,
        CapabilityKind::Skill,
        "search",
        "search",
        digest('6'),
        [],
    );

    let error = CapabilitySet::from_contributions(
        CodeCatalogGeneration::new(1),
        vec![
            CapabilityContribution::new(first, vec![guide]).unwrap(),
            CapabilityContribution::new(second, vec![search]).unwrap(),
        ],
    )
    .unwrap_err();
    assert!(matches!(
        error,
        CapabilitySetError::MixedUseGeneration {
            expected_generation: 7,
            actual_generation: 7,
            revision_mismatch: true,
            registry_revision_mismatch: false,
        }
    ));
}

#[test]
fn aggregate_dependency_edges_are_bounded_before_digesting() {
    let source = host_source();
    let dependency_count = MAX_CAPABILITY_DEPENDENCIES;
    let dependencies = (0..dependency_count)
        .map(|index| {
            descriptor(
                &source,
                CapabilityKind::Tool,
                &format!("leaf-{index:03}"),
                &format!("leaf-{index:03}"),
                digest('7'),
                [],
            )
        })
        .collect::<Vec<_>>();
    let dependency_ids = dependencies
        .iter()
        .map(|descriptor| descriptor.id().clone())
        .collect::<Vec<_>>();
    let consumer_count = MAX_CAPABILITY_DEPENDENCY_EDGES / dependency_count + 1;
    let consumers = (0..consumer_count).map(|index| {
        descriptor(
            &source,
            CapabilityKind::Skill,
            &format!("consumer-{index:03}"),
            &format!("consumer-{index:03}"),
            digest('8'),
            dependency_ids.iter().cloned(),
        )
    });
    let contribution =
        CapabilityContribution::new(source.clone(), dependencies.into_iter().chain(consumers))
            .unwrap();

    assert!(matches!(
        CapabilitySet::from_contributions(CodeCatalogGeneration::new(1), [contribution]),
        Err(CapabilitySetError::BoundExceeded {
            field: "dependency_edges",
            ..
        })
    ));
}
