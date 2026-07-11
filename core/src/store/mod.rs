//! Session persistence layer
//!
//! Provides pluggable session storage via the `SessionStore` trait.
//!
//! ## Default Implementation
//!
//! `FileSessionStore` stores each session as a JSON file:
//! - Session metadata (id, name, timestamps)
//! - Configuration (system prompt, policies)
//! - Conversation history (messages)
//! - Context usage statistics
//!
//! ## Custom Backends
//!
//! Implement `SessionStore` trait for custom backends (Redis, PostgreSQL, etc.):
//!
//! ```ignore
//! use a3s_code::store::{
//!     SessionData, SessionSnapshotV1, SessionStore, SessionStoreCapabilities,
//! };
//!
//! struct RedisStore { /* ... */ }
//!
//! #[async_trait::async_trait]
//! impl SessionStore for RedisStore {
//!     // Required by AgentSession::save: commit the entire value in one
//!     // backend transaction / atomic replacement.
//!     async fn save_snapshot(&self, snapshot: &SessionSnapshotV1) -> Result<()> { /* ... */ }
//!     async fn load_snapshot(&self, id: &str) -> Result<Option<SessionSnapshotV1>> { /* ... */ }
//!     fn capabilities(&self) -> SessionStoreCapabilities {
//!         SessionStoreCapabilities { atomic_session_snapshots: true }
//!     }
//!
//!     // Legacy fragment APIs remain available for migration compatibility.
//!     async fn save(&self, session: &SessionData) -> Result<()> { /* ... */ }
//!     async fn load(&self, id: &str) -> Result<Option<SessionData>> { /* ... */ }
//!     async fn delete(&self, id: &str) -> Result<()> { /* ... */ }
//!     async fn list(&self) -> Result<Vec<String>> { /* ... */ }
//!     async fn exists(&self, id: &str) -> Result<bool> { /* ... */ }
//! }
//! ```

mod file_store;
mod memory_store;
mod session_data;
mod session_snapshot;

#[cfg(test)]
mod tests;

pub use file_store::FileSessionStore;
pub use memory_store::MemorySessionStore;
pub use session_data::{
    ContextUsage, LlmConfigData, SessionConfig, SessionData, SessionState,
    DEFAULT_AUTO_COMPACT_THRESHOLD,
};
pub use session_snapshot::{SessionSnapshotV1, SESSION_SNAPSHOT_SCHEMA_VERSION};

use crate::loop_checkpoint::LoopCheckpoint;
use crate::run::RunRecord;
use crate::subagent_task_tracker::SubagentTaskSnapshot;
use crate::tools::ArtifactStore;
use crate::trace::TraceEvent;
use crate::verification::VerificationReport;
use anyhow::{bail, Result};

/// Persistence guarantees advertised by a session store implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionStoreCapabilities {
    /// A complete [`SessionSnapshotV1`] is committed as one atomic generation.
    pub atomic_session_snapshots: bool,
}

// ============================================================================
// Session Store Trait
// ============================================================================

/// Session storage trait
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// Save session data
    async fn save(&self, session: &SessionData) -> Result<()>;

    /// Load session data by ID
    async fn load(&self, id: &str) -> Result<Option<SessionData>>;

    /// Delete session data
    async fn delete(&self, id: &str) -> Result<()>;

    /// List all session IDs
    async fn list(&self) -> Result<Vec<String>>;

    /// Check if session exists
    async fn exists(&self, id: &str) -> Result<bool>;

    /// Save a complete session generation.
    ///
    /// There is deliberately no fragmented default write. Silently mapping an
    /// aggregate save to several independent writes would acknowledge a
    /// generation that readers can observe only partially. Backends must make
    /// their write semantics explicit by overriding this method.
    async fn save_snapshot(&self, _snapshot: &SessionSnapshotV1) -> Result<()> {
        bail!(
            "session store '{}' does not support aggregate session snapshots",
            self.backend_name()
        )
    }

    /// Load one complete session generation.
    ///
    /// Legacy backends are assembled through the fragment APIs. This path is
    /// best-effort and may observe concurrent fragment updates; callers can
    /// inspect [`Self::capabilities`] before relying on atomicity.
    async fn load_snapshot(&self, id: &str) -> Result<Option<SessionSnapshotV1>> {
        let Some(session) = self.load(id).await? else {
            return Ok(None);
        };
        let artifacts = self.load_artifacts(id).await?.unwrap_or_default();
        Ok(Some(SessionSnapshotV1::new(
            session,
            &artifacts,
            self.load_trace_events(id).await?.unwrap_or_default(),
            self.load_run_records(id).await?.unwrap_or_default(),
            self.load_verification_reports(id)
                .await?
                .unwrap_or_default(),
            self.load_subagent_tasks(id).await?.unwrap_or_default(),
        )))
    }

    /// Report persistence guarantees without requiring a write probe.
    fn capabilities(&self) -> SessionStoreCapabilities {
        SessionStoreCapabilities::default()
    }

    /// Save artifacts associated with a session.
    async fn save_artifacts(&self, _id: &str, artifacts: &ArtifactStore) -> Result<()> {
        if !artifacts.is_empty() {
            bail!(
                "session store '{}' does not support artifacts",
                self.backend_name()
            );
        }
        Ok(())
    }

    /// Load artifacts associated with a session.
    async fn load_artifacts(&self, _id: &str) -> Result<Option<ArtifactStore>> {
        Ok(None)
    }

    /// Save compact trace events associated with a session.
    async fn save_trace_events(&self, _id: &str, events: &[TraceEvent]) -> Result<()> {
        if !events.is_empty() {
            bail!(
                "session store '{}' does not support trace events",
                self.backend_name()
            );
        }
        Ok(())
    }

    /// Load compact trace events associated with a session.
    async fn load_trace_events(&self, _id: &str) -> Result<Option<Vec<TraceEvent>>> {
        Ok(None)
    }

    /// Save run snapshots and replayable runtime events associated with a session.
    async fn save_run_records(&self, _id: &str, records: &[RunRecord]) -> Result<()> {
        if !records.is_empty() {
            bail!(
                "session store '{}' does not support run records",
                self.backend_name()
            );
        }
        Ok(())
    }

    /// Load run snapshots and replayable runtime events associated with a session.
    async fn load_run_records(&self, _id: &str) -> Result<Option<Vec<RunRecord>>> {
        Ok(None)
    }

    /// Save structured verification reports associated with a session.
    async fn save_verification_reports(
        &self,
        _id: &str,
        reports: &[VerificationReport],
    ) -> Result<()> {
        if !reports.is_empty() {
            bail!(
                "session store '{}' does not support verification reports",
                self.backend_name()
            );
        }
        Ok(())
    }

    /// Load structured verification reports associated with a session.
    async fn load_verification_reports(
        &self,
        _id: &str,
    ) -> Result<Option<Vec<VerificationReport>>> {
        Ok(None)
    }

    /// Save the session's delegated subagent task tracker snapshots.
    ///
    /// Cluster-grade hosts need this so a migrated session keeps a
    /// queryable history of its delegated child runs. Cancellers are
    /// **not** persisted — they are runtime-only and re-attaching them
    /// is the executor's job at task respawn time.
    async fn save_subagent_tasks(&self, _id: &str, tasks: &[SubagentTaskSnapshot]) -> Result<()> {
        if !tasks.is_empty() {
            bail!(
                "session store '{}' does not support subagent tasks",
                self.backend_name()
            );
        }
        Ok(())
    }

    /// Load the session's delegated subagent task tracker snapshots.
    async fn load_subagent_tasks(&self, _id: &str) -> Result<Option<Vec<SubagentTaskSnapshot>>> {
        Ok(None)
    }

    /// Save the latest per-tool-round loop checkpoint for `run_id`.
    ///
    /// The agent loop calls this through the
    /// [`SessionStoreCheckpointSink`](crate::loop_checkpoint::SessionStoreCheckpointSink)
    /// adapter after each completed tool round. Implementations should
    /// **overwrite** any earlier checkpoint for the same `run_id` — the
    /// loop only ever needs the most recent boundary.
    async fn save_loop_checkpoint(
        &self,
        _run_id: &str,
        _checkpoint: &LoopCheckpoint,
    ) -> Result<()> {
        Ok(())
    }

    /// Load the latest loop checkpoint for `run_id`.
    async fn load_loop_checkpoint(&self, _run_id: &str) -> Result<Option<LoopCheckpoint>> {
        Ok(None)
    }

    /// Delete the loop checkpoint for `run_id`, if present.
    ///
    /// Called by the run lifecycle when a run reaches a terminal state
    /// **in-process** (completed, failed, or cancelled) — at that point
    /// the checkpoint is dead weight. Only a process crash (the agent
    /// loop never returns) should leave a checkpoint behind for
    /// crash-recovery resume. Without this, every tool-using run would
    /// leak a checkpoint forever — the dominant unbounded-growth source
    /// for long-running cluster deployments.
    ///
    /// Deleting a non-existent checkpoint is a no-op success.
    async fn delete_loop_checkpoint(&self, _run_id: &str) -> Result<()> {
        Ok(())
    }

    /// Persist a workflow checkpoint, overwriting any earlier one for the same
    /// `workflow_id`. The resumable orchestration combinators call this at each
    /// step boundary so an interrupted workflow resumes from the last
    /// completed step (here or, after migration, on another node).
    async fn save_workflow_checkpoint(
        &self,
        _workflow_id: &str,
        _checkpoint: &crate::orchestration::WorkflowCheckpoint,
    ) -> Result<()> {
        Ok(())
    }

    /// Load the latest workflow checkpoint for `workflow_id`.
    async fn load_workflow_checkpoint(
        &self,
        _workflow_id: &str,
    ) -> Result<Option<crate::orchestration::WorkflowCheckpoint>> {
        Ok(None)
    }

    /// Delete the workflow checkpoint for `workflow_id`, if present. Called
    /// when a workflow reaches a terminal state in-process; only a crash should
    /// leave one behind for resume. Deleting a non-existent checkpoint is a
    /// no-op success.
    async fn delete_workflow_checkpoint(&self, _workflow_id: &str) -> Result<()> {
        Ok(())
    }

    /// Health check — verify the store backend is reachable and operational
    async fn health_check(&self) -> Result<()> {
        Ok(())
    }

    /// Backend name for diagnostics
    fn backend_name(&self) -> &str {
        "unknown"
    }
}
