use super::SessionData;
use crate::run::RunRecord;
use crate::subagent_task_tracker::SubagentTaskSnapshot;
use crate::tools::{ArtifactStore, ArtifactStoreLimits, ToolArtifact};
use crate::trace::TraceEvent;
use crate::verification::VerificationReport;
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Schema version written by [`SessionSnapshotV1`].
pub const SESSION_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Hard safety boundary for exact historical package identities retained by
/// one session snapshot. The default run-retention window is much smaller;
/// this cap also bounds explicitly unbounded or untrusted persisted input.
const MAX_PRIOR_COGNITIVE_PACKAGE_BINDINGS: usize = 256;

/// A complete, versioned persistence generation for one session.
///
/// Stores commit this value as a unit so conversation state and its related
/// runtime records cannot be observed from different save generations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshotV1 {
    pub schema_version: u32,
    pub session: SessionData,
    /// Exact cognitive bindings that were active before the current binding.
    ///
    /// Atomic session replacement may move a conversation to a newly admitted
    /// package generation. Historical run events retain their original
    /// binding. An identity may outlive its event when per-run FIFO retention
    /// trims that event, while `session.cognitive_package_binding` identifies
    /// the only binding available to new work.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prior_cognitive_package_bindings: Vec<crate::cognitive_context::CognitivePackageBindingV1>,
    #[serde(default)]
    pub artifacts: Vec<ToolArtifact>,
    #[serde(default)]
    pub trace_events: Vec<TraceEvent>,
    #[serde(default)]
    pub run_records: Vec<RunRecord>,
    #[serde(default)]
    pub verification_reports: Vec<VerificationReport>,
    #[serde(default)]
    pub subagent_tasks: Vec<SubagentTaskSnapshot>,
}

impl SessionSnapshotV1 {
    pub fn new(
        session: SessionData,
        artifacts: &ArtifactStore,
        trace_events: Vec<TraceEvent>,
        run_records: Vec<RunRecord>,
        verification_reports: Vec<VerificationReport>,
        subagent_tasks: Vec<SubagentTaskSnapshot>,
    ) -> Self {
        Self {
            schema_version: SESSION_SNAPSHOT_SCHEMA_VERSION,
            session,
            prior_cognitive_package_bindings: Vec::new(),
            artifacts: artifacts.artifacts(),
            trace_events,
            run_records,
            verification_reports,
            subagent_tasks,
        }
    }

    pub fn session_only(session: SessionData) -> Self {
        Self::new(
            session,
            &ArtifactStore::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    }

    /// Rebind a complete snapshot to a new session and workspace.
    ///
    /// Session forks retain historical artifacts, traces, run ids, and child
    /// session ids. Top-level run ownership and subagent parent ownership must
    /// move with the new session or the aggregate would no longer be loadable.
    pub fn fork_for_session(
        mut self,
        session_id: impl Into<String>,
        workspace: impl Into<String>,
    ) -> Result<Self> {
        let source_session_id = self.session.id.clone();
        self.validate_for_session(&source_session_id)?;

        let session_id = session_id.into();
        if session_id.trim().is_empty() {
            bail!("forked session id cannot be empty");
        }

        self.session.id = session_id.clone();
        self.session.config.workspace = workspace.into();
        for record in &mut self.run_records {
            record.snapshot.session_id.clone_from(&session_id);
        }
        for task in &mut self.subagent_tasks {
            if !task.parent_session_id.is_empty() {
                task.parent_session_id.clone_from(&session_id);
            }
        }

        self.validate_for_session(&session_id)?;
        Ok(self)
    }

    pub fn artifact_store(&self) -> ArtifactStore {
        artifact_store_from(&self.artifacts)
    }

    pub(crate) fn artifact_store_requirements(&self) -> ArtifactStoreLimits {
        artifact_store_requirements(&self.artifacts)
    }

    /// Change the binding available to future runs while retaining the exact
    /// identities carried by historical run evidence.
    pub(crate) fn replace_cognitive_package_binding(
        &mut self,
        replacement: Option<crate::cognitive_context::CognitivePackageBindingV1>,
    ) -> Result<()> {
        if let Some(binding) = &replacement {
            binding.validate().map_err(|error| {
                anyhow::anyhow!("replacement cognitive package binding is invalid: {error}")
            })?;
        }
        if self.session.cognitive_package_binding == replacement {
            return Ok(());
        }
        self.session.cognitive_package_binding = replacement;
        self.normalize_cognitive_package_bindings()?;
        self.validate_for_session(&self.session.id)
    }

    /// Extend historical package identities from retained run evidence.
    ///
    /// Existing identities survive per-run FIFO event trimming; retained
    /// `cognitive_context_bound` events add exact identities. The hard cap
    /// prevents persisted input from growing this set without limit.
    pub(crate) fn normalize_cognitive_package_bindings(&mut self) -> Result<()> {
        let current = self
            .session
            .cognitive_package_binding
            .as_ref()
            .map(binding_identity)
            .transpose()?;
        let mut identities = HashSet::new();
        let mut prior = Vec::new();

        for binding in &self.prior_cognitive_package_bindings {
            if prior.len() >= MAX_PRIOR_COGNITIVE_PACKAGE_BINDINGS {
                bail!(
                    "session snapshot {:?} retains more than {} prior cognitive package bindings",
                    self.session.id,
                    MAX_PRIOR_COGNITIVE_PACKAGE_BINDINGS
                );
            }
            binding.validate().map_err(|error| {
                anyhow::anyhow!("retained cognitive package binding is invalid: {error}")
            })?;
            let identity = binding_identity(binding)?;
            if current.as_ref() != Some(&identity) && identities.insert(identity) {
                prior.push(binding.clone());
            }
        }

        for record in &self.run_records {
            for event in &record.events {
                let crate::agent::AgentEvent::CognitiveContextBound { binding } = &event.event
                else {
                    continue;
                };
                binding.validate().map_err(|error| {
                    anyhow::anyhow!(
                        "run {:?} carries an invalid cognitive package binding: {error}",
                        record.snapshot.id
                    )
                })?;
                let identity = binding_identity(binding)?;
                if current.as_ref() == Some(&identity) || !identities.insert(identity) {
                    continue;
                }
                if prior.len() >= MAX_PRIOR_COGNITIVE_PACKAGE_BINDINGS {
                    bail!(
                        "session snapshot {:?} retains more than {} prior cognitive package bindings",
                        self.session.id,
                        MAX_PRIOR_COGNITIVE_PACKAGE_BINDINGS
                    );
                }
                prior.push(binding.clone());
            }
        }

        self.prior_cognitive_package_bindings = prior;
        Ok(())
    }

    pub fn ensure_loadable(&self) -> Result<()> {
        if self.schema_version != SESSION_SNAPSHOT_SCHEMA_VERSION {
            bail!(
                "unsupported session snapshot schema version {}; expected {}",
                self.schema_version,
                SESSION_SNAPSHOT_SCHEMA_VERSION
            );
        }
        Ok(())
    }

    /// Validate relationships that must hold within one persisted generation.
    ///
    /// Event buffers may be FIFO-trimmed, so their first sequence is allowed
    /// to be greater than zero and `event_count` is allowed to exceed the
    /// retained length. It must, however, remain a valid next-sequence cursor
    /// for every retained event.
    pub fn validate_invariants(&self) -> Result<()> {
        if self.prior_cognitive_package_bindings.len() > MAX_PRIOR_COGNITIVE_PACKAGE_BINDINGS {
            bail!(
                "session snapshot {:?} retains more than {} prior cognitive package bindings",
                self.session.id,
                MAX_PRIOR_COGNITIVE_PACKAGE_BINDINGS
            );
        }
        if let Some(binding) = &self.session.cognitive_package_binding {
            binding.validate().map_err(|error| {
                anyhow::anyhow!(
                    "session snapshot {:?} has an invalid cognitive package binding: {error}",
                    self.session.id
                )
            })?;
        }
        let mut cognitive_bindings = HashSet::with_capacity(
            self.prior_cognitive_package_bindings.len()
                + usize::from(self.session.cognitive_package_binding.is_some()),
        );
        let current_cognitive_binding = self
            .session
            .cognitive_package_binding
            .as_ref()
            .map(binding_identity)
            .transpose()?;
        if let Some(identity) = &current_cognitive_binding {
            cognitive_bindings.insert(identity.clone());
        }
        for (index, binding) in self.prior_cognitive_package_bindings.iter().enumerate() {
            binding.validate().map_err(|error| {
                anyhow::anyhow!(
                    "session snapshot {:?} has an invalid prior cognitive package binding at {}: {error}",
                    self.session.id,
                    index
                )
            })?;
            let identity = binding_identity(binding)?;
            if !cognitive_bindings.insert(identity.clone()) {
                bail!(
                    "session snapshot {:?} repeats cognitive package binding identity at {}",
                    self.session.id,
                    index
                );
            }
        }
        let mut run_ids = HashSet::with_capacity(self.run_records.len());

        for (run_index, record) in self.run_records.iter().enumerate() {
            let run_id = &record.snapshot.id;
            if !run_ids.insert(run_id.as_str()) {
                bail!(
                    "session snapshot {:?} contains duplicate run id {:?} at run record {}",
                    self.session.id,
                    run_id,
                    run_index
                );
            }

            if record.snapshot.session_id != self.session.id {
                bail!(
                    "run {:?} at record {} belongs to session {:?}, but snapshot belongs to session {:?}",
                    run_id,
                    run_index,
                    record.snapshot.session_id,
                    self.session.id
                );
            }

            let mut previous_sequence = None;
            for (event_index, event) in record.events.iter().enumerate() {
                if let Some(previous) = previous_sequence {
                    if event.sequence <= previous {
                        bail!(
                            "run {:?} event {} has sequence {}, which is not strictly greater than previous sequence {}",
                            run_id,
                            event_index,
                            event.sequence,
                            previous
                        );
                    }
                }
                previous_sequence = Some(event.sequence);
                if let crate::agent::AgentEvent::CognitiveContextBound { binding } = &event.event {
                    binding.validate().map_err(|error| {
                        anyhow::anyhow!(
                            "run {:?} event {} carries an invalid cognitive package binding: {error}",
                            run_id,
                            event_index
                        )
                    })?;
                    let identity = binding_identity(binding)?;
                    if !cognitive_bindings.contains(&identity) {
                        bail!(
                            "run {:?} event {} carries cognitive context outside session {:?} binding history",
                            run_id,
                            event_index,
                            self.session.id
                        );
                    }
                }
            }

            if let Some(max_sequence) = previous_sequence {
                let minimum_event_count = max_sequence.checked_add(1).ok_or_else(|| {
                    anyhow::anyhow!(
                        "run {:?} retained event sequence {} cannot be represented by event_count",
                        run_id,
                        max_sequence
                    )
                })?;
                if record.snapshot.event_count < minimum_event_count {
                    bail!(
                        "run {:?} event_count {} does not cover retained event sequence {}; expected at least {}",
                        run_id,
                        record.snapshot.event_count,
                        max_sequence,
                        minimum_event_count
                    );
                }
            }
        }
        for (task_index, task) in self.subagent_tasks.iter().enumerate() {
            // Older snapshots can contain an empty parent when progress/end
            // arrived before SubagentStart. A non-empty parent is authoritative
            // and must identify the session that owns this task tracker.
            if !task.parent_session_id.is_empty() && task.parent_session_id != self.session.id {
                bail!(
                    "subagent task {:?} at record {} belongs to parent session {:?}, but snapshot belongs to session {:?}",
                    task.task_id,
                    task_index,
                    task.parent_session_id,
                    self.session.id
                );
            }
        }

        Ok(())
    }

    /// Validate this snapshot for a load request targeting `session_id`.
    pub fn validate_for_session(&self, session_id: &str) -> Result<()> {
        self.ensure_loadable()?;
        if self.session.id != session_id {
            bail!(
                "requested session {:?}, but snapshot payload belongs to session {:?}",
                session_id,
                self.session.id
            );
        }
        self.validate_invariants()
    }
}

fn binding_identity(
    binding: &crate::cognitive_context::CognitivePackageBindingV1,
) -> Result<String> {
    serde_json::to_string(binding)
        .map_err(|error| anyhow::anyhow!("could not serialize cognitive binding identity: {error}"))
}

#[cfg(test)]
mod cognitive_binding_tests {
    use super::*;
    use crate::agent::AgentEvent;
    use crate::cognitive_context::{
        CognitiveContextLimits, CognitiveKnowledgeBindingV1, CognitivePackageBindingV1,
        COGNITIVE_PACKAGE_BINDING_SCHEMA,
    };
    use crate::run::{RunEventRecord, RunRecord, RunSnapshot, RunStatus};
    use sha2::{Digest, Sha256};

    fn digest(byte: u8) -> String {
        format!("sha256:{}", format!("{byte:02x}").repeat(32))
    }

    fn binding(generation: u64) -> CognitivePackageBindingV1 {
        let generation_digest = digest((generation % 250 + 1) as u8);
        let knowledge = CognitiveKnowledgeBindingV1::new(
            "domain-knowledge",
            "0.2",
            digest(251),
            generation,
            generation_digest.clone(),
        )
        .unwrap();
        let mut binding = CognitivePackageBindingV1 {
            schema: COGNITIVE_PACKAGE_BINDING_SCHEMA.to_string(),
            package_id: "contra-sense/handbook".to_string(),
            package_version: format!("0.{generation}.0"),
            lifecycle_generation: generation,
            generation_digest,
            capability_snapshot_digest: String::new(),
            knowledge,
            limits: CognitiveContextLimits::default(),
        };
        let encoded = serde_json::to_vec(&(
            binding.package_id.as_str(),
            binding.package_version.as_str(),
            binding.lifecycle_generation,
            &binding.generation_digest,
            binding.knowledge.surface_id.as_str(),
            binding.knowledge.format_version.as_str(),
            binding.knowledge.content_digest.as_str(),
        ))
        .unwrap();
        let domain = b"a3s.use.capability-snapshot.v1";
        let mut hasher = Sha256::new();
        hasher.update(b"agentic-ontology-canonical-v1\0");
        hasher.update((domain.len() as u64).to_be_bytes());
        hasher.update(domain);
        hasher.update((encoded.len() as u64).to_be_bytes());
        hasher.update(encoded);
        binding.capability_snapshot_digest = format!("sha256:{:x}", hasher.finalize());
        binding.validate().unwrap();
        binding
    }

    fn record(session_id: &str, binding: CognitivePackageBindingV1) -> RunRecord {
        RunRecord {
            snapshot: RunSnapshot {
                id: format!("run-{}", binding.lifecycle_generation),
                session_id: session_id.to_string(),
                status: RunStatus::Completed,
                prompt: "test".to_string(),
                created_at_ms: 1,
                updated_at_ms: 1,
                result_text: Some("done".to_string()),
                error: None,
                event_count: 1,
                workspace_change_set: None,
            },
            events: vec![RunEventRecord {
                sequence: 0,
                timestamp_ms: 1,
                event: AgentEvent::CognitiveContextBound { binding },
            }],
        }
    }

    #[test]
    fn normalization_rejects_more_than_the_hard_binding_limit() {
        let session_id = "cognitive-binding-limit";
        let mut snapshot =
            SessionSnapshotV1::session_only(super::super::tests::create_test_session_data());
        snapshot.session.id = session_id.to_string();
        snapshot.session.cognitive_package_binding = Some(binding(300));
        snapshot.run_records = (1..=MAX_PRIOR_COGNITIVE_PACKAGE_BINDINGS as u64 + 1)
            .map(|generation| record(session_id, binding(generation)))
            .collect();

        assert!(snapshot.normalize_cognitive_package_bindings().is_err());
    }
}

pub(super) fn artifact_store_from(artifacts: &[ToolArtifact]) -> ArtifactStore {
    // A snapshot is an authoritative persisted generation. Rehydrating it
    // through the default in-memory limits must not silently evict records
    // that were accepted by a store configured with larger limits.
    let defaults = ArtifactStoreLimits::default();
    let requirements = artifact_store_requirements(artifacts);
    let store = ArtifactStore::with_limits(ArtifactStoreLimits {
        max_artifacts: defaults.max_artifacts.max(requirements.max_artifacts),
        max_bytes: defaults.max_bytes.max(requirements.max_bytes),
    });
    for artifact in artifacts {
        store.put(artifact.clone());
    }
    store
}

fn artifact_store_requirements(artifacts: &[ToolArtifact]) -> ArtifactStoreLimits {
    ArtifactStoreLimits {
        max_artifacts: artifacts.len(),
        max_bytes: artifacts.iter().fold(0usize, |total, artifact| {
            total.saturating_add(artifact.content.len())
        }),
    }
}
