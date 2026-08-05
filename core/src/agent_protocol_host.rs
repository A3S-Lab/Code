//! Code-owned adapter from the versioned headless protocol to `AgentSession`.
//!
//! This adapter deliberately stores no parallel run state or event journal.
//! Exact command replay is resolved by the session's authoritative run store,
//! and event pages are projected directly from that same store.

use crate::agent_api::{AgentRunSpawn, AgentSession};
use crate::agent_protocol::{
    validate_lower_sha256, AgentProtocolCommandReceiptV1, AgentProtocolCommandV1,
    AgentProtocolError, AgentProtocolEventPageRequestV1, AgentProtocolEventPageV1,
    AgentProtocolRunIdentityV1, AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE,
};
use crate::error::CodeError;
use crate::release::{AgentReleaseManifest, AGENT_PROTOCOL_V1};
use crate::run::RunSnapshot;
use std::sync::Arc;
use thiserror::Error;

/// Stable failures returned by the Code-owned headless protocol adapter.
#[derive(Debug, Error)]
pub enum AgentProtocolHostError {
    #[error(transparent)]
    Protocol(#[from] AgentProtocolError),
    #[error("A3S Code Agent command targets another release")]
    ReleaseMismatch,
    #[error("A3S Code Agent release declares another protocol")]
    ReleaseProtocolMismatch,
    #[error("A3S Code Agent command targets another session")]
    SessionMismatch,
    #[error("A3S Code Agent run was not found")]
    RunNotFound,
    #[error("A3S Code Agent run is not active; recover it from a durable checkpoint")]
    RunUnavailable,
    #[error("A3S Code Agent sequence cannot be represented on this host")]
    SequenceOverflow,
    #[error(transparent)]
    Code(#[from] CodeError),
}

impl AgentProtocolHostError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Protocol(error) => error.code(),
            Self::ReleaseMismatch => "a3s.code.agent_protocol.release_mismatch",
            Self::ReleaseProtocolMismatch => "a3s.code.agent_protocol.release_protocol_mismatch",
            Self::SessionMismatch => "a3s.code.agent_protocol.session_mismatch",
            Self::RunNotFound => "a3s.code.agent_protocol.run_not_found",
            Self::RunUnavailable => "a3s.code.agent_protocol.run_unavailable",
            Self::SequenceOverflow => "a3s.code.agent_protocol.sequence_overflow",
            Self::Code(error) => error.code(),
        }
    }
}

/// One release- and session-bound A3S Code headless protocol host.
///
/// Cloud, Fleet, and other callers may transport commands and receipts, but
/// this adapter is the sole mapping into Code's run lifecycle and event store.
#[derive(Clone)]
pub struct AgentProtocolHost {
    agent_release_identity: String,
    session: Arc<AgentSession>,
}

impl AgentProtocolHost {
    pub fn new(
        agent_release_identity: impl Into<String>,
        session: Arc<AgentSession>,
    ) -> Result<Self, AgentProtocolHostError> {
        let agent_release_identity = agent_release_identity.into();
        validate_lower_sha256("agent_release_identity", &agent_release_identity)?;
        Ok(Self {
            agent_release_identity,
            session,
        })
    }

    /// Bind an admitted v1 release manifest to its Code session.
    ///
    /// Capability compatibility remains an activation concern for the process
    /// host, but a manifest for another protocol can never enter this v1 host.
    pub fn from_manifest(
        manifest: &AgentReleaseManifest,
        session: Arc<AgentSession>,
    ) -> Result<Self, AgentProtocolHostError> {
        if manifest.protocol() != AGENT_PROTOCOL_V1 {
            return Err(AgentProtocolHostError::ReleaseProtocolMismatch);
        }
        Ok(Self {
            agent_release_identity: manifest.artifact().digest().to_string(),
            session,
        })
    }

    pub fn agent_release_identity(&self) -> &str {
        &self.agent_release_identity
    }

    pub fn session(&self) -> &Arc<AgentSession> {
        &self.session
    }

    /// Execute, cancel, or recover one exact run and return a digest-bound
    /// receipt. Start and recovery return after Code has admitted the detached
    /// worker; progress is observed through [`Self::event_page`].
    pub async fn execute(
        &self,
        command: &AgentProtocolCommandV1,
    ) -> Result<AgentProtocolCommandReceiptV1, AgentProtocolHostError> {
        command.validate()?;
        self.validate_identity(command.identity())?;

        let replayed = match command {
            AgentProtocolCommandV1::Start { request } => {
                let spawned = self
                    .session
                    .spawn_run_with_id(&request.identity.run_id, &request.prompt)
                    .await?;
                detach(spawned)
            }
            AgentProtocolCommandV1::Recover { request } => {
                let spawned = self
                    .session
                    .spawn_recovery_with_run_id(
                        &request.checkpoint_run_id,
                        &request.identity.run_id,
                    )
                    .await?;
                detach(spawned)
            }
            AgentProtocolCommandV1::Cancel { request } => {
                let snapshot = self.snapshot(&request.identity).await?;
                if snapshot.status.is_terminal() {
                    true
                } else if self.session.cancel_run(&request.identity.run_id).await {
                    false
                } else if self.snapshot(&request.identity).await?.status.is_terminal() {
                    true
                } else {
                    return Err(AgentProtocolHostError::RunUnavailable);
                }
            }
        };

        let snapshot = self.snapshot(command.identity()).await?;
        let receipt = AgentProtocolCommandReceiptV1 {
            schema: AgentProtocolCommandReceiptV1::SCHEMA.into(),
            action: command.action(),
            request_id: command.request_id().into(),
            identity: command.identity().clone(),
            command_digest: command.digest()?,
            state: snapshot.status.into(),
            latest_event_sequence_exclusive: u64::try_from(snapshot.event_count)
                .map_err(|_| AgentProtocolHostError::SequenceOverflow)?,
            observed_at_ms: now_ms().max(snapshot.updated_at_ms),
            replayed,
        };
        receipt.validate_for(command)?;
        Ok(receipt)
    }

    /// Project a bounded cursor page directly from Code's authoritative run
    /// store without introducing a second provider event model.
    pub async fn event_page(
        &self,
        identity: &AgentProtocolRunIdentityV1,
        after_event_sequence: Option<u64>,
        limit: usize,
    ) -> Result<AgentProtocolEventPageV1, AgentProtocolHostError> {
        identity.validate()?;
        self.validate_identity(identity)?;
        if limit == 0 || limit > AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE {
            return Err(AgentProtocolError::InvalidField("limit").into());
        }
        let after_sequence = after_event_sequence
            .map(|sequence| {
                usize::try_from(sequence).map_err(|_| AgentProtocolHostError::SequenceOverflow)
            })
            .transpose()?;
        let snapshot = self.snapshot(identity).await?;
        let page = self
            .session
            .run_event_page(&identity.run_id, after_sequence, limit)
            .await
            .ok_or(AgentProtocolHostError::RunNotFound)?;
        AgentProtocolEventPageV1::from_run_page(
            identity.clone(),
            snapshot.status,
            now_ms().max(snapshot.updated_at_ms),
            after_sequence,
            &page,
        )
        .map_err(Into::into)
    }

    /// Execute the canonical transport-facing event page query.
    pub async fn event_page_for(
        &self,
        request: &AgentProtocolEventPageRequestV1,
    ) -> Result<AgentProtocolEventPageV1, AgentProtocolHostError> {
        request.validate()?;
        self.event_page(
            &request.identity,
            request.after_event_sequence,
            usize::from(request.limit),
        )
        .await
    }

    fn validate_identity(
        &self,
        identity: &AgentProtocolRunIdentityV1,
    ) -> Result<(), AgentProtocolHostError> {
        if identity.agent_release_identity != self.agent_release_identity {
            return Err(AgentProtocolHostError::ReleaseMismatch);
        }
        if identity.session_id != self.session.session_id() {
            return Err(AgentProtocolHostError::SessionMismatch);
        }
        Ok(())
    }

    async fn snapshot(
        &self,
        identity: &AgentProtocolRunIdentityV1,
    ) -> Result<RunSnapshot, AgentProtocolHostError> {
        let snapshot = self
            .session
            .run_snapshot(&identity.run_id)
            .await
            .ok_or(AgentProtocolHostError::RunNotFound)?;
        if snapshot.session_id != identity.session_id {
            return Err(AgentProtocolHostError::SessionMismatch);
        }
        Ok(snapshot)
    }
}

fn detach(spawned: AgentRunSpawn) -> bool {
    match spawned {
        AgentRunSpawn::Started { worker, .. } => {
            // Tokio tasks continue after their JoinHandle is dropped. The
            // session remains the cancellation and graceful-shutdown owner.
            drop(worker);
            false
        }
        AgentRunSpawn::Replayed { .. } => true,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}
