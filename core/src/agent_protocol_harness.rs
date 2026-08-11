//! Code-owned multi-session kernel for the native `a3s code harness` process.
//!
//! The executable supplies HTTP and health transport. This kernel owns only
//! admission into existing `Agent`/`AgentSession` state and deliberately has
//! no parallel run store, scheduler, event journal, or recovery semantics.

use crate::agent_api::{Agent, SessionOptions};
use crate::agent_protocol::{
    AgentProtocolChangeSetRequestV1, AgentProtocolChangeSetV1, AgentProtocolCommandReceiptV1,
    AgentProtocolCommandV1, AgentProtocolError, AgentProtocolEventPageRequestV1,
    AgentProtocolEventPageV1, AgentProtocolRunIdentityV1,
};
use crate::agent_protocol_host::{AgentProtocolHost, AgentProtocolHostError};
use crate::error::CodeError;
use crate::release::{
    agent_harness_compatibility_v1, AgentReleaseError, AgentReleaseManifest, AGENT_PROTOCOL_V1,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

/// Finite default number of conversation sessions retained by one Harness.
pub const AGENT_PROTOCOL_HARNESS_MAX_SESSIONS: usize = 1_024;

/// Stable failures returned by the Code-owned multi-session Harness kernel.
#[derive(Debug, Error)]
pub enum AgentProtocolHarnessError {
    #[error(transparent)]
    Protocol(#[from] AgentProtocolError),
    #[error(transparent)]
    Release(#[from] AgentReleaseError),
    #[error(transparent)]
    Host(#[from] AgentProtocolHostError),
    #[error(transparent)]
    Code(#[from] CodeError),
    #[error("A3S Code Harness session was not found")]
    SessionNotFound,
    #[error("A3S Code Harness session capacity is exhausted")]
    SessionCapacity,
    #[error("A3S Code Harness is draining or stopped")]
    Closed,
    #[error("A3S Code Harness workspace isolation failed: {0}")]
    Workspace(String),
}

impl AgentProtocolHarnessError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Protocol(error) => error.code(),
            Self::Release(error) => error.code(),
            Self::Host(error) => error.code(),
            Self::Code(error) => error.code(),
            Self::SessionNotFound => "a3s.code.agent_protocol.session_not_found",
            Self::SessionCapacity => "a3s.code.agent_protocol.session_capacity",
            Self::Closed => "a3s.code.agent_protocol.harness_closed",
            Self::Workspace(_) => "a3s.code.agent_protocol.workspace_isolation",
        }
    }
}

struct HarnessSessionEntry {
    host: Arc<AgentProtocolHost>,
    _workspace: HarnessSessionWorkspace,
}

enum HarnessSessionWorkspace {
    Shared(PathBuf),
    Isolated {
        source: PathBuf,
        path: PathBuf,
        _temporary_root: tempfile::TempDir,
    },
}

impl HarnessSessionWorkspace {
    async fn prepare(source: PathBuf) -> Result<Self, AgentProtocolHarnessError> {
        tokio::task::spawn_blocking(move || {
            if !crate::git::is_git_repo(&source) {
                return Ok(Self::Shared(source));
            }
            let temporary_root = tempfile::Builder::new()
                .prefix("a3s-code-harness-session-")
                .tempdir()
                .map_err(|error| AgentProtocolHarnessError::Workspace(error.to_string()))?;
            let path = temporary_root.path().join("workspace");
            crate::git::create_isolated_worktree(&source, &path)
                .map_err(|error| AgentProtocolHarnessError::Workspace(error.to_string()))?;
            Ok(Self::Isolated {
                source,
                path,
                _temporary_root: temporary_root,
            })
        })
        .await
        .map_err(|error| AgentProtocolHarnessError::Workspace(error.to_string()))?
    }

    fn path(&self) -> &Path {
        match self {
            Self::Shared(path) | Self::Isolated { path, .. } => path,
        }
    }
}

impl Drop for HarnessSessionWorkspace {
    fn drop(&mut self) {
        if let Self::Isolated { source, path, .. } = self {
            if let Err(error) = crate::git::remove_isolated_worktree(source, path) {
                tracing::warn!(%error, workspace = %path.display(), "could not remove Agent Harness session worktree");
            }
        }
    }
}

/// Release-bound, multi-session kernel used by the sole native Harness.
///
/// Each entry is an [`AgentProtocolHost`] over one ordinary [`AgentSession`](crate::AgentSession).
/// The map only retains those Code-owned sessions for conversation reuse; it
/// never mirrors their runs or events. Miss admission is serialized so two
/// concurrent commands cannot construct the same session twice, while work on
/// already admitted sessions remains concurrent.
pub struct AgentProtocolHarness {
    manifest: Arc<AgentReleaseManifest>,
    agent: Arc<Agent>,
    workspace: String,
    session_options: SessionOptions,
    max_sessions: usize,
    sessions: RwLock<HashMap<String, Arc<HarnessSessionEntry>>>,
    admission: Mutex<()>,
    closed: AtomicBool,
}

impl std::fmt::Debug for AgentProtocolHarness {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentProtocolHarness")
            .field("agent_release_identity", &self.manifest.artifact().digest())
            .field("manifest_identity", &self.manifest.identity())
            .field("workspace", &self.workspace)
            .field("max_sessions", &self.max_sessions)
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl AgentProtocolHarness {
    /// Admit one release into the native Harness compatibility surface.
    pub fn new(
        manifest: AgentReleaseManifest,
        agent: Arc<Agent>,
        workspace: impl Into<String>,
    ) -> Result<Self, AgentProtocolHarnessError> {
        manifest.verify_compatibility(&agent_harness_compatibility_v1())?;
        if manifest.protocol() != AGENT_PROTOCOL_V1 {
            return Err(AgentProtocolHostError::ReleaseProtocolMismatch.into());
        }
        Ok(Self {
            manifest: Arc::new(manifest),
            agent,
            workspace: workspace.into(),
            session_options: SessionOptions::new(),
            max_sessions: AGENT_PROTOCOL_HARNESS_MAX_SESSIONS,
            sessions: RwLock::new(HashMap::new()),
            admission: Mutex::new(()),
            closed: AtomicBool::new(false),
        })
    }

    /// Apply common options to every Code session created by this Harness.
    ///
    /// A caller-provided session ID is ignored. The exact protocol identity is
    /// authoritative, and auto-save is always enabled when a store is present.
    pub fn with_session_options(mut self, options: SessionOptions) -> Self {
        self.session_options = options;
        self.session_options.session_id = None;
        self.session_options.auto_save = true;
        self
    }

    /// Override the finite retained-session limit.
    pub fn with_max_sessions(
        mut self,
        max_sessions: usize,
    ) -> Result<Self, AgentProtocolHarnessError> {
        if max_sessions == 0 {
            return Err(AgentProtocolHarnessError::SessionCapacity);
        }
        self.max_sessions = max_sessions;
        Ok(self)
    }

    pub fn manifest(&self) -> &AgentReleaseManifest {
        &self.manifest
    }

    pub fn agent_release_identity(&self) -> &str {
        self.manifest.artifact().digest()
    }

    pub fn max_sessions(&self) -> usize {
        self.max_sessions
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// Route an exact command into its Code-owned conversation session.
    pub async fn execute(
        &self,
        command: &AgentProtocolCommandV1,
    ) -> Result<AgentProtocolCommandReceiptV1, AgentProtocolHarnessError> {
        command.validate()?;
        let create_if_missing = matches!(
            command,
            AgentProtocolCommandV1::Start { .. } | AgentProtocolCommandV1::Recover { .. }
        );
        let host = self.host_for(command.identity(), create_if_missing).await?;
        host.execute(command).await.map_err(Into::into)
    }

    /// Route a bounded event query into the same authoritative Code session.
    pub async fn event_page(
        &self,
        request: &AgentProtocolEventPageRequestV1,
    ) -> Result<AgentProtocolEventPageV1, AgentProtocolHarnessError> {
        request.validate()?;
        let host = self.host_for(&request.identity, false).await?;
        host.event_page_for(request).await.map_err(Into::into)
    }

    /// Route an immutable change-set query into the same authoritative run.
    pub async fn change_set(
        &self,
        request: &AgentProtocolChangeSetRequestV1,
    ) -> Result<AgentProtocolChangeSetV1, AgentProtocolHarnessError> {
        request.validate()?;
        let host = self.host_for(&request.identity, false).await?;
        host.change_set_for(request).await.map_err(Into::into)
    }

    /// Stop admission and close every Code-owned session and Agent resource.
    pub async fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let _admission = self.admission.lock().await;
        self.agent.close().await;
        self.sessions.write().await.clear();
    }

    async fn host_for(
        &self,
        identity: &AgentProtocolRunIdentityV1,
        create_if_missing: bool,
    ) -> Result<Arc<AgentProtocolHost>, AgentProtocolHarnessError> {
        identity.validate()?;
        if identity.agent_release_identity != self.manifest.artifact().digest() {
            return Err(AgentProtocolHostError::ReleaseMismatch.into());
        }
        if self.is_closed() {
            return Err(AgentProtocolHarnessError::Closed);
        }
        if let Some(host) = self
            .sessions
            .read()
            .await
            .get(&identity.session_id)
            .map(|entry| Arc::clone(&entry.host))
        {
            return Ok(host);
        }

        let _admission = self.admission.lock().await;
        if self.is_closed() {
            return Err(AgentProtocolHarnessError::Closed);
        }
        if let Some(host) = self
            .sessions
            .read()
            .await
            .get(&identity.session_id)
            .map(|entry| Arc::clone(&entry.host))
        {
            return Ok(host);
        }
        if self.sessions.read().await.len() >= self.max_sessions {
            return Err(AgentProtocolHarnessError::SessionCapacity);
        }

        let workspace = HarnessSessionWorkspace::prepare(PathBuf::from(&self.workspace)).await?;
        let options = self
            .session_options
            .clone()
            .with_session_id(&identity.session_id)
            .with_auto_save(true);
        let session = self
            .agent
            .open_protocol_session_async(
                workspace.path().to_string_lossy().into_owned(),
                options,
                create_if_missing,
            )
            .await?
            .ok_or(AgentProtocolHarnessError::SessionNotFound)?;
        let host = Arc::new(AgentProtocolHost::from_manifest(
            &self.manifest,
            Arc::new(session),
        )?);
        self.sessions.write().await.insert(
            identity.session_id.clone(),
            Arc::new(HarnessSessionEntry {
                host: Arc::clone(&host),
                _workspace: workspace,
            }),
        );
        Ok(host)
    }
}
