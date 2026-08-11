mod r0_cross_project_support;

use std::path::Path;

use a3s_code_core::{
    AgentProtocolRunIdentityV1, CognitiveContextLimits, CognitiveKnowledgeBindingV1,
    CognitiveKnowledgeCitationV1, CognitivePackageBindingV1,
};
use r0_cross_project_support::{
    canonical_digest, read_fixture, verify_fixture_package, CodeBinding, Contract, ContractError,
    KnowledgeBinding,
};
use serde::Deserialize;

const FIXTURE_ROOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/agentic-ontology/r0-cross-project-v1"
);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilitySnapshotFixture {
    schema: String,
    package_id: String,
    package_version: String,
    lifecycle_generation: u64,
    generation_digest: String,
    surfaces: Vec<CapabilitySurfaceFixture>,
    snapshot_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilitySurfaceFixture {
    kind: String,
    id: String,
    format_version: String,
    content_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TaskResultFixture {
    schema: String,
    text: String,
    citation_digests: Vec<String>,
    result_digest: String,
}

#[test]
fn code_accepts_the_pinned_package_and_rejects_all_contract_drift() {
    let root = Path::new(FIXTURE_ROOT);
    verify_fixture_package(root).unwrap();
    Contract::parse(&read_fixture(root, "fixtures/r0-cross-project.valid.json")).unwrap();

    for (path, expected) in [
        (
            "fixtures/r0-cross-project.unknown-field.json",
            ContractError::Invalid,
        ),
        (
            "fixtures/r0-cross-project.drift-handoff-digest.json",
            ContractError::BindingMismatch,
        ),
        (
            "fixtures/r0-cross-project.drift-generation.json",
            ContractError::BindingMismatch,
        ),
        (
            "fixtures/r0-cross-project.drift-session.json",
            ContractError::DigestMismatch,
        ),
        (
            "fixtures/r0-cross-project.drift-cloud-task-proof.json",
            ContractError::BindingMismatch,
        ),
    ] {
        assert_eq!(
            Contract::parse(&read_fixture(root, path)).unwrap_err(),
            expected,
            "{path}"
        );
    }
}

#[test]
fn code_owns_and_validates_the_exact_agent_run_identity() {
    let root = Path::new(FIXTURE_ROOT);
    let contract =
        Contract::parse(&read_fixture(root, "fixtures/r0-cross-project.valid.json")).unwrap();
    let code: CodeBinding = read_json(root, "fixtures/code-session-binding.json");
    assert_eq!(code, contract.code);
    let identity: AgentProtocolRunIdentityV1 = read_json(root, "fixtures/code-run-identity.json");
    identity.validate().unwrap();
    assert_eq!(identity.protocol, code.agent_protocol);
    assert_eq!(identity.agent_release_identity, code.agent_release_identity);
    assert_eq!(identity.session_id, code.session_id);
    assert_eq!(identity.run_id, code.run_id);

    let mut another_run = identity.clone();
    another_run.run_id = "run-execution-018f4f86-attempt-2".into();
    another_run.validate().unwrap();
    assert_ne!(another_run.run_id, code.run_id);
    assert_eq!(
        Contract::parse(&read_fixture(
            root,
            "fixtures/r0-cross-project.drift-session.json"
        ))
        .unwrap_err(),
        ContractError::DigestMismatch
    );
}

#[test]
fn code_binds_one_use_generation_capability_and_cited_result() {
    let root = Path::new(FIXTURE_ROOT);
    let contract =
        Contract::parse(&read_fixture(root, "fixtures/r0-cross-project.valid.json")).unwrap();
    let knowledge: KnowledgeBinding = read_json(root, "fixtures/knowledge-lease-binding.json");
    assert_eq!(knowledge, contract.knowledge);
    let runtime_knowledge: CognitiveKnowledgeBindingV1 =
        read_json(root, "fixtures/knowledge-lease-binding.json");
    runtime_knowledge.validate().unwrap();
    let capability: CapabilitySnapshotFixture =
        read_json(root, "fixtures/capability-snapshot.json");
    assert_eq!(capability.schema, "a3s.use.capability-snapshot.v1");
    assert_eq!(capability.package_id, contract.code.package_id);
    assert_eq!(capability.package_version, contract.code.package_version);
    assert_eq!(
        capability.lifecycle_generation,
        contract.code.lifecycle_generation
    );
    assert_eq!(
        capability.generation_digest,
        contract.code.generation_digest
    );
    assert_eq!(capability.surfaces.len(), 1);
    let surface = &capability.surfaces[0];
    assert_eq!(surface.kind, "okf");
    assert_eq!(surface.id, knowledge.surface_id);
    assert_eq!(surface.format_version, knowledge.format_version);
    assert_eq!(surface.content_digest, knowledge.content_digest);
    assert_eq!(
        capability.snapshot_digest,
        canonical_digest(
            "a3s.use.capability-snapshot.v1",
            &(
                capability.package_id.as_str(),
                capability.package_version.as_str(),
                capability.lifecycle_generation,
                &capability.generation_digest,
                surface.id.as_str(),
                surface.format_version.as_str(),
                surface.content_digest.as_str(),
            ),
        )
    );
    assert_eq!(
        capability.snapshot_digest,
        contract.code.capability_snapshot_digest
    );
    let runtime_binding = CognitivePackageBindingV1::new(
        capability.package_id.clone(),
        capability.package_version.clone(),
        capability.lifecycle_generation,
        capability.generation_digest.clone(),
        capability.snapshot_digest.clone(),
        runtime_knowledge,
        CognitiveContextLimits::default(),
    )
    .unwrap();

    let citation: CognitiveKnowledgeCitationV1 =
        read_json(root, "fixtures/knowledge-citation.json");
    citation.validate_for(&runtime_binding).unwrap();
    assert_eq!(citation.schema, knowledge.citation_schema);
    assert_eq!(citation.package_id, contract.code.package_id);
    assert_eq!(citation.package_version, contract.code.package_version);
    assert_eq!(
        citation.lifecycle_generation,
        contract.code.lifecycle_generation
    );
    assert_eq!(citation.generation_digest, contract.code.generation_digest);
    assert_eq!(citation.surface_id, knowledge.surface_id);
    assert_eq!(citation.content_digest, knowledge.content_digest);
    assert!(!citation.document_path.starts_with('/'));
    assert!(!citation.document_path.contains(".."));
    assert!(!citation.heading.trim().is_empty());
    assert!(!citation.evidence_ids.is_empty());
    assert_eq!(
        citation.citation_digest,
        canonical_digest(
            "a3s.use.okf-knowledge-citation.v1",
            &(
                citation.package_id.as_str(),
                citation.package_version.as_str(),
                citation.lifecycle_generation,
                &citation.generation_digest,
                citation.surface_id.as_str(),
                citation.content_digest.as_str(),
                citation.document_path.as_str(),
                citation.heading.as_str(),
                &citation.evidence_ids,
            ),
        )
    );

    let result: TaskResultFixture = read_json(root, "fixtures/code-task-result.json");
    assert_eq!(result.schema, "a3s.code.agent-task-result.v1");
    assert_eq!(result.citation_digests, [citation.citation_digest]);
    assert_eq!(
        result.result_digest,
        canonical_digest(
            "a3s.code.agent-task-result.v1",
            &(
                result.schema.as_str(),
                result.text.as_str(),
                &result.citation_digests,
            ),
        )
    );
    assert_eq!(result.result_digest, contract.code.result_digest);
}

fn read_json<T: for<'de> Deserialize<'de>>(root: &Path, path: &str) -> T {
    serde_json::from_slice(&read_fixture(root, path)).unwrap()
}
