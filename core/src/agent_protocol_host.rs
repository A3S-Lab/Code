//! Code-owned adapter from the versioned headless protocol to `AgentSession`.
//!
//! This adapter deliberately stores no parallel run state or event journal.
//! Exact command replay is resolved by the session's authoritative run store,
//! and event pages are projected directly from that same store.

use crate::agent_api::{AgentRunSpawn, AgentSession, ExactRecoveryError, ExactRecoveryPreparation};
use crate::agent_protocol::{
    validate_lower_sha256, AgentProtocolChangeSetRequestV1, AgentProtocolChangeSetV1,
    AgentProtocolCommandReceiptV1, AgentProtocolCommandV1, AgentProtocolError,
    AgentProtocolEventPageRequestV1, AgentProtocolEventPageV1, AgentProtocolRunIdentityV1,
    AgentProtocolRunRecoverExactV1, AGENT_PROTOCOL_CHANGE_SET_ENCODING_V1,
    AGENT_PROTOCOL_CHANGE_SET_FORMAT_V1, AGENT_PROTOCOL_MAX_CHANGE_SET_BYTES,
    AGENT_PROTOCOL_MAX_EVENTS_PER_PAGE,
};
use crate::error::CodeError;
use crate::release::{AgentReleaseManifest, AGENT_PROTOCOL_V1};
use crate::run::{RunSnapshot, RunWorkspaceChangeSet};
use crate::session_checkpoint::SessionCheckpointError;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

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
    #[error("A3S Code Agent change set is still being captured")]
    ChangeSetPending,
    #[error("A3S Code Agent run has no Git-compatible change set")]
    ChangeSetUnavailable,
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
            Self::ChangeSetPending => "a3s.code.agent_protocol.change_set_pending",
            Self::ChangeSetUnavailable => "a3s.code.agent_protocol.change_set_unavailable",
            Self::Code(error) => error.code(),
        }
    }
}

/// Failures specific to the additive evidence-bound recovery entry point.
///
/// Keeping checkpoint drift in this adjacent error preserves the variant set
/// of [`AgentProtocolHostError`] for existing v1 command callers.
#[derive(Debug, Error)]
pub enum AgentProtocolExactRecoveryError {
    #[error(transparent)]
    Host(#[from] AgentProtocolHostError),
    #[error(transparent)]
    Checkpoint(#[from] SessionCheckpointError),
}

impl AgentProtocolExactRecoveryError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Host(error) => error.code(),
            Self::Checkpoint(error) => error.code(),
        }
    }
}

impl From<AgentProtocolError> for AgentProtocolExactRecoveryError {
    fn from(error: AgentProtocolError) -> Self {
        Self::Host(AgentProtocolHostError::Protocol(error))
    }
}

impl From<ExactRecoveryError> for AgentProtocolExactRecoveryError {
    fn from(error: ExactRecoveryError) -> Self {
        match error {
            ExactRecoveryError::Checkpoint(error) => Self::Checkpoint(error),
            ExactRecoveryError::Code(error) => Self::Host(AgentProtocolHostError::Code(error)),
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
    change_set_admission: Arc<Mutex<()>>,
    change_set_states: Arc<RwLock<HashMap<String, ChangeSetCaptureState>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeSetCaptureState {
    Capturing,
    Unavailable,
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
            change_set_admission: Arc::new(Mutex::new(())),
            change_set_states: Arc::new(RwLock::new(HashMap::new())),
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
            change_set_admission: Arc::new(Mutex::new(())),
            change_set_states: Arc::new(RwLock::new(HashMap::new())),
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

        let _change_set_admission = self.change_set_admission.lock().await;
        let replayed = match command {
            AgentProtocolCommandV1::Start { request } => {
                let baseline = self.prepare_change_set(&request.identity).await;
                let spawned = match self
                    .session
                    .spawn_run_with_id(&request.identity.run_id, &request.prompt)
                    .await
                {
                    Ok(spawned) => spawned,
                    Err(error) => {
                        self.mark_change_set_unavailable(&request.identity).await;
                        return Err(error.into());
                    }
                };
                self.detach_with_change_set_capture(&request.identity, spawned, baseline)
                    .await
            }
            AgentProtocolCommandV1::Recover { request } => {
                let baseline = self.prepare_change_set(&request.identity).await;
                let spawned = match self
                    .session
                    .spawn_recovery_with_run_id(
                        &request.checkpoint_run_id,
                        &request.identity.run_id,
                    )
                    .await
                {
                    Ok(spawned) => spawned,
                    Err(error) => {
                        self.mark_change_set_unavailable(&request.identity).await;
                        return Err(error.into());
                    }
                };
                self.detach_with_change_set_capture(&request.identity, spawned, baseline)
                    .await
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

    /// Recover only when the locally loadable loop boundary matches the
    /// logical component of the complete portable-checkpoint descriptor.
    ///
    /// This additive entry point leaves [`AgentProtocolCommandV1`] and its v1
    /// transport shape unchanged. Validation pins the immutable checkpoint in
    /// memory before workspace baseline capture and before the target Run is
    /// admitted, so a concurrently overwritten store entry cannot change what
    /// the worker resumes.
    pub async fn execute_exact_recovery(
        &self,
        request: &AgentProtocolRunRecoverExactV1,
    ) -> Result<AgentProtocolCommandReceiptV1, AgentProtocolExactRecoveryError> {
        request.validate()?;
        self.validate_identity(&request.identity)?;
        let logical_resume = request.logical_resume()?;

        let _change_set_admission = self.change_set_admission.lock().await;
        let prepared = self
            .session
            .prepare_recovery_with_evidence(
                logical_resume,
                &request.checkpoint.descriptor_digest,
                &request.identity.run_id,
            )
            .await?;
        self.finish_exact_recovery(request, prepared).await
    }

    /// Recover from the logical value decoded from the same validated portable
    /// checkpoint as `request`, without first publishing split store writes.
    pub async fn execute_exact_recovery_from_checkpoint(
        &self,
        request: &AgentProtocolRunRecoverExactV1,
        checkpoint: crate::loop_checkpoint::LoopCheckpoint,
    ) -> Result<AgentProtocolCommandReceiptV1, AgentProtocolExactRecoveryError> {
        request.validate()?;
        self.validate_identity(&request.identity)?;
        let logical_resume = request.logical_resume()?;

        let _change_set_admission = self.change_set_admission.lock().await;
        let prepared = self
            .session
            .prepare_recovery_from_checkpoint(
                logical_resume,
                &request.checkpoint.descriptor_digest,
                &request.identity.run_id,
                checkpoint,
            )
            .await?;
        self.finish_exact_recovery(request, prepared).await
    }

    async fn finish_exact_recovery(
        &self,
        request: &AgentProtocolRunRecoverExactV1,
        prepared: ExactRecoveryPreparation,
    ) -> Result<AgentProtocolCommandReceiptV1, AgentProtocolExactRecoveryError> {
        let replayed = match prepared {
            ExactRecoveryPreparation::Replayed(spawned) => spawned.replayed(),
            ExactRecoveryPreparation::Ready(prepared) => {
                let baseline = self.prepare_change_set(&request.identity).await;
                let spawned = match self.session.spawn_prepared_recovery(prepared).await {
                    Ok(spawned) => spawned,
                    Err(error) => {
                        self.mark_change_set_unavailable(&request.identity).await;
                        return Err(error.into());
                    }
                };
                self.detach_with_change_set_capture(&request.identity, spawned, baseline)
                    .await
            }
        };

        let snapshot = self.snapshot(&request.identity).await?;
        let receipt = AgentProtocolCommandReceiptV1 {
            schema: AgentProtocolCommandReceiptV1::SCHEMA.into(),
            action: crate::agent_protocol::AgentProtocolCommandActionV1::Recover,
            request_id: request.request_id.clone(),
            identity: request.identity.clone(),
            command_digest: request.digest()?,
            state: snapshot.status.into(),
            latest_event_sequence_exclusive: u64::try_from(snapshot.event_count)
                .map_err(|_| AgentProtocolHostError::SequenceOverflow)?,
            observed_at_ms: now_ms().max(snapshot.updated_at_ms),
            replayed,
        };
        receipt.validate_for_exact_recovery(request)?;
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
        let observation = self
            .session
            .run_event_observation(&identity.run_id, after_sequence, limit)
            .await
            .ok_or(AgentProtocolHostError::RunNotFound)?;
        if observation.snapshot.session_id != identity.session_id {
            return Err(AgentProtocolHostError::SessionMismatch);
        }
        AgentProtocolEventPageV1::from_run_page(
            identity.clone(),
            observation.snapshot.status,
            now_ms().max(observation.snapshot.updated_at_ms),
            after_sequence,
            &observation.page,
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

    /// Read the immutable Git-compatible patch captured for one terminal run.
    pub async fn change_set_for(
        &self,
        request: &AgentProtocolChangeSetRequestV1,
    ) -> Result<AgentProtocolChangeSetV1, AgentProtocolHostError> {
        request.validate()?;
        self.validate_identity(&request.identity)?;
        let snapshot = self.snapshot(&request.identity).await?;
        if !snapshot.status.is_terminal() {
            return Err(AgentProtocolHostError::ChangeSetPending);
        }
        if let Some(change_set) = snapshot.workspace_change_set {
            let response = AgentProtocolChangeSetV1 {
                schema: AgentProtocolChangeSetV1::SCHEMA.into(),
                identity: request.identity.clone(),
                state: snapshot.status.into(),
                format: AGENT_PROTOCOL_CHANGE_SET_FORMAT_V1.into(),
                encoding: AGENT_PROTOCOL_CHANGE_SET_ENCODING_V1.into(),
                base_tree: change_set.base_tree,
                result_tree: change_set.result_tree,
                patch_digest: change_set.patch_digest,
                patch_bytes: change_set.patch_bytes,
                patch_base64: change_set.patch_base64,
                observed_at_ms: change_set.observed_at_ms,
            };
            response.validate()?;
            return Ok(response);
        }
        match self
            .change_set_states
            .read()
            .await
            .get(&request.identity.run_id)
            .copied()
        {
            Some(ChangeSetCaptureState::Capturing) => Err(AgentProtocolHostError::ChangeSetPending),
            Some(ChangeSetCaptureState::Unavailable) | None => {
                Err(AgentProtocolHostError::ChangeSetUnavailable)
            }
        }
    }

    async fn prepare_change_set(
        &self,
        identity: &AgentProtocolRunIdentityV1,
    ) -> Option<crate::git::WorkspaceTreeSnapshot> {
        if self.session.run_snapshot(&identity.run_id).await.is_some() {
            return None;
        }
        let workspace = self.session.workspace().to_path_buf();
        let baseline =
            tokio::task::spawn_blocking(move || crate::git::snapshot_workspace_tree(&workspace))
                .await
                .ok()
                .and_then(Result::ok);
        self.change_set_states.write().await.insert(
            identity.run_id.clone(),
            if baseline.is_some() {
                ChangeSetCaptureState::Capturing
            } else {
                ChangeSetCaptureState::Unavailable
            },
        );
        baseline
    }

    async fn mark_change_set_unavailable(&self, identity: &AgentProtocolRunIdentityV1) {
        self.change_set_states
            .write()
            .await
            .insert(identity.run_id.clone(), ChangeSetCaptureState::Unavailable);
    }

    async fn detach_with_change_set_capture(
        &self,
        identity: &AgentProtocolRunIdentityV1,
        spawned: AgentRunSpawn,
        baseline: Option<crate::git::WorkspaceTreeSnapshot>,
    ) -> bool {
        match spawned {
            AgentRunSpawn::Started { worker, .. } => {
                let Some(baseline) = baseline else {
                    drop(worker);
                    return false;
                };
                let workspace = self.session.workspace().to_path_buf();
                let session = Arc::clone(&self.session);
                let run_id = identity.run_id.clone();
                let pin_identity = format!(
                    "{}:{}:{}",
                    self.agent_release_identity, identity.session_id, identity.run_id
                );
                let states = Arc::clone(&self.change_set_states);
                tokio::spawn(async move {
                    let _ = worker.await;
                    let evidence = tokio::task::spawn_blocking(move || {
                        let result = crate::git::snapshot_workspace_tree(&workspace)?;
                        let patch = crate::git::diff_workspace_trees(
                            &workspace,
                            &baseline,
                            &result,
                            AGENT_PROTOCOL_MAX_CHANGE_SET_BYTES,
                        )?;
                        crate::git::pin_workspace_tree(&workspace, &pin_identity, &result)?;
                        let patch_bytes = u64::try_from(patch.len())
                            .map_err(|_| anyhow::anyhow!("change-set byte count overflowed"))?;
                        Ok::<_, anyhow::Error>(RunWorkspaceChangeSet {
                            base_tree: format!("git-tree:{}", baseline.tree),
                            result_tree: format!("git-tree:{}", result.tree),
                            patch_digest: format!("sha256:{:x}", Sha256::digest(&patch)),
                            patch_bytes,
                            patch_base64: base64::engine::general_purpose::STANDARD.encode(patch),
                            observed_at_ms: now_ms(),
                        })
                    })
                    .await
                    .ok()
                    .and_then(Result::ok);
                    let available = match evidence {
                        Some(evidence) => session
                            .record_workspace_change_set(&run_id, evidence)
                            .await
                            .is_ok(),
                        None => false,
                    };
                    let mut states = states.write().await;
                    if available {
                        states.remove(&run_id);
                    } else {
                        states.insert(run_id, ChangeSetCaptureState::Unavailable);
                    }
                });
                false
            }
            AgentRunSpawn::Replayed { .. } => true,
        }
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}
