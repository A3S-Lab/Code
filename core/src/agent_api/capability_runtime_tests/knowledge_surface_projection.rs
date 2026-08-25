use super::flow_projection::flow_binding;
use super::*;

use crate::capability::{
    CapabilityContribution, CapabilityDescriptor, CapabilityKind, CapabilitySet, CapabilitySource,
    CapabilityValue, CodeCatalogGeneration, KnowledgeSurfaceBinding, KnowledgeSurfaceBindingSpec,
    SessionCapabilityBatch, UsePackageGeneration,
};

fn knowledge_surface(
    public_name: &str,
    content: char,
    projection: char,
) -> Arc<KnowledgeSurfaceBinding> {
    Arc::new(
        KnowledgeSurfaceBinding::new(KnowledgeSurfaceBindingSpec {
            public_name: public_name.to_owned(),
            format_version: "0.2".to_owned(),
            content_digest: digest(content),
            projection_digests: vec![digest(projection)],
        })
        .unwrap(),
    )
}

#[tokio::test]
async fn multiple_knowledge_surfaces_publish_flow_readiness_without_becoming_cognitive_authority() {
    let session = test_session("knowledge-surface-readiness").await;
    let acquired = Arc::new(AtomicUsize::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));
    let upstream = use_generation(1, 'a');
    let source = CapabilitySource::use_package(
        upstream.clone(),
        UsePackageGeneration::new(
            "acme/research",
            "use/acme-research",
            "research",
            "1.0.0",
            1,
            digest('b'),
            digest('c'),
        )
        .unwrap(),
    )
    .unwrap();
    let domain = knowledge_surface("research:domain", 'd', 'e');
    let runbook = knowledge_surface("research:runbook", 'f', '1');
    let executions = Arc::new(Mutex::new(Vec::new()));
    let flow = flow_binding("research:review", "v1", &executions);
    let domain_descriptor = CapabilityDescriptor::new(
        &source,
        CapabilityKind::KnowledgeSurface,
        "domain",
        domain.public_name(),
        domain.surface_digest().clone(),
        [],
    )
    .unwrap();
    let domain_id = domain_descriptor.id().clone();
    let runbook_descriptor = CapabilityDescriptor::new(
        &source,
        CapabilityKind::KnowledgeSurface,
        "runbook",
        runbook.public_name(),
        runbook.surface_digest().clone(),
        [],
    )
    .unwrap();
    let runbook_id = runbook_descriptor.id().clone();
    let flow_descriptor = CapabilityDescriptor::new(
        &source,
        CapabilityKind::Flow,
        "review",
        flow.public_name(),
        digest('2'),
        [domain_id.clone()],
    )
    .unwrap();
    let flow_id = flow_descriptor.id().clone();
    let set = CapabilitySet::from_use_projection(
        CodeCatalogGeneration::new(1),
        upstream.clone(),
        [CapabilityContribution::new(
            source,
            [domain_descriptor, runbook_descriptor, flow_descriptor],
        )
        .unwrap()],
    )
    .unwrap();
    let mut batch =
        SessionCapabilityBatch::from_use_projection(set, provider(upstream, &acquired, &dropped))
            .unwrap();
    batch
        .stage_value(
            domain_id.clone(),
            CapabilityValue::KnowledgeSurface(Arc::clone(&domain)),
        )
        .unwrap()
        .stage_value(
            runbook_id.clone(),
            CapabilityValue::KnowledgeSurface(Arc::clone(&runbook)),
        )
        .unwrap()
        .stage_value(flow_id.clone(), CapabilityValue::Flow(Arc::clone(&flow)))
        .unwrap();
    session
        .apply_capability_batch(batch, CancellationToken::new())
        .await
        .unwrap();

    let run = session.admit_capability_run().await.unwrap();
    assert_eq!(acquired.load(Ordering::SeqCst), 1);
    assert!(std::ptr::eq(
        run.projection().knowledge_surface(&domain_id).unwrap(),
        domain.as_ref()
    ));
    assert!(std::ptr::eq(
        run.projection().knowledge_surface(&runbook_id).unwrap(),
        runbook.as_ref()
    ));
    assert!(std::ptr::eq(
        run.projection().flow(&flow_id).unwrap(),
        flow.as_ref()
    ));
    let waves = run.projection().readiness_plan().waves();
    assert_eq!(waves.len(), 2);
    assert!(waves[0].contains(&domain_id));
    assert!(waves[0].contains(&runbook_id));
    assert_eq!(waves[1], [flow_id]);
    assert!(run
        .projection()
        .iter()
        .all(|(id, _)| id.kind() != CapabilityKind::Knowledge));

    run.close().await.unwrap();
    drop(run);
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}
