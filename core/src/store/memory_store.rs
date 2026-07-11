use super::{SessionData, SessionSnapshotV1, SessionStore, SessionStoreCapabilities};
use crate::loop_checkpoint::LoopCheckpoint;
use crate::orchestration::WorkflowCheckpoint;
use crate::run::RunRecord;
use crate::subagent_task_tracker::SubagentTaskSnapshot;
use crate::tools::{ArtifactStore, ToolArtifact};
use crate::trace::TraceEvent;
use crate::verification::VerificationReport;
use anyhow::Result;
use std::collections::HashMap;

// ============================================================================
// In-Memory Session Store (for testing)
// ============================================================================

/// In-memory session store for testing
pub struct MemorySessionStore {
    /// One lock owns every persisted component of a session. Replacing an
    /// entry therefore publishes a complete generation to readers at once.
    sessions: tokio::sync::RwLock<HashMap<String, MemorySessionEntry>>,
    loop_checkpoints: tokio::sync::RwLock<HashMap<String, LoopCheckpoint>>,
    workflow_checkpoints: tokio::sync::RwLock<HashMap<String, WorkflowCheckpoint>>,
}

#[derive(Debug, Clone, Default)]
struct MemorySessionEntry {
    session: Option<SessionData>,
    artifacts: Vec<ToolArtifact>,
    trace_events: Vec<TraceEvent>,
    run_records: Vec<RunRecord>,
    verification_reports: Vec<VerificationReport>,
    subagent_tasks: Vec<SubagentTaskSnapshot>,
}

impl MemorySessionEntry {
    fn from_snapshot(snapshot: &SessionSnapshotV1) -> Self {
        Self {
            session: Some(snapshot.session.clone()),
            artifacts: snapshot.artifacts.clone(),
            trace_events: snapshot.trace_events.clone(),
            run_records: snapshot.run_records.clone(),
            verification_reports: snapshot.verification_reports.clone(),
            subagent_tasks: snapshot.subagent_tasks.clone(),
        }
    }

    fn snapshot(&self) -> Option<SessionSnapshotV1> {
        let session = self.session.clone()?;
        Some(SessionSnapshotV1 {
            schema_version: super::SESSION_SNAPSHOT_SCHEMA_VERSION,
            session,
            artifacts: self.artifacts.clone(),
            trace_events: self.trace_events.clone(),
            run_records: self.run_records.clone(),
            verification_reports: self.verification_reports.clone(),
            subagent_tasks: self.subagent_tasks.clone(),
        })
    }
}

impl MemorySessionStore {
    pub fn new() -> Self {
        Self {
            sessions: tokio::sync::RwLock::new(HashMap::new()),
            loop_checkpoints: tokio::sync::RwLock::new(HashMap::new()),
            workflow_checkpoints: tokio::sync::RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl SessionStore for MemorySessionStore {
    async fn save(&self, session: &SessionData) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        sessions.entry(session.id.clone()).or_default().session = Some(session.clone());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<SessionData>> {
        let sessions = self.sessions.read().await;
        Ok(sessions.get(id).and_then(|entry| entry.session.clone()))
    }

    async fn save_snapshot(&self, snapshot: &SessionSnapshotV1) -> Result<()> {
        snapshot.ensure_loadable()?;
        self.sessions.write().await.insert(
            snapshot.session.id.clone(),
            MemorySessionEntry::from_snapshot(snapshot),
        );
        Ok(())
    }

    async fn load_snapshot(&self, id: &str) -> Result<Option<SessionSnapshotV1>> {
        let snapshot = self
            .sessions
            .read()
            .await
            .get(id)
            .and_then(MemorySessionEntry::snapshot);
        if let Some(snapshot) = &snapshot {
            snapshot.ensure_loadable()?;
        }
        Ok(snapshot)
    }

    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities {
            atomic_session_snapshots: true,
        }
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(id);
        // Loop checkpoints are keyed by run_id, not session_id, so a
        // session-level delete can't address them. They are removed by
        // `delete_loop_checkpoint(run_id)` — called automatically by the
        // run lifecycle when each run reaches a terminal state in-process.
        Ok(())
    }

    async fn list(&self) -> Result<Vec<String>> {
        let sessions = self.sessions.read().await;
        Ok(sessions
            .iter()
            .filter(|(_, entry)| entry.session.is_some())
            .map(|(id, _)| id.clone())
            .collect())
    }

    async fn exists(&self, id: &str) -> Result<bool> {
        let sessions = self.sessions.read().await;
        Ok(sessions
            .get(id)
            .is_some_and(|entry| entry.session.is_some()))
    }

    async fn save_artifacts(&self, id: &str, artifacts: &ArtifactStore) -> Result<()> {
        self.sessions
            .write()
            .await
            .entry(id.to_string())
            .or_default()
            .artifacts = artifacts.artifacts();
        Ok(())
    }

    async fn load_artifacts(&self, id: &str) -> Result<Option<ArtifactStore>> {
        let artifacts = self
            .sessions
            .read()
            .await
            .get(id)
            .map(|entry| entry.artifacts.clone());
        Ok(artifacts.map(|artifacts| super::session_snapshot::artifact_store_from(&artifacts)))
    }

    async fn save_trace_events(&self, id: &str, events: &[TraceEvent]) -> Result<()> {
        self.sessions
            .write()
            .await
            .entry(id.to_string())
            .or_default()
            .trace_events = events.to_vec();
        Ok(())
    }

    async fn load_trace_events(&self, id: &str) -> Result<Option<Vec<TraceEvent>>> {
        Ok(self
            .sessions
            .read()
            .await
            .get(id)
            .map(|entry| entry.trace_events.clone()))
    }

    async fn save_run_records(&self, id: &str, records: &[RunRecord]) -> Result<()> {
        self.sessions
            .write()
            .await
            .entry(id.to_string())
            .or_default()
            .run_records = records.to_vec();
        Ok(())
    }

    async fn load_run_records(&self, id: &str) -> Result<Option<Vec<RunRecord>>> {
        Ok(self
            .sessions
            .read()
            .await
            .get(id)
            .map(|entry| entry.run_records.clone()))
    }

    async fn save_verification_reports(
        &self,
        id: &str,
        reports: &[VerificationReport],
    ) -> Result<()> {
        self.sessions
            .write()
            .await
            .entry(id.to_string())
            .or_default()
            .verification_reports = reports.to_vec();
        Ok(())
    }

    async fn load_verification_reports(&self, id: &str) -> Result<Option<Vec<VerificationReport>>> {
        Ok(self
            .sessions
            .read()
            .await
            .get(id)
            .map(|entry| entry.verification_reports.clone()))
    }

    async fn save_subagent_tasks(&self, id: &str, tasks: &[SubagentTaskSnapshot]) -> Result<()> {
        self.sessions
            .write()
            .await
            .entry(id.to_string())
            .or_default()
            .subagent_tasks = tasks.to_vec();
        Ok(())
    }

    async fn load_subagent_tasks(&self, id: &str) -> Result<Option<Vec<SubagentTaskSnapshot>>> {
        Ok(self
            .sessions
            .read()
            .await
            .get(id)
            .map(|entry| entry.subagent_tasks.clone()))
    }

    async fn save_loop_checkpoint(&self, run_id: &str, checkpoint: &LoopCheckpoint) -> Result<()> {
        checkpoint.ensure_addressed_by(run_id)?;
        self.loop_checkpoints
            .write()
            .await
            .insert(run_id.to_string(), checkpoint.clone());
        Ok(())
    }

    async fn load_loop_checkpoint(&self, run_id: &str) -> Result<Option<LoopCheckpoint>> {
        match self.loop_checkpoints.read().await.get(run_id).cloned() {
            // Enforce the same future-schema rejection as the file store so
            // the contract holds uniformly across backends.
            Some(cp) => {
                cp.ensure_loadable()?;
                cp.ensure_addressed_by(run_id)?;
                Ok(Some(cp))
            }
            None => Ok(None),
        }
    }

    async fn delete_loop_checkpoint(&self, run_id: &str) -> Result<()> {
        self.loop_checkpoints.write().await.remove(run_id);
        Ok(())
    }

    async fn save_workflow_checkpoint(
        &self,
        workflow_id: &str,
        checkpoint: &WorkflowCheckpoint,
    ) -> Result<()> {
        if checkpoint.workflow_id != workflow_id {
            anyhow::bail!(
                "workflow checkpoint key mismatch: requested workflow {:?}, payload belongs to {:?}",
                workflow_id,
                checkpoint.workflow_id
            );
        }
        self.workflow_checkpoints
            .write()
            .await
            .insert(workflow_id.to_string(), checkpoint.clone());
        Ok(())
    }

    async fn load_workflow_checkpoint(
        &self,
        workflow_id: &str,
    ) -> Result<Option<WorkflowCheckpoint>> {
        match self
            .workflow_checkpoints
            .read()
            .await
            .get(workflow_id)
            .cloned()
        {
            Some(cp) => {
                cp.ensure_loadable()?;
                if cp.workflow_id != workflow_id {
                    anyhow::bail!(
                        "workflow checkpoint key mismatch: requested workflow {:?}, payload belongs to {:?}",
                        workflow_id,
                        cp.workflow_id
                    );
                }
                Ok(Some(cp))
            }
            None => Ok(None),
        }
    }

    async fn delete_workflow_checkpoint(&self, workflow_id: &str) -> Result<()> {
        self.workflow_checkpoints.write().await.remove(workflow_id);
        Ok(())
    }

    fn backend_name(&self) -> &str {
        "memory"
    }
}
