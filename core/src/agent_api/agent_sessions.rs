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
    Replacing {
        reservation_id: u64,
        current: Weak<SessionCloseHandle>,
    },
}

impl SessionRegistry {
    fn prune_dead_sessions(&mut self) {
        self.entries.retain(|_, entry| match entry {
            SessionRegistryEntry::Building(_) => true,
            SessionRegistryEntry::Live(weak) => {
                weak.upgrade().is_some_and(|handle| !handle.is_closed())
            }
            // The reservation owns cleanup while a replacement is building.
            // Keep the entry even if the current handle closes so another
            // factory cannot steal the ID before finalization observes it.
            SessionRegistryEntry::Replacing { .. } => true,
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

/// Reservation for an in-progress replacement of one live session.
///
/// Unlike a normal build reservation, dropping this restores the current live
/// registry entry. This is the rollback boundary that keeps a failed model or
/// effort switch from stranding the host with a closed session.
struct SessionReplacementReservation {
    registry: Arc<Mutex<SessionRegistry>>,
    agent_closed: Arc<AtomicBool>,
    session_id: String,
    reservation_id: u64,
    current: Weak<SessionCloseHandle>,
    finalized: bool,
}

impl SessionReplacementReservation {
    fn finalize(
        mut self,
        replacement: &Arc<SessionCloseHandle>,
    ) -> Result<Arc<SessionCloseHandle>> {
        let result = {
            let mut registry = self
                .registry
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if self.agent_closed.load(Ordering::Acquire) {
                remove_replacement_reservation(
                    &mut registry,
                    &self.session_id,
                    self.reservation_id,
                );
                Err(agent_closed_error())
            } else if !replacement_reservation_is_owned(
                &registry,
                &self.session_id,
                self.reservation_id,
            ) {
                Err(CodeError::Session(format!(
                    "Session replacement reservation was lost for '{}'",
                    self.session_id
                )))
            } else {
                let current = self.current.upgrade().filter(|handle| !handle.is_closed());
                match current {
                    Some(current) => {
                        registry.entries.insert(
                            self.session_id.clone(),
                            SessionRegistryEntry::Live(Arc::downgrade(replacement)),
                        );
                        Ok(current)
                    }
                    None => {
                        registry.entries.remove(&self.session_id);
                        Err(CodeError::SessionClosed {
                            session_id: self.session_id.clone(),
                        })
                    }
                }
            }
        };
        self.finalized = true;
        result
    }
}

impl Drop for SessionReplacementReservation {
    fn drop(&mut self) {
        if self.finalized {
            return;
        }
        let mut registry = self
            .registry
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !replacement_reservation_is_owned(&registry, &self.session_id, self.reservation_id) {
            return;
        }
        match self.current.upgrade().filter(|handle| !handle.is_closed()) {
            Some(current) if !self.agent_closed.load(Ordering::Acquire) => {
                registry.entries.insert(
                    self.session_id.clone(),
                    SessionRegistryEntry::Live(Arc::downgrade(&current)),
                );
            }
            _ => {
                registry.entries.remove(&self.session_id);
            }
        }
    }
}

fn replacement_reservation_is_owned(
    registry: &SessionRegistry,
    session_id: &str,
    reservation_id: u64,
) -> bool {
    matches!(
        registry.entries.get(session_id),
        Some(SessionRegistryEntry::Replacing {
            reservation_id: current,
            ..
        }) if *current == reservation_id
    )
}

fn remove_replacement_reservation(
    registry: &mut SessionRegistry,
    session_id: &str,
    reservation_id: u64,
) {
    if replacement_reservation_is_owned(registry, session_id, reservation_id) {
        registry.entries.remove(session_id);
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

pub(super) async fn open_protocol_session_async(
    agent: &Agent,
    workspace: impl Into<String>,
    mut options: SessionOptions,
    create_if_missing: bool,
) -> Result<Option<AgentSession>> {
    bail_if_agent_closed(agent)?;
    let session_id = options
        .session_id
        .clone()
        .ok_or_else(|| CodeError::SessionConfiguration {
            field: "session_id",
            message: "protocol sessions require a host-selected session ID".to_string(),
        })?;
    let store = session_config::resolve_session_store(&agent.code_config, &options).await?;
    let persisted = match &store {
        Some(store) => store.exists(&session_id).await?,
        None => false,
    };
    if !persisted && !create_if_missing {
        return Ok(None);
    }
    if let Some(store) = store {
        options = options.with_session_store(store);
    }
    if persisted {
        resume_session_async(agent, &session_id, options)
            .await
            .map(Some)
    } else {
        create_session_async(agent, workspace, Some(options))
            .await
            .map(Some)
    }
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

fn reserve_session_replacement(
    agent: &Agent,
    current: &AgentSession,
) -> Result<SessionReplacementReservation> {
    let session_id = current.session_id();
    let registry = Arc::clone(&agent.sessions);
    let agent_closed = Arc::clone(&agent.closed);
    let mut sessions = registry.lock().unwrap_or_else(|poison| poison.into_inner());
    if agent_closed.load(Ordering::Acquire) {
        return Err(agent_closed_error());
    }

    sessions.prune_dead_sessions();
    let current_handle = match sessions.entries.get(session_id) {
        Some(SessionRegistryEntry::Live(weak)) => weak
            .upgrade()
            .filter(|handle| Arc::ptr_eq(handle, &current.close_handle))
            .filter(|handle| !handle.is_closed()),
        Some(SessionRegistryEntry::Building(_))
        | Some(SessionRegistryEntry::Replacing { .. })
        | None => None,
    }
    .ok_or_else(|| CodeError::SessionConfiguration {
        field: "session_id",
        message: format!(
            "session '{session_id}' is not the registered live session or is already being replaced"
        ),
    })?;

    let reservation_id = sessions.next_reservation_id;
    sessions.next_reservation_id = sessions
        .next_reservation_id
        .checked_add(1)
        .ok_or_else(|| CodeError::Session("Session build reservation counter exhausted".into()))?;
    let current = Arc::downgrade(&current_handle);
    sessions.entries.insert(
        session_id.to_string(),
        SessionRegistryEntry::Replacing {
            reservation_id,
            current: current.clone(),
        },
    );
    drop(sessions);

    Ok(SessionReplacementReservation {
        registry,
        agent_closed,
        session_id: session_id.to_string(),
        reservation_id,
        current,
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
            SessionRegistryEntry::Live(_) | SessionRegistryEntry::Replacing { .. } => {
                Some(id.clone())
            }
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
            Some(SessionRegistryEntry::Replacing { current, .. }) => {
                let handle = Weak::upgrade(current);
                // Removing the entry invalidates the in-progress replacement;
                // its finalization will close the newly built session.
                sessions.entries.remove(session_id);
                handle
            }
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
                SessionRegistryEntry::Replacing { current, .. } => Weak::upgrade(current),
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
    options: SessionOptions,
) -> Result<AgentSession> {
    bail_if_agent_closed(agent)?;
    let reservation = reserve_session(agent, session_id)?;

    let session = build_resumed_session(agent, session_id, options).await?;
    if let Err(error) = reservation.finalize(&session.close_handle) {
        session.close().await;
        return Err(error);
    }

    Ok(session)
}

pub(super) async fn replace_session_async(
    agent: &Agent,
    current: &AgentSession,
    options: SessionOptions,
) -> Result<AgentSession> {
    bail_if_agent_closed(agent)?;
    if current.is_closed() {
        return Err(CodeError::SessionClosed {
            session_id: current.session_id().to_string(),
        });
    }

    let session_id = current.session_id().to_string();
    let reservation = reserve_session_replacement(agent, current)?;
    current.save().await?;
    let options = options.with_session_id(&session_id);
    let replacement = build_resumed_session(agent, &session_id, options).await?;
    let current_handle = match reservation.finalize(&replacement.close_handle) {
        Ok(handle) => handle,
        Err(error) => {
            replacement.close().await;
            return Err(error);
        }
    };

    // The registry already points at the replacement, so no new work can be
    // admitted through the old session ID while cleanup runs.
    current_handle.close().await;
    Ok(replacement)
}

async fn build_resumed_session(
    agent: &Agent,
    session_id: &str,
    mut options: SessionOptions,
) -> Result<AgentSession> {
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

    Ok(session)
}
