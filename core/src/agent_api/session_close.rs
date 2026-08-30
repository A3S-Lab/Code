//! Out-of-band session close handle.
//!
//! `SessionCloseHandle` is an `Arc`-shareable substruct that owns just the
//! fields needed to terminate an `AgentSession` from outside (typically from
//! the parent [`Agent`](super::Agent)'s session registry).
//!
//! `AgentSession` carries one of these via `Arc<SessionCloseHandle>`; the
//! parent `Agent` stores a `Weak<SessionCloseHandle>` in its registry. When
//! the user drops the session, the handle drops too and the registry's
//! `Weak` becomes dangling — pruned on the next `list_sessions()` /
//! `close_session()` call.
//!
//! Sharing the close mechanics through a single `close()` method on this
//! struct guarantees `AgentSession::close()` and `Agent::close_session(id)`
//! perform exactly the same cleanup.

use crate::hitl::ConfirmationProvider;
use crate::hooks::{HookEvent, HookExecutor, SessionEndEvent};
use crate::run::InMemoryRunStore;
use crate::subagent_task_tracker::{InMemorySubagentTaskTracker, SubagentStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const QUEUE_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const MEMORY_EXTRACTION_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const RUN_CANCEL_GRACE: std::time::Duration = std::time::Duration::from_secs(1);
const RUN_ABORT_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

/// Bundle of `Arc`-shared session state needed to perform a graceful close
/// from anywhere holding (a clone of) the handle.
pub(crate) struct SessionCloseHandle {
    pub(crate) session_id: String,
    /// Tripped on first `close()` call; subsequent calls become no-ops and
    /// `AgentSession::send`/`stream` fast-fail.
    pub(crate) closed: Arc<AtomicBool>,
    /// Session-level parent token. All in-flight run/subagent tokens are
    /// `child_token()` of this.
    pub(crate) session_cancel: CancellationToken,
    /// Per-run cancel-token slot (currently active run's token, if any).
    /// Populated by the run lifecycle.
    pub(crate) cancel_token: Arc<Mutex<Option<CancellationToken>>>,
    /// Current run id (matches `cancel_token` when set).
    pub(crate) current_run_id: Arc<Mutex<Option<String>>>,
    pub(crate) run_store: Arc<InMemoryRunStore>,
    pub(crate) subagent_tasks: Arc<InMemorySubagentTaskTracker>,
    pub(crate) confirmation_manager: Option<Arc<dyn ConfirmationProvider>>,
    pub(crate) hook_executor: Option<Arc<dyn HookExecutor>>,
    pub(crate) started_at: std::time::Instant,
    /// Optional session-owned lane queue. Close first shuts it down to reject
    /// new work, then drains commands admitted before the close boundary.
    pub(crate) command_queue: Option<Arc<crate::session_lane_queue::SessionLaneQueue>>,
    /// Single-flight run admission and the abort handles for any streaming
    /// execution/forwarder tasks owned by the active lease.
    pub(crate) run_admission: Arc<super::run_admission::RunAdmission>,
    /// Completed-turn memory extractions registered before the close boundary.
    /// They are drained before session cancellation so durable memories are not
    /// lost merely because the host closes immediately after receiving output.
    pub(crate) memory: Option<Arc<crate::memory::AgentMemory>>,
    /// Every periodic prune, semantic refresh, or host job owned by this session.
    pub(crate) memory_maintenance: Option<Arc<crate::memory::MemoryMaintenanceRuntime>>,
    /// Session-owned MCP source. Inherited/global managers are deliberately
    /// absent so closing one session cannot tear down shared connections.
    pub(crate) mcp_manager: Arc<crate::mcp::manager::McpManager>,
    /// Session tool executor used to unwind exact local MCP registrations at
    /// close while restoring their captured shadows.
    pub(crate) tool_executor: Arc<crate::tools::ToolExecutor>,
    /// Atomic host projection catalog whose effects retire at Session close.
    pub(crate) capability_catalog: Arc<crate::capability::CapabilityCatalog>,
    /// Serializes session-local extension mutation. `closed` is the admission
    /// bit; close takes this mutex before its final MCP cleanup so a mutation
    /// admitted before close either rolls back or is subsequently cleaned up.
    pub(crate) extension_mutation: Mutex<()>,
    /// Linearizes immediate, non-async capability changes (dynamic tools,
    /// workers, hooks, and commands) with the close admission boundary.
    pub(crate) immediate_extension_mutation: std::sync::Mutex<()>,
    /// Exact MCP wrapper identities installed by each session-local server.
    pub(crate) mcp_tool_ownership:
        std::sync::Mutex<super::session_extensions::SessionMcpToolOwnership>,
    /// Effective session-local skill registry shared by the Skill tools and the
    /// dynamic catalog context provider.
    pub(crate) skill_registry: Arc<crate::skills::SkillRegistry>,
    /// Exact live skill registrations owned by this session.
    pub(crate) skill_ownership: std::sync::Mutex<super::session_extensions::SessionSkillOwnership>,
    /// Session-owned semantic index. It is cancelled, joined, and cleared by
    /// the same close boundary as runs and other session resources.
    pub(crate) workspace_retrieval: Option<Arc<crate::workspace::WorkspaceRetrievalRuntime>>,
    /// Local manifest backend created by Code for this session. Host-supplied
    /// backends keep their external lifetime and are never shut down here.
    pub(crate) owned_workspace_backend: Option<Arc<crate::workspace::ManifestWorkspaceBackend>>,
}

impl SessionCloseHandle {
    /// Return whether `close()` has already been called.
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Perform the full session close sequence. Idempotent: subsequent calls
    /// are no-ops and are guaranteed not to panic.
    ///
    /// Sequence (see [`AgentSession::close`](super::AgentSession::close)
    /// for the public-facing contract):
    /// 1. Flip the `closed` flag so further `send`/`stream` fast-fail;
    /// 2. Stop the lane queue from accepting new commands;
    /// 3. Cancel and join owned memory maintenance;
    /// 4. Drain completed-turn memory extractions already accepted for storage;
    /// 5. Fire the session-level cancellation token so every derived run
    ///    and subagent task token fires;
    /// 6. Mark the active run as `Cancelled` in the run store and notify the
    ///    configured hook executor;
    /// 7. Mark every still-running delegated subagent task as `Cancelled`
    ///    in the tracker;
    /// 8. Cancel pending HITL tool confirmations so blocked tool callers
    ///    receive a rejection instead of hanging.
    /// 9. Drain commands admitted before queue shutdown;
    /// 10. Settle the active stream, force-aborting its execution and forwarder
    ///     after a bounded cooperative grace period;
    /// 11. Disconnect MCP servers owned by this session only.
    pub(crate) async fn close(&self) {
        {
            let _mutation = self
                .immediate_extension_mutation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.closed.swap(true, Ordering::AcqRel) {
                return;
            }
        }

        // 1. Establish the queue admission boundary before the first
        // cancellation await so close cannot race with a new queued command.
        if let Some(queue) = &self.command_queue {
            queue.shutdown().await;
        }

        // 2. Stop periodic memory mutation before draining the final accepted
        // extraction writes. The runtime applies its own bounded join policy.
        if let Some(maintenance) = &self.memory_maintenance {
            let report = maintenance.close().await;
            if !report.is_clean() {
                tracing::warn!(
                    session_id = %self.session_id,
                    jobs_aborted = report.jobs_aborted,
                    join_failures = report.join_failures,
                    "Memory maintenance did not close cleanly"
                );
            }
        }

        // 3. Preserve already-completed turns before cancellation invalidates
        // their model request tokens. The wait is bounded so an unavailable
        // provider cannot make session close hang indefinitely.
        if let Some(memory) = &self.memory {
            if !memory
                .drain_llm_extractions(MEMORY_EXTRACTION_DRAIN_TIMEOUT)
                .await
            {
                tracing::warn!(
                    session_id = %self.session_id,
                    "Completed-turn memory extraction did not drain before close timeout"
                );
            }
        }

        // 4. Fire the session-level token so children cascade.
        self.session_cancel.cancel();

        // The runtime derives its token from `session_cancel`, then performs a
        // bounded join and releases the complete vector allocation graph.
        if let Some(retrieval) = &self.workspace_retrieval {
            retrieval.close().await;
        }
        if let Some(backend) = &self.owned_workspace_backend {
            backend.shutdown();
        }

        // 5. Mark the active run cancelled and notify the hook executor. The
        //    per-run token has already fired via step 1.
        let had_active_token = self.cancel_token.lock().await.is_some();
        if had_active_token {
            if let Some(run_id) = self.current_run_id.lock().await.clone() {
                let _ = self.run_store.mark_cancelled(&run_id).await;
                if let Some(hook) = &self.hook_executor {
                    hook.record_run_cancelled(&run_id, &self.session_id, Some("cancelled by host"))
                        .await;
                }
            }
        }

        // 6. Mark every still-running subagent task cancelled.
        let pending: Vec<String> = self
            .subagent_tasks
            .list_for_parent(&self.session_id)
            .await
            .into_iter()
            .filter(|task| task.status == SubagentStatus::Running)
            .map(|task| task.task_id)
            .collect();
        for task_id in pending {
            let _ = self.subagent_tasks.cancel(&task_id).await;
        }

        // 7. Cancel pending HITL confirmations.
        if let Some(manager) = &self.confirmation_manager {
            let _ = manager.cancel_all().await;
        }

        // 8. Let work admitted before shutdown finish after cancellation has
        // propagated. A misbehaving command cannot block close indefinitely.
        if let Some(queue) = &self.command_queue {
            if let Err(error) = queue.drain(QUEUE_DRAIN_TIMEOUT).await {
                tracing::warn!(
                    session_id = %self.session_id,
                    error = %error,
                    "Session lane queue did not drain before close timeout"
                );
            }
        }

        // 9. Give the active operation a bounded cooperative grace period.
        // If a streaming execution or a blocked event forwarder ignores
        // cancellation, abort those concrete tasks while leaving the lifecycle
        // supervisor alive to clear run state and release admission.
        if !self.run_admission.wait_until_idle(RUN_CANCEL_GRACE).await {
            if self.run_admission.abort_stream_workers() {
                if !self.run_admission.wait_until_idle(RUN_ABORT_GRACE).await {
                    tracing::warn!(
                        session_id = %self.session_id,
                        "Session streaming tasks did not settle after forced abortion"
                    );
                }
            } else {
                tracing::warn!(
                    session_id = %self.session_id,
                    "Session blocking run did not settle before close timeout"
                );
            }
        }

        // 10. Wait for any extension mutation admitted before `closed` was
        // flipped, then release only session-owned MCP resources. Removing all
        // configurations (not only connected clients) also cleans up a failed
        // add transaction. Agent-global and host-owned inherited managers have
        // independent lifetimes.
        let _extension_mutation = self.extension_mutation.lock().await;
        let mut server_names: std::collections::HashSet<String> = self
            .mcp_manager
            .all_configs()
            .await
            .into_iter()
            .map(|config| config.name)
            .collect();
        server_names.extend(self.mcp_manager.list_connected().await);
        server_names.extend(
            self.mcp_tool_ownership
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .server_names(),
        );
        for name in server_names {
            if let Err(error) = self.mcp_manager.remove_server(&name).await {
                tracing::warn!(
                    session_id = %self.session_id,
                    server = %name,
                    error = %error,
                    "Failed to clean up session MCP transport during close"
                );
            }
            self.mcp_tool_ownership
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .remove(&name, &self.tool_executor);
        }

        self.skill_ownership
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove_all(&self.skill_registry);

        // No conversation Run remains after the admission drain above. Retire
        // the current projection's prepared effects, then close every retired
        // or rolled-back batch through the bounded catalog cleanup policy.
        self.capability_catalog.retire_current_effects();
        let capability_cleanup = self.capability_catalog.drain_cleanup().await;
        if !capability_cleanup.is_clean() {
            tracing::warn!(
                session_id = %self.session_id,
                effects_failed = capability_cleanup.effects_failed,
                effects_timed_out = capability_cleanup.effects_timed_out,
                "Session capability cleanup did not finish cleanly"
            );
        }

        if let Some(hook) = &self.hook_executor {
            let duration_ms = self.started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
            let event = HookEvent::SessionEnd(SessionEndEvent {
                session_id: self.session_id.clone(),
                total_tokens: 0,
                total_tool_calls: 0,
                duration_ms,
            });
            let _ = hook.fire(&event).await;
        }

        tracing::info!(session_id = %self.session_id, "AgentSession closed");
    }

    pub(crate) fn mutate_immediate<T>(
        &self,
        mutate: impl FnOnce() -> T,
    ) -> crate::error::Result<T> {
        let _mutation = self
            .immediate_extension_mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.closed.load(Ordering::Acquire) {
            return Err(crate::error::CodeError::SessionClosed {
                session_id: self.session_id.clone(),
            });
        }
        Ok(mutate())
    }
}
