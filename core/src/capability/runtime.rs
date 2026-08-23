use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use super::{
    CapabilityCatalog, CapabilityCeiling, CapabilityCommitReceipt, CapabilityId, CapabilityKind,
    CapabilityProjectionAdapter, CapabilityProjectionError, CapabilityProjectionLease,
    CapabilityScope, CapabilityScopeError, CapabilitySet, CapabilityTxn, CapabilityValue, Prepared,
    RetainedUseGeneration, Run, ScopeCloseReport, Session, Staged, UseCapabilityGeneration,
    Validated,
};

const MAX_USE_LEASE_ERROR_BYTES: usize = 1_024;

/// Bounded failure returned while retaining one exact A3S Use generation.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{message}")]
pub struct UseGenerationLeaseError {
    message: Box<str>,
}

impl UseGenerationLeaseError {
    pub fn new(message: impl Into<String>) -> Self {
        let message = message.into();
        let message = if message.is_empty() {
            "A3S Use generation lease acquisition failed".to_owned()
        } else {
            truncate_utf8(message, MAX_USE_LEASE_ERROR_BYTES)
        };
        Self {
            message: message.into_boxed_str(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Generation-bound host seam for the real non-clone A3S Use snapshot lease.
///
/// A provider is published in the same catalog CAS as its projection. Each Run
/// calls [`Self::acquire`] again so A3S Use can reject a generation that has
/// become hidden or stale since Code installed the projection. Implementations
/// must retain the concrete `a3s_use::CapabilitySnapshotLease` inside the
/// returned [`RetainedUseGeneration`] value.
#[async_trait]
pub trait UseGenerationLeaseProvider: Send + Sync + 'static {
    fn use_generation(&self) -> &UseCapabilityGeneration;

    async fn acquire(
        &self,
        cancellation: CancellationToken,
    ) -> Result<Box<dyn RetainedUseGeneration>, UseGenerationLeaseError>;
}

/// Session-host failure that cannot expose a partial capability generation.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum CapabilityRuntimeError {
    #[error(transparent)]
    Projection(#[from] CapabilityProjectionError),
    #[error(transparent)]
    Scope(#[from] CapabilityScopeError),
    #[error("A Session capability batch with an A3S Use cursor requires a lease provider")]
    MissingUseLeaseProvider,
    #[error("A Session capability batch without an A3S Use cursor cannot carry a lease provider")]
    UnexpectedUseLeaseProvider,
    #[error(
        "A3S Use lease provider does not match the batch cursor (generation {expected_generation} vs {actual_generation}, capability revision mismatch: {revision_mismatch}, Registry revision mismatch: {registry_revision_mismatch})"
    )]
    UseLeaseProviderMismatch {
        expected_generation: u64,
        actual_generation: u64,
        revision_mismatch: bool,
        registry_revision_mismatch: bool,
    },
    #[error("A3S Use generation lease acquisition failed: {message}")]
    UseLeaseAcquisition { message: String },
    #[error("Session capability Run admission was cancelled")]
    Cancelled,
    #[error("The owning Session is closed")]
    SessionClosed,
    #[error("Session capability kind '{kind}' is not migrated to the atomic host runtime")]
    UnsupportedSessionKind { kind: CapabilityKind },
    #[error("Session runtime {kind} name '{public_name}' conflicts with a compatibility value")]
    RuntimeNameConflict {
        kind: CapabilityKind,
        public_name: String,
    },
    #[error("Session runtime {kind} value '{public_name}' is invalid: {message}")]
    RuntimeValueInvalid {
        kind: CapabilityKind,
        public_name: String,
        message: String,
    },
    #[error(
        "Capability Run close was incomplete (tasks failed: {tasks_failed}, tasks timed out: {tasks_timed_out}, child scopes failed: {child_scopes_failed}, child scopes timed out: {child_scopes_timed_out}, effects failed: {effects_failed}, effects timed out: {effects_timed_out})"
    )]
    RunCloseIncomplete {
        tasks_failed: usize,
        tasks_timed_out: usize,
        child_scopes_failed: usize,
        child_scopes_timed_out: usize,
        effects_failed: usize,
        effects_timed_out: usize,
    },
}

/// One complete next-generation Tool/Skill/Agent projection for a Session.
///
/// The batch owns every adapter before preparation starts. A Use-backed batch
/// also owns the generation-specific lease provider that will be published in
/// the same catalog compare-and-swap as the prepared projection.
#[must_use = "a Session capability batch must be applied or dropped"]
pub struct SessionCapabilityBatch {
    target: Arc<CapabilitySet>,
    staged: BTreeMap<CapabilityId, Box<dyn CapabilityProjectionAdapter>>,
    use_lease_provider: Option<Arc<dyn UseGenerationLeaseProvider>>,
}

impl SessionCapabilityBatch {
    pub fn new(target: Arc<CapabilitySet>) -> Result<Self, CapabilityRuntimeError> {
        if target.use_capability_generation().is_some() {
            return Err(CapabilityRuntimeError::MissingUseLeaseProvider);
        }
        validate_session_kinds(&target)?;
        Ok(Self {
            target,
            staged: BTreeMap::new(),
            use_lease_provider: None,
        })
    }

    pub fn from_use_projection(
        target: Arc<CapabilitySet>,
        provider: Arc<dyn UseGenerationLeaseProvider>,
    ) -> Result<Self, CapabilityRuntimeError> {
        let expected = target
            .use_capability_generation()
            .ok_or(CapabilityRuntimeError::UnexpectedUseLeaseProvider)?;
        ensure_use_generation_matches(expected, provider.use_generation())?;
        validate_session_kinds(&target)?;
        Ok(Self {
            target,
            staged: BTreeMap::new(),
            use_lease_provider: Some(provider),
        })
    }

    pub fn target(&self) -> &CapabilitySet {
        &self.target
    }

    pub fn stage<A>(
        &mut self,
        id: CapabilityId,
        adapter: A,
    ) -> Result<&mut Self, CapabilityRuntimeError>
    where
        A: CapabilityProjectionAdapter,
    {
        self.stage_boxed(id, Box::new(adapter))
    }

    pub fn stage_value(
        &mut self,
        id: CapabilityId,
        value: CapabilityValue,
    ) -> Result<&mut Self, CapabilityRuntimeError> {
        struct ReadyValue(CapabilityValue);

        #[async_trait]
        impl CapabilityProjectionAdapter for ReadyValue {
            async fn prepare(
                self: Box<Self>,
                _cancellation: CancellationToken,
            ) -> Result<super::PreparedCapability, super::CapabilityAdapterError> {
                Ok(super::PreparedCapability::new(self.0))
            }
        }

        self.stage(id, ReadyValue(value))
    }

    pub fn len(&self) -> usize {
        self.staged.len()
    }

    pub fn is_empty(&self) -> bool {
        self.staged.is_empty()
    }

    fn stage_boxed(
        &mut self,
        id: CapabilityId,
        adapter: Box<dyn CapabilityProjectionAdapter>,
    ) -> Result<&mut Self, CapabilityRuntimeError> {
        if !self.target.contains(&id) {
            return Err(CapabilityProjectionError::UnknownStagedCapability {
                capability: id.to_string(),
            }
            .into());
        }
        if self.staged.insert(id.clone(), adapter).is_some() {
            return Err(CapabilityProjectionError::DuplicateStagedCapability {
                capability: id.to_string(),
            }
            .into());
        }
        Ok(self)
    }

    pub(crate) async fn prepare(
        self,
        catalog: &CapabilityCatalog,
        cancellation: CancellationToken,
    ) -> Result<PreparedSessionCapabilityBatch, CapabilityRuntimeError> {
        let Self {
            target,
            staged,
            use_lease_provider,
        } = self;
        let mut transaction: CapabilityTxn<Staged> = catalog.begin(target)?;
        for (id, adapter) in staged {
            transaction.stage_boxed(id, adapter)?;
        }
        let transaction: CapabilityTxn<Prepared> = transaction.prepare(cancellation).await?;
        let transaction: CapabilityTxn<Validated> = transaction.validate()?;
        Ok(PreparedSessionCapabilityBatch {
            transaction,
            use_lease_provider,
        })
    }
}

impl fmt::Debug for SessionCapabilityBatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionCapabilityBatch")
            .field("target_generation", &self.target.generation())
            .field("target_digest", &self.target.digest())
            .field("staged", &self.staged.len())
            .field("use_backed", &self.use_lease_provider.is_some())
            .finish()
    }
}

pub(crate) struct PreparedSessionCapabilityBatch {
    transaction: CapabilityTxn<Validated>,
    use_lease_provider: Option<Arc<dyn UseGenerationLeaseProvider>>,
}

impl PreparedSessionCapabilityBatch {
    pub(crate) fn projection(
        &self,
    ) -> Result<&super::CapabilityProjection, CapabilityRuntimeError> {
        self.transaction.projection().map_err(Into::into)
    }

    pub(crate) fn commit(self) -> Result<CapabilityCommitReceipt, CapabilityRuntimeError> {
        self.transaction
            .commit_with_use_lease_provider(self.use_lease_provider)
            .map_err(Into::into)
    }
}

/// Non-clone Run guard retaining one Code projection and one exact Use lease.
///
/// The Run and its generation-specific Session scope are closed together. The
/// A3S Use lease lives in the Run supervisor and is released after children,
/// tasks, and effects. The projection lease remains pinned until this guard is
/// dropped, so model definitions and execution borrow the same runtime values.
#[must_use = "a Session capability Run must remain alive for the complete execution"]
pub struct SessionCapabilityRun {
    run_scope: CapabilityScope<Run>,
    session_scope: CapabilityScope<Session>,
    projection: CapabilityProjectionLease,
}

impl SessionCapabilityRun {
    pub(crate) async fn admit(
        projection: CapabilityProjectionLease,
        session_local_id: &str,
        run_local_id: &str,
        ceiling: CapabilityCeiling,
        cancellation: CancellationToken,
    ) -> Result<Self, CapabilityRuntimeError> {
        if cancellation.is_cancelled() {
            return Err(CapabilityRuntimeError::Cancelled);
        }
        let set = projection.projection().set();
        let session_scope = CapabilityScope::new_session(
            session_local_id,
            Arc::clone(projection.projection().set_arc()),
            ceiling.clone(),
        )?;

        let run_scope = match set.use_capability_generation() {
            Some(expected) => {
                let provider = projection
                    .use_lease_provider()
                    .ok_or(CapabilityRuntimeError::MissingUseLeaseProvider)?;
                ensure_use_generation_matches(expected, provider.use_generation())?;
                let acquire = provider.acquire(cancellation.clone());
                tokio::pin!(acquire);
                let lease = tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => {
                        return Err(CapabilityRuntimeError::Cancelled);
                    }
                    result = &mut acquire => result.map_err(|error| {
                        CapabilityRuntimeError::UseLeaseAcquisition {
                            message: error.message().to_owned(),
                        }
                    })?,
                };
                session_scope.admit_use_run(run_local_id, ceiling, lease)?
            }
            None => {
                if projection.use_lease_provider().is_some() {
                    return Err(CapabilityRuntimeError::UnexpectedUseLeaseProvider);
                }
                session_scope.admit_run(run_local_id, ceiling)?
            }
        };

        Ok(Self {
            run_scope,
            session_scope,
            projection,
        })
    }

    pub fn projection(&self) -> &super::CapabilityProjection {
        self.projection.projection()
    }

    pub fn run_scope(&self) -> &CapabilityScope<Run> {
        &self.run_scope
    }

    pub async fn close(&self) -> Result<ScopeCloseReport, CapabilityRuntimeError> {
        let report = self.session_scope.close().await?;
        if !report.is_clean() {
            return Err(CapabilityRuntimeError::RunCloseIncomplete {
                tasks_failed: report.tasks_failed,
                tasks_timed_out: report.tasks_timed_out,
                child_scopes_failed: report.child_scopes_failed,
                child_scopes_timed_out: report.child_scopes_timed_out,
                effects_failed: report.effects_failed,
                effects_timed_out: report.effects_timed_out,
            });
        }
        Ok(report)
    }
}

impl fmt::Debug for SessionCapabilityRun {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionCapabilityRun")
            .field("stamp", self.projection.stamp())
            .field("run_scope", &self.run_scope.id())
            .finish_non_exhaustive()
    }
}

fn validate_session_kinds(target: &CapabilitySet) -> Result<(), CapabilityRuntimeError> {
    for (_, descriptor) in target.iter() {
        if !matches!(
            descriptor.id().kind(),
            CapabilityKind::Tool | CapabilityKind::Skill | CapabilityKind::Agent
        ) {
            return Err(CapabilityRuntimeError::UnsupportedSessionKind {
                kind: descriptor.id().kind(),
            });
        }
    }
    Ok(())
}

fn ensure_use_generation_matches(
    expected: &UseCapabilityGeneration,
    actual: &UseCapabilityGeneration,
) -> Result<(), CapabilityRuntimeError> {
    if expected == actual {
        return Ok(());
    }
    Err(CapabilityRuntimeError::UseLeaseProviderMismatch {
        expected_generation: expected.generation(),
        actual_generation: actual.generation(),
        revision_mismatch: expected.revision() != actual.revision(),
        registry_revision_mismatch: expected.registry_revision() != actual.registry_revision(),
    })
}

fn truncate_utf8(mut value: String, max: usize) -> String {
    if value.len() <= max {
        return value;
    }
    let mut boundary = max;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
    value
}
