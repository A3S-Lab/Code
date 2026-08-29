use a3s_code_core::capability::{
    CapabilityCeiling, CapabilityExecutionCeiling, CapabilitySet, GovernanceCapabilityCeiling,
    RunCapabilityBindingV1, WorkspaceCapabilityCeiling,
};
use a3s_code_core::cognitive_context::{
    CognitiveContextLimits, CognitiveKnowledgeBindingV1, CognitivePackageBindingV1,
};
use a3s_code_core::loop_checkpoint::{
    LoopCheckpoint, LoopConvergenceState, LOOP_CHECKPOINT_SCHEMA_VERSION,
};
use a3s_code_core::run::{RunRecord, RunSnapshot, RunStatus};
use a3s_code_core::session_checkpoint::{
    SessionCheckpointExportV1, SESSION_CHECKPOINT_FORMAT_V1,
    SESSION_CHECKPOINT_LOGICAL_RESUME_SEMANTICS_V1, SESSION_CHECKPOINT_MEDIA_TYPE_V1,
};
use a3s_code_core::store::{
    ContextUsage, LlmConfigData, SessionConfig, SessionData, SessionSnapshotV1, SessionState,
};
use a3s_code_core::tools::ArtifactStore;
use a3s_code_core::{
    llm::Message, llm::TokenUsage, DurableMemoryBindingV1, DurableMemoryRecallPolicy,
    DurableMemorySession,
};
use a3s_memory::repository::{InMemoryRepository, MemoryNamespace};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const CANONICAL_DIGEST_PREFIX: &[u8] = b"agentic-ontology-canonical-v1\0";
const CAPABILITY_SNAPSHOT_DIGEST_DOMAIN: &str = "a3s.use.capability-snapshot.v1";

fn session_data() -> SessionData {
    SessionData {
        id: "session-checkpoint-1".into(),
        config: SessionConfig {
            name: "checkpoint fixture".into(),
            workspace: "/workspace".into(),
            max_context_length: 128_000,
            ..SessionConfig::default()
        },
        state: SessionState::Active,
        messages: vec![Message::user("retain exact state")],
        context_usage: ContextUsage::default(),
        total_usage: TokenUsage::default(),
        total_cost: 0.0,
        model_name: Some("deepseek-chat".into()),
        cost_records: Vec::new(),
        tool_names: vec!["read".into()],
        thinking_enabled: false,
        thinking_budget: None,
        created_at: 1_700_000_000,
        updated_at: 1_700_000_001,
        llm_config: None,
        tasks: Vec::new(),
        parent_id: None,
        tenant_id: None,
        principal: None,
        agent_template_id: None,
        correlation_id: None,
        durable_memory_binding: None,
        cognitive_package_binding: None,
        immutable_content_adapter_binding: None,
    }
}

fn snapshot(with_run: bool) -> SessionSnapshotV1 {
    let records = if with_run {
        vec![RunRecord {
            snapshot: RunSnapshot {
                id: "source-run".into(),
                session_id: "session-checkpoint-1".into(),
                status: RunStatus::Executing,
                prompt: "continue".into(),
                cognitive_package_binding: None,
                capability_binding: None,
                created_at_ms: 1_700_000_000_000,
                updated_at_ms: 1_700_000_001_000,
                result_text: None,
                error: None,
                event_count: 0,
                workspace_change_set: None,
            },
            events: Vec::new(),
        }]
    } else {
        Vec::new()
    };
    SessionSnapshotV1::new(
        session_data(),
        &ArtifactStore::new(),
        Vec::new(),
        records,
        Vec::new(),
        Vec::new(),
    )
}

fn logical_resume() -> LoopCheckpoint {
    LoopCheckpoint {
        schema_version: LOOP_CHECKPOINT_SCHEMA_VERSION,
        run_id: "source-run".into(),
        session_id: "session-checkpoint-1".into(),
        capability_binding: None,
        turn: 3,
        messages: vec![Message::user("retain exact state")],
        total_usage: TokenUsage {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        tool_calls_count: 2,
        verification_reports: Vec::new(),
        convergence: LoopConvergenceState::default(),
        checkpoint_ms: 1_700_000_001_500,
    }
}

fn cognitive_binding(generation: u64, digest_byte: char) -> CognitivePackageBindingV1 {
    let package_id = "acme/checkpoint-knowledge";
    let package_version = format!("1.0.{generation}");
    let generation_digest = format!("sha256:{}", digest_byte.to_string().repeat(64));
    let content_byte = char::from_u32(u32::from(digest_byte) + 1).unwrap();
    let knowledge = CognitiveKnowledgeBindingV1::new(
        "checkpoint-knowledge",
        "0.2",
        format!("sha256:{}", content_byte.to_string().repeat(64)),
        generation,
        generation_digest.clone(),
    )
    .unwrap();
    let encoded = serde_json::to_vec(&(
        package_id,
        package_version.as_str(),
        generation,
        generation_digest.as_str(),
        knowledge.surface_id.as_str(),
        knowledge.format_version.as_str(),
        knowledge.content_digest.as_str(),
    ))
    .unwrap();
    let mut hasher = Sha256::new();
    hasher.update(CANONICAL_DIGEST_PREFIX);
    hasher.update((CAPABILITY_SNAPSHOT_DIGEST_DOMAIN.len() as u64).to_be_bytes());
    hasher.update(CAPABILITY_SNAPSHOT_DIGEST_DOMAIN.as_bytes());
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    let capability_snapshot_digest = format!("sha256:{:x}", hasher.finalize());
    CognitivePackageBindingV1::new(
        package_id,
        package_version,
        generation,
        generation_digest,
        capability_snapshot_digest,
        knowledge,
        CognitiveContextLimits::default(),
    )
    .unwrap()
}

fn capability_binding(max_tool_rounds: usize) -> RunCapabilityBindingV1 {
    let set = CapabilitySet::empty().unwrap();
    let ceiling = CapabilityCeiling::all(
        &set,
        WorkspaceCapabilityCeiling::all(),
        GovernanceCapabilityCeiling::none_required(),
        CapabilityExecutionCeiling::new(max_tool_rounds, 4, None, None, None).unwrap(),
    )
    .unwrap();
    RunCapabilityBindingV1::from_set_and_ceiling(&set, &ceiling).unwrap()
}

#[test]
fn semantic_and_logical_state_form_one_exact_portable_checkpoint() {
    let export = SessionCheckpointExportV1::new(snapshot(true), Some(logical_resume())).unwrap();
    let descriptor = export.descriptor();

    assert_eq!(descriptor.format, SESSION_CHECKPOINT_FORMAT_V1);
    assert_eq!(descriptor.media_type, SESSION_CHECKPOINT_MEDIA_TYPE_V1);
    assert_eq!(descriptor.size_bytes, 2062);
    assert_eq!(
        descriptor.content_digest,
        "sha256:9e66c6ab9896727d3094ec9278974001e80a70c25b199b4706efd7e1c6520302"
    );
    assert_eq!(
        descriptor.descriptor_digest,
        "sha256:6d01924f9b243433fde22496958090f8017d87695af7dd3646c306675b97e8a4"
    );
    assert_eq!(
        descriptor.snapshot.evidence_digest,
        "sha256:0388b0c594c04f00f13b3a6954754385b847f0f68e0d72382e7edf89903653f0"
    );
    let logical = descriptor.logical_resume.as_ref().unwrap();
    assert_eq!(
        logical.resume_semantics,
        SESSION_CHECKPOINT_LOGICAL_RESUME_SEMANTICS_V1
    );
    assert_eq!(logical.source_run_id, "source-run");
    assert_eq!(logical.completed_tool_rounds, 3);
    assert_eq!(
        logical.evidence_digest,
        "sha256:76b51a55eecc62343df274dba53d051bfff4961541d36214458d6b1c5f9ad5ee"
    );
    let descriptor_json = serde_json::to_value(descriptor).unwrap();
    for cloud_owned in [
        "checkpoint_id",
        "content_uri",
        "provider",
        "retention",
        "approval",
        "parent_checkpoint_id",
        "fork_id",
    ] {
        assert!(descriptor_json.get(cloud_owned).is_none(), "{cloud_owned}");
    }
    descriptor.snapshot.validate_for(&snapshot(true)).unwrap();
    logical.validate_for(&logical_resume()).unwrap();

    let payload = export.open().unwrap();
    assert_eq!(payload.snapshot.session.id, "session-checkpoint-1");
    assert_eq!(payload.logical_resume.unwrap().run_id, "source-run");
}

#[test]
fn semantic_only_checkpoint_is_supported_without_inventing_fork_identity() {
    let export = SessionCheckpointExportV1::new(snapshot(false), None).unwrap();
    assert!(export.descriptor().logical_resume.is_none());
    assert!(export.open().unwrap().logical_resume.is_none());
}

#[test]
fn content_or_descriptor_drift_fails_closed() {
    let export = SessionCheckpointExportV1::new(snapshot(true), Some(logical_resume())).unwrap();
    let (descriptor, mut content) = export.into_parts();
    let last = content.last_mut().unwrap();
    *last ^= 1;
    assert!(SessionCheckpointExportV1::from_parts(descriptor.clone(), content).is_err());

    let mut drifted = descriptor;
    drifted
        .logical_resume
        .as_mut()
        .unwrap()
        .completed_tool_rounds += 1;
    assert!(SessionCheckpointExportV1::from_parts(
        drifted,
        SessionCheckpointExportV1::new(snapshot(true), Some(logical_resume()))
            .unwrap()
            .into_content(),
    )
    .is_err());

    let export = SessionCheckpointExportV1::new(snapshot(true), Some(logical_resume())).unwrap();
    let mut later = logical_resume();
    later.turn += 1;
    assert!(export
        .descriptor()
        .logical_resume
        .as_ref()
        .unwrap()
        .validate_for(&later)
        .is_err());
}

#[test]
fn logical_resume_must_belong_to_a_run_in_the_same_snapshot() {
    let missing = SessionCheckpointExportV1::new(snapshot(false), Some(logical_resume()))
        .unwrap_err()
        .to_string();
    assert!(missing.contains("source run"), "{missing}");

    let mut foreign = logical_resume();
    foreign.session_id = "another-session".into();
    let foreign = SessionCheckpointExportV1::new(snapshot(true), Some(foreign))
        .unwrap_err()
        .to_string();
    assert!(foreign.contains("ownership"), "{foreign}");
}

#[test]
fn logical_resume_rejects_a_session_bound_to_another_cognitive_generation() {
    let mut mixed = snapshot(true);
    mixed.run_records[0].snapshot.cognitive_package_binding = Some(cognitive_binding(1, 'a'));
    mixed.session.cognitive_package_binding = Some(cognitive_binding(2, 'c'));

    let error = SessionCheckpointExportV1::new(mixed, Some(logical_resume()))
        .unwrap_err()
        .to_string();
    assert!(error.contains("cognitive"), "{error}");
}

#[test]
fn logical_resume_rejects_a_source_run_bound_to_another_capability_generation() {
    let mut mixed = snapshot(true);
    mixed.run_records[0].snapshot.capability_binding = Some(capability_binding(8));
    let mut logical = logical_resume();
    logical.capability_binding = Some(capability_binding(7));

    let error = SessionCheckpointExportV1::new(mixed, Some(logical))
        .unwrap_err()
        .to_string();
    assert!(error.contains("capability generations"), "{error}");
}

#[test]
fn portable_checkpoint_rejects_future_or_non_boundary_resume_state() {
    let mut future_snapshot = snapshot(false);
    future_snapshot.schema_version += 1;
    assert!(SessionCheckpointExportV1::new(future_snapshot, None).is_err());

    let mut future_resume = logical_resume();
    future_resume.schema_version += 1;
    assert!(SessionCheckpointExportV1::new(snapshot(true), Some(future_resume)).is_err());

    let mut no_boundary = logical_resume();
    no_boundary.turn = 0;
    assert!(SessionCheckpointExportV1::new(snapshot(true), Some(no_boundary)).is_err());
}

#[test]
fn portable_checkpoint_rejects_invalid_durable_memory_binding() {
    let namespace = MemoryNamespace::try_new("tenant", "principal", "workspace").unwrap();
    let binding = DurableMemorySession::active_recall(
        Arc::new(InMemoryRepository::new()),
        namespace,
        DurableMemoryRecallPolicy::try_new(5, 0.25).unwrap(),
    )
    .binding();
    let mut encoded = serde_json::to_value(binding).unwrap();
    encoded["schemaVersion"] = serde_json::json!(3);
    let invalid: DurableMemoryBindingV1 = serde_json::from_value(encoded).unwrap();
    let mut invalid_snapshot = snapshot(false);
    invalid_snapshot.session.durable_memory_binding = Some(invalid);

    let error = SessionCheckpointExportV1::new(invalid_snapshot, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("durable-memory binding"), "{error}");

    let namespace = MemoryNamespace::try_new("tenant", "principal", "workspace").unwrap();
    let binding = DurableMemorySession::active_recall(
        Arc::new(InMemoryRepository::new()),
        namespace,
        DurableMemoryRecallPolicy::try_new(5, 0.25).unwrap(),
    )
    .binding();
    let mut encoded = serde_json::to_value(binding).unwrap();
    encoded["mode"] = serde_json::json!("shadow_candidates");
    let invalid: DurableMemoryBindingV1 = serde_json::from_value(encoded).unwrap();
    let mut invalid_snapshot = snapshot(false);
    invalid_snapshot.session.durable_memory_binding = Some(invalid);

    let error = SessionCheckpointExportV1::new(invalid_snapshot, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("recall policy"), "{error}");

    let namespace = MemoryNamespace::try_new("tenant", "principal", "workspace").unwrap();
    let binding = DurableMemorySession::active_recall(
        Arc::new(InMemoryRepository::new()),
        namespace,
        DurableMemoryRecallPolicy::try_new(5, 0.25).unwrap(),
    )
    .binding();
    let mut encoded = serde_json::to_value(binding).unwrap();
    encoded["retrievalProfile"] = serde_json::json!("a3s.memory.lexical.unknown.v1");
    let invalid: DurableMemoryBindingV1 = serde_json::from_value(encoded).unwrap();
    let mut invalid_snapshot = snapshot(false);
    invalid_snapshot.session.durable_memory_binding = Some(invalid);

    let error = SessionCheckpointExportV1::new(invalid_snapshot, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("retrieval profile"), "{error}");

    let namespace = MemoryNamespace::try_new("tenant", "principal", "workspace").unwrap();
    let binding = DurableMemorySession::active_recall(
        Arc::new(InMemoryRepository::new()),
        namespace,
        DurableMemoryRecallPolicy::try_new(5, 0.25).unwrap(),
    )
    .binding();
    let mut encoded = serde_json::to_value(binding).unwrap();
    encoded["schemaVersion"] = serde_json::json!(1);
    let invalid: DurableMemoryBindingV1 = serde_json::from_value(encoded).unwrap();
    let mut invalid_snapshot = snapshot(false);
    invalid_snapshot.session.durable_memory_binding = Some(invalid);

    let error = SessionCheckpointExportV1::new(invalid_snapshot, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("retrieval profile"), "{error}");
}

#[test]
fn portable_checkpoint_keeps_legacy_durable_memory_state_readable() {
    let namespace = MemoryNamespace::try_new("tenant", "principal", "workspace").unwrap();
    let binding = DurableMemorySession::active_recall(
        Arc::new(InMemoryRepository::new()),
        namespace,
        DurableMemoryRecallPolicy::try_new(5, 0.25).unwrap(),
    )
    .binding();
    let mut encoded = serde_json::to_value(binding).unwrap();
    encoded["schemaVersion"] = serde_json::json!(1);
    encoded.as_object_mut().unwrap().remove("retrievalProfile");
    let legacy: DurableMemoryBindingV1 = serde_json::from_value(encoded).unwrap();
    assert_eq!(legacy.schema_version(), 1);
    assert_eq!(legacy.retrieval_profile(), "a3s.memory.lexical.word.v1");

    let mut legacy_snapshot = snapshot(false);
    legacy_snapshot.session.tenant_id = Some("tenant".to_string());
    legacy_snapshot.session.principal = Some("principal".to_string());
    legacy_snapshot.session.durable_memory_binding = Some(legacy);
    SessionCheckpointExportV1::new(legacy_snapshot, None)
        .expect("legacy binding must remain readable and portable");
}

#[test]
fn portable_checkpoint_rejects_durable_memory_identity_drift() {
    let namespace = MemoryNamespace::try_new("tenant", "principal", "workspace").unwrap();
    let binding = DurableMemorySession::active_recall(
        Arc::new(InMemoryRepository::new()),
        namespace,
        DurableMemoryRecallPolicy::try_new(5, 0.25).unwrap(),
    )
    .binding();
    let mut drifted = snapshot(false);
    drifted.session.tenant_id = Some("other-tenant".to_string());
    drifted.session.principal = Some("principal".to_string());
    drifted.session.durable_memory_binding = Some(binding);

    let error = SessionCheckpointExportV1::new(drifted, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("tenant identity"), "{error}");
}

#[test]
fn checkpoint_bytes_omit_runtime_credentials_and_debug_redacts_content() {
    let mut snapshot = snapshot(false);
    snapshot.session.llm_config = Some(LlmConfigData {
        provider: "deepseek".into(),
        model: "deepseek-chat".into(),
        api_key: Some("must-never-enter-checkpoint".into()),
        base_url: Some("https://api.deepseek.com".into()),
    });

    let export = SessionCheckpointExportV1::new(snapshot, None).unwrap();
    let content = String::from_utf8(export.content().to_vec()).unwrap();
    assert!(!content.contains("must-never-enter-checkpoint"));
    assert!(!format!("{export:?}").contains("retain exact state"));
    assert!(export
        .open()
        .unwrap()
        .snapshot
        .session
        .llm_config
        .unwrap()
        .api_key
        .is_none());
}

#[test]
fn alternate_json_encoding_cannot_claim_the_canonical_content_identity() {
    let export = SessionCheckpointExportV1::new(snapshot(false), None).unwrap();
    let (descriptor, content) = export.into_parts();
    let mut noncanonical = Vec::with_capacity(content.len() + 1);
    noncanonical.push(b' ');
    noncanonical.extend(content);
    assert!(SessionCheckpointExportV1::from_parts(descriptor, noncanonical).is_err());
}
