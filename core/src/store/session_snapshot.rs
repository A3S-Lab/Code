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

/// A complete, versioned persistence generation for one session.
///
/// Stores commit this value as a unit so conversation state and its related
/// runtime records cannot be observed from different save generations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshotV1 {
    pub schema_version: u32,
    pub session: SessionData,
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
        if let Some(binding) = &self.session.cognitive_package_binding {
            binding.validate().map_err(|error| {
                anyhow::anyhow!(
                    "session snapshot {:?} has an invalid cognitive package binding: {error}",
                    self.session.id
                )
            })?;
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
                    match &self.session.cognitive_package_binding {
                        Some(expected) if expected == binding => {}
                        Some(_) => bail!(
                            "run {:?} event {} carries a cognitive generation different from session {:?}",
                            run_id,
                            event_index,
                            self.session.id
                        ),
                        None => bail!(
                            "run {:?} event {} carries cognitive context but session {:?} is unbound",
                            run_id,
                            event_index,
                            self.session.id
                        ),
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
