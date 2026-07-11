//! Agent-to-session factory operations.
//!
//! `Agent` is workspace-independent; this module owns the transition from an
//! agent config/runtime to a workspace-bound `AgentSession`, including resume.
//! It also implements the agent-side session registry. A session ID is reserved
//! before configuration or runtime initialization begins, then atomically
//! finalized to a `Weak<SessionCloseHandle>` after construction (and restore,
//! for resumed sessions) is complete. The same registry lock establishes the
//! admission boundary for `Agent::close`.

use super::{
    agent_binding, session_builder, session_close::SessionCloseHandle, session_config,
    session_persistence, Agent, AgentSession, SessionOptions,
};
use crate::error::{CodeError, Result};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// Agent-owned registry state guarded by `Agent::sessions`.
///
/// Building entries are reservations, not live sessions: they are deliberately
/// omitted from `list_sessions` and cannot be targeted by `close_session`.
/// Their only job is to prevent duplicate IDs and to make finalization atomic
/// with the permanent agent-close boundary.
#[derive(Default)]
pub(super) struct SessionRegistry {
    entries: HashMap<String, SessionRegistryEntry>,
    next_reservation_id: u64,
}

enum SessionRegistryEntry {
    Building(u64),
    Live(Weak<SessionCloseHandle>),
}

impl SessionRegistry {
    fn prune_dead_sessions(&mut self) {
        self.entries.retain(|_, entry| match entry {
            SessionRegistryEntry::Building(_) => true,
            SessionRegistryEntry::Live(weak) => {
                weak.upgrade().is_some_and(|handle| !handle.is_closed())
            }
        });
    }

    fn remove_reservation(&mut self, session_id: &str, reservation_id: u64) {
        let owned = matches!(
            self.entries.get(session_id),
            Some(SessionRegistryEntry::Building(current)) if *current == reservation_id
        );
        if owned {
            self.entries.remove(session_id);
        }
    }
}

/// RAII reservation for one in-progress session build.
///
/// Dropping a failed or cancelled build releases only its own reservation.
/// The monotonically increasing token prevents a stale drop from removing a
/// future reservation for the same session ID.
struct SessionReservation {
    registry: Arc<Mutex<SessionRegistry>>,
    agent_closed: Arc<AtomicBool>,
    session_id: String,
    reservation_id: u64,
    finalized: bool,
}

impl SessionReservation {
    fn finalize(mut self, handle: &Arc<SessionCloseHandle>) -> Result<()> {
        let result = {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if self.agent_closed.load(Ordering::Acquire) {
                registry.remove_reservation(&self.session_id, self.reservation_id);
                Err(agent_closed_error())
            } else {
                let owns_reservation = matches!(
                    registry.entries.get(&self.session_id),
                    Some(SessionRegistryEntry::Building(current))
                        if *current == self.reservation_id
                );
                if owns_reservation {
                    registry.entries.insert(
                        self.session_id.clone(),
                        SessionRegistryEntry::Live(Arc::downgrade(handle)),
                    );
                    Ok(())
                } else {
                    Err(CodeError::Session(format!(
                        "Session build reservation was lost for '{}'",
                        self.session_id
                    )))
                }
            }
        };
        self.finalized = true;
        result
    }
}

impl Drop for SessionReservation {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        registry.remove_reservation(&self.session_id, self.reservation_id);
    }
}

pub(super) async fn refresh_mcp_tools(agent: &Agent) -> Result<()> {
    if let Some(mcp) = &agent.global_mcp {
        let fresh = mcp.get_all_tools().await;
        *agent
            .global_mcp_tools
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = fresh;
    }
    Ok(())
}

pub(super) fn create_session(
    agent: &Agent,
    workspace: impl Into<String>,
    options: Option<SessionOptions>,
) -> Result<AgentSession> {
    bail_if_agent_closed(agent)?;

    let merged_opts = session_builder::prepare_session_options(agent, options.unwrap_or_default());
    let reservation = reserve_session(agent, required_session_id(&merged_opts)?)?;
    let workspace = workspace.into();
    let canonical = super::safe_canonicalize(std::path::Path::new(&workspace));
    let resolved =
        session_config::ResolvedSessionConfig::resolve_sync(agent, &canonical, merged_opts)?;
    let session = session_builder::build_agent_session_sync(agent, workspace, resolved)?;
    reservation.finalize(&session.close_handle)?;
    Ok(session)
}

pub(super) async fn create_session_async(
    agent: &Agent,
    workspace: impl Into<String>,
    options: Option<SessionOptions>,
) -> Result<AgentSession> {
    bail_if_agent_closed(agent)?;

    let options = session_builder::prepare_session_options(agent, options.unwrap_or_default());
    let reservation = reserve_session(agent, required_session_id(&options)?)?;
    let workspace = workspace.into();
    let canonical = super::safe_canonicalize(std::path::Path::new(&workspace));
    let resolved =
        session_config::ResolvedSessionConfig::resolve(agent, &canonical, options).await?;
    let session = session_builder::build_agent_session(agent, workspace, resolved).await?;
    if let Err(error) = reservation.finalize(&session.close_handle) {
        session.close().await;
        return Err(error);
    }
    Ok(session)
}

fn reserve_session(agent: &Agent, session_id: &str) -> Result<SessionReservation> {
    let registry = Arc::clone(&agent.sessions);
    let agent_closed = Arc::clone(&agent.closed);
    let mut sessions = registry.lock().unwrap_or_else(|poison| poison.into_inner());
    if agent_closed.load(Ordering::Acquire) {
        return Err(agent_closed_error());
    }

    sessions.prune_dead_sessions();
    if sessions.entries.contains_key(session_id) {
        return Err(CodeError::SessionConfiguration {
            field: "session_id",
            message: format!("session '{session_id}' is already live or being built"),
        });
    }

    let reservation_id = sessions.next_reservation_id;
    sessions.next_reservation_id =
        sessions.next_reservation_id.checked_add(1).ok_or_else(|| {
            CodeError::Session("Session build reservation counter exhausted".to_string())
        })?;
    sessions.entries.insert(
        session_id.to_string(),
        SessionRegistryEntry::Building(reservation_id),
    );
    drop(sessions);

    Ok(SessionReservation {
        registry,
        agent_closed,
        session_id: session_id.to_string(),
        reservation_id,
        finalized: false,
    })
}

fn required_session_id(options: &SessionOptions) -> Result<&str> {
    options
        .session_id
        .as_deref()
        .ok_or_else(|| CodeError::SessionConfiguration {
            field: "session_id",
            message: "a session id must be assigned before construction".to_string(),
        })
}

fn bail_if_agent_closed(agent: &Agent) -> Result<()> {
    if agent.closed.load(Ordering::Acquire) {
        return Err(agent_closed_error());
    }
    Ok(())
}

fn agent_closed_error() -> CodeError {
    CodeError::SessionClosed {
        session_id: "<agent-closed>".to_string(),
    }
}

pub(super) async fn list_sessions(agent: &Agent) -> Vec<String> {
    let mut sessions = agent
        .sessions
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    sessions.prune_dead_sessions();
    let mut ids: Vec<String> = sessions
        .entries
        .iter()
        .filter_map(|(id, entry)| match entry {
            SessionRegistryEntry::Building(_) => None,
            SessionRegistryEntry::Live(_) => Some(id.clone()),
        })
        .collect();
    ids.sort();
    ids
}

pub(super) async fn close_session(agent: &Agent, session_id: &str) -> bool {
    let handle: Option<Arc<SessionCloseHandle>> = {
        let mut sessions = agent
            .sessions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        sessions.prune_dead_sessions();
        match sessions.entries.get(session_id) {
            Some(SessionRegistryEntry::Live(weak)) => Weak::upgrade(weak),
            Some(SessionRegistryEntry::Building(_)) | None => None,
        }
    };
    match handle {
        Some(handle) => {
            let was_open = !handle.is_closed();
            handle.close().await;
            was_open
        }
        None => false,
    }
}

pub(super) async fn close_agent(agent: &Agent) {
    // Mark the agent closed while holding the same lock used by build
    // reservation/finalization. This is the lifecycle linearization point:
    // an admitted build either finalized first and is included below, or its
    // later finalization observes the closed flag and is rejected.
    let handles: Vec<Arc<SessionCloseHandle>> = {
        let mut sessions = agent
            .sessions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if agent.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        sessions.prune_dead_sessions();
        sessions
            .entries
            .values()
            .filter_map(|entry| match entry {
                SessionRegistryEntry::Building(_) => None,
                SessionRegistryEntry::Live(weak) => Weak::upgrade(weak),
            })
            .collect()
    };
    for handle in handles {
        handle.close().await;
    }

    // Tear down global MCP connections so background workers exit.
    if let Some(mcp) = &agent.global_mcp {
        for name in mcp.list_connected().await {
            if let Err(e) = mcp.disconnect(&name).await {
                tracing::warn!(
                    server = %name,
                    error = %e,
                    "Failed to disconnect MCP server during Agent::close"
                );
            }
        }
    }
}

pub(super) fn create_session_for_agent(
    agent: &Agent,
    workspace: impl Into<String>,
    def: &crate::subagent::AgentDefinition,
    extra: Option<SessionOptions>,
) -> Result<AgentSession> {
    let opts = agent_binding::apply_agent_definition(extra.unwrap_or_default(), def);
    create_session(agent, workspace, Some(opts))
}

pub(super) async fn create_session_for_agent_async(
    agent: &Agent,
    workspace: impl Into<String>,
    def: &crate::subagent::AgentDefinition,
    extra: Option<SessionOptions>,
) -> Result<AgentSession> {
    let opts = agent_binding::apply_agent_definition(extra.unwrap_or_default(), def);
    create_session_async(agent, workspace, Some(opts)).await
}

pub(super) fn resume_session(
    agent: &Agent,
    _session_id: &str,
    _options: SessionOptions,
) -> Result<AgentSession> {
    bail_if_agent_closed(agent)?;
    Err(CodeError::AsyncSessionBuildRequired {
        resource: crate::error::SessionBuildResource::SessionStore,
    })
}

pub(super) async fn resume_session_async(
    agent: &Agent,
    session_id: &str,
    mut options: SessionOptions,
) -> Result<AgentSession> {
    bail_if_agent_closed(agent)?;
    let reservation = reserve_session(agent, session_id)?;

    let store = session_config::resolve_session_store(&agent.code_config, &options)
        .await?
        .ok_or_else(|| crate::error::CodeError::SessionConfiguration {
            field: "session_store",
            message: "resume_session requires a configured session store".to_string(),
        })?;

    let snapshot = session_persistence::load_session_snapshot(&store, session_id).await?;
    let data = &snapshot.session;
    options = options.with_session_store(Arc::clone(&store));
    let mut opts = session_persistence::apply_persisted_runtime_options(options, data);
    session_persistence::ensure_artifact_restore_capacity(&mut opts, &snapshot);
    let opts = session_builder::prepare_session_options(agent, opts);
    let workspace = data.config.workspace.clone();
    let canonical = super::safe_canonicalize(std::path::Path::new(&workspace));
    let resolved = session_config::ResolvedSessionConfig::resolve(agent, &canonical, opts).await?;
    let session = session_builder::build_agent_session(agent, workspace, resolved).await?;
    if let Err(error) =
        session_persistence::restore_persisted_session_state(&session, snapshot).await
    {
        session.close().await;
        return Err(error);
    }

    if let Err(error) = reservation.finalize(&session.close_handle) {
        session.close().await;
        return Err(error);
    }

    Ok(session)
}
