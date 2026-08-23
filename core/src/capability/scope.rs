use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::supervisor::{
    remove_registered_child, EffectSupervisor, SupervisedChild, SupervisorInner,
};
use super::{
    CapabilityCeiling, CapabilityEffect, CapabilityEffectError, CapabilityLease,
    CapabilityScopeError, CapabilitySet, RetainedUseGeneration, ScopeClosePolicy, ScopeCloseReport,
    Sha256Digest, SupervisedTaskId, UseCapabilityGeneration, MAX_CAPABILITY_IDENTIFIER_BYTES,
};

/// Closed scope hierarchy used by the capability lifecycle kernel.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityScopeKind {
    Session,
    Run,
    Turn,
    Subtask,
}

impl CapabilityScopeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Run => "run",
            Self::Turn => "turn",
            Self::Subtask => "subtask",
        }
    }
}

impl fmt::Display for CapabilityScopeKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

mod sealed {
    pub trait Sealed {}
}

/// Sealed marker implemented only by the four supported scope levels.
pub trait ScopeKind: sealed::Sealed + Send + Sync + 'static {
    const KIND: CapabilityScopeKind;
}

#[derive(Debug)]
pub struct Session;

#[derive(Debug)]
pub struct Run;

#[derive(Debug)]
pub struct Turn;

#[derive(Debug)]
pub struct Subtask;

impl sealed::Sealed for Session {}
impl sealed::Sealed for Run {}
impl sealed::Sealed for Turn {}
impl sealed::Sealed for Subtask {}

impl ScopeKind for Session {
    const KIND: CapabilityScopeKind = CapabilityScopeKind::Session;
}

impl ScopeKind for Run {
    const KIND: CapabilityScopeKind = CapabilityScopeKind::Run;
}

impl ScopeKind for Turn {
    const KIND: CapabilityScopeKind = CapabilityScopeKind::Turn;
}

impl ScopeKind for Subtask {
    const KIND: CapabilityScopeKind = CapabilityScopeKind::Subtask;
}

/// Canonical hierarchical scope identity assigned by the Code host.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CapabilityScopeId(Box<str>);

impl CapabilityScopeId {
    fn root(
        kind: CapabilityScopeKind,
        local_id: impl Into<String>,
    ) -> Result<Self, CapabilityScopeError> {
        let local_id = local_id.into();
        validate_scope_local_id(&local_id)?;
        Self::from_complete(format!("{kind}/{local_id}"))
    }

    fn child(
        parent: &Self,
        kind: CapabilityScopeKind,
        local_id: impl Into<String>,
    ) -> Result<Self, CapabilityScopeError> {
        let local_id = local_id.into();
        validate_scope_local_id(&local_id)?;
        Self::from_complete(format!("{}/{kind}/{local_id}", parent.as_str()))
    }

    fn from_complete(value: String) -> Result<Self, CapabilityScopeError> {
        if value.len() > MAX_CAPABILITY_IDENTIFIER_BYTES {
            return Err(CapabilityScopeError::BoundExceeded {
                field: "scope_id",
                max: MAX_CAPABILITY_IDENTIFIER_BYTES,
            });
        }
        Ok(Self(value.into_boxed_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CapabilityScopeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

struct ParentRegistration {
    supervisor: Weak<SupervisorInner>,
    id: u64,
}

pub(super) struct ScopeInner {
    id: CapabilityScopeId,
    kind: CapabilityScopeKind,
    parent_id: Option<CapabilityScopeId>,
    set: Arc<CapabilitySet>,
    ceiling: CapabilityCeiling,
    use_generation: Option<UseCapabilityGeneration>,
    supervisor: EffectSupervisor,
    parent_registration: Mutex<Option<ParentRegistration>>,
}

impl ScopeInner {
    pub(super) fn id(&self) -> &CapabilityScopeId {
        &self.id
    }

    pub(super) fn parent_id(&self) -> Option<&CapabilityScopeId> {
        self.parent_id.as_ref()
    }

    pub(super) fn set(&self) -> &CapabilitySet {
        &self.set
    }

    pub(super) fn ceiling(&self) -> &CapabilityCeiling {
        &self.ceiling
    }

    pub(super) fn use_generation(&self) -> Option<&UseCapabilityGeneration> {
        self.use_generation.as_ref()
    }

    pub(super) fn supervisor_cancellation(&self) -> CancellationToken {
        self.supervisor.cancellation()
    }

    pub(super) fn ensure_active(&self) -> Result<(), CapabilityScopeError> {
        if !self.supervisor.is_open() {
            return Err(CapabilityScopeError::ScopeInactive {
                scope_id: self.id.to_string(),
            });
        }
        Ok(())
    }

    async fn close(&self) -> Result<ScopeCloseReport, CapabilityScopeError> {
        let report = self.supervisor.close().await?;
        self.detach_from_parent();
        Ok(report)
    }

    fn set_parent_registration(&self, supervisor: Weak<SupervisorInner>, id: u64) {
        *self
            .parent_registration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(ParentRegistration { supervisor, id });
    }

    fn detach_from_parent(&self) {
        let registration = self
            .parent_registration
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(registration) = registration {
            remove_registered_child(&registration.supervisor, registration.id);
        }
    }
}

impl fmt::Debug for ScopeInner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScopeInner")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("parent_id", &self.parent_id)
            .field("catalog_digest", &self.set.digest())
            .field("ceiling", &self.ceiling)
            .field("use_generation", &self.use_generation)
            .field("active", &self.supervisor.is_open())
            .finish()
    }
}

struct ChildScopeOwner {
    inner: Arc<ScopeInner>,
}

#[async_trait]
impl SupervisedChild for ChildScopeOwner {
    fn name(&self) -> &str {
        self.inner.id.as_str()
    }

    fn cancel(&self) {
        self.inner.supervisor.cancel();
    }

    async fn close(self: Box<Self>) -> Result<ScopeCloseReport, CapabilityScopeError> {
        self.inner.close().await
    }
}

/// Typed immutable catalog and monotonic governance scope.
///
/// The value is deliberately not `Clone`: its owner must close it explicitly.
/// Borrowed [`CapabilityLease`] values carry the marker type and cannot outlive
/// this owner. Dropping the owner synchronously cancels and aborts supervised
/// tasks; it never starts asynchronous cleanup.
#[must_use = "capability scopes own effects and must be closed explicitly"]
pub struct CapabilityScope<K: ScopeKind> {
    inner: Arc<ScopeInner>,
    _kind: PhantomData<K>,
}

impl<K: ScopeKind> fmt::Debug for CapabilityScope<K> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityScope")
            .field("marker", &K::KIND)
            .field("inner", &self.inner)
            .finish()
    }
}

impl CapabilityScope<Session> {
    pub fn new_session(
        local_id: impl Into<String>,
        set: Arc<CapabilitySet>,
        ceiling: CapabilityCeiling,
    ) -> Result<Self, CapabilityScopeError> {
        Self::new_session_with_close_policy(local_id, set, ceiling, ScopeClosePolicy::default())
    }

    pub fn new_session_with_close_policy(
        local_id: impl Into<String>,
        set: Arc<CapabilitySet>,
        ceiling: CapabilityCeiling,
        close_policy: ScopeClosePolicy,
    ) -> Result<Self, CapabilityScopeError> {
        if ceiling.catalog_digest() != set.digest() {
            return Err(CapabilityScopeError::CeilingCatalogMismatch);
        }
        let id = CapabilityScopeId::root(CapabilityScopeKind::Session, local_id)?;
        let cancellation = CancellationToken::new();
        let supervisor = EffectSupervisor::new(id.to_string(), cancellation, close_policy);
        let use_generation = set.use_capability_generation().cloned();
        Ok(Self {
            inner: Arc::new(ScopeInner {
                id,
                kind: CapabilityScopeKind::Session,
                parent_id: None,
                set,
                ceiling,
                use_generation,
                supervisor,
                parent_registration: Mutex::new(None),
            }),
            _kind: PhantomData,
        })
    }

    /// Admit a Run for a catalog without an upstream A3S Use cursor.
    pub fn admit_run(
        &self,
        local_id: impl Into<String>,
        ceiling: CapabilityCeiling,
    ) -> Result<CapabilityScope<Run>, CapabilityScopeError> {
        if self.inner.use_generation.is_some() {
            return Err(CapabilityScopeError::MissingUseGenerationLease);
        }
        self.create_child(local_id, ceiling, None, None)
    }

    /// Admit a Run while retaining the exact, non-clone A3S Use generation
    /// lease for the complete Run lifetime.
    pub fn admit_use_run<L>(
        &self,
        local_id: impl Into<String>,
        ceiling: CapabilityCeiling,
        lease: L,
    ) -> Result<CapabilityScope<Run>, CapabilityScopeError>
    where
        L: RetainedUseGeneration,
    {
        let Some(expected) = self.inner.use_generation.as_ref() else {
            return Err(CapabilityScopeError::UnexpectedUseGenerationLease);
        };
        let actual = lease.use_generation();
        if actual != expected {
            return Err(CapabilityScopeError::UseGenerationLeaseMismatch {
                expected_generation: expected.generation(),
                actual_generation: actual.generation(),
                revision_mismatch: expected.revision() != actual.revision(),
                registry_revision_mismatch: expected.registry_revision()
                    != actual.registry_revision(),
            });
        }
        self.create_child(
            local_id,
            ceiling,
            Some(Box::new(lease)),
            Some(expected.clone()),
        )
    }
}

impl CapabilityScope<Run> {
    pub fn turn(
        &self,
        local_id: impl Into<String>,
        ceiling: CapabilityCeiling,
    ) -> Result<CapabilityScope<Turn>, CapabilityScopeError> {
        self.create_child(local_id, ceiling, None, self.inner.use_generation.clone())
    }

    pub fn subtask(
        &self,
        local_id: impl Into<String>,
        ceiling: CapabilityCeiling,
    ) -> Result<CapabilityScope<Subtask>, CapabilityScopeError> {
        self.create_child(local_id, ceiling, None, self.inner.use_generation.clone())
    }
}

impl CapabilityScope<Turn> {
    pub fn subtask(
        &self,
        local_id: impl Into<String>,
        ceiling: CapabilityCeiling,
    ) -> Result<CapabilityScope<Subtask>, CapabilityScopeError> {
        self.create_child(local_id, ceiling, None, self.inner.use_generation.clone())
    }
}

impl<K: ScopeKind> CapabilityScope<K> {
    pub fn id(&self) -> &CapabilityScopeId {
        &self.inner.id
    }

    pub const fn kind(&self) -> CapabilityScopeKind {
        K::KIND
    }

    pub fn parent_id(&self) -> Option<&CapabilityScopeId> {
        self.inner.parent_id.as_ref()
    }

    pub fn catalog_digest(&self) -> &Sha256Digest {
        self.inner.set.digest()
    }

    pub fn ceiling(&self) -> &CapabilityCeiling {
        &self.inner.ceiling
    }

    pub fn use_generation(&self) -> Option<&UseCapabilityGeneration> {
        self.inner.use_generation.as_ref()
    }

    pub fn is_active(&self) -> bool {
        self.inner.supervisor.is_open()
    }

    pub fn cancellation(&self) -> CancellationToken {
        self.inner.supervisor.cancellation()
    }

    pub fn lease(&self) -> Result<CapabilityLease<'_, K>, CapabilityScopeError> {
        self.inner.ensure_active()?;
        Ok(CapabilityLease::new(&self.inner))
    }

    pub fn register_effect<E>(&self, effect: E) -> Result<(), CapabilityScopeError>
    where
        E: CapabilityEffect,
    {
        self.inner.supervisor.register_effect(Box::new(effect))
    }

    pub fn spawn_task<F>(
        &self,
        name: impl Into<String>,
        task: F,
    ) -> Result<SupervisedTaskId, CapabilityScopeError>
    where
        F: Future<Output = Result<(), CapabilityEffectError>> + Send + 'static,
    {
        self.inner.supervisor.spawn_task(name, task)
    }

    pub fn cancel(&self) {
        self.inner.supervisor.cancel();
    }

    pub async fn close(&self) -> Result<ScopeCloseReport, CapabilityScopeError> {
        self.inner.close().await
    }

    fn create_child<C: ScopeKind>(
        &self,
        local_id: impl Into<String>,
        ceiling: CapabilityCeiling,
        generation_lease: Option<Box<dyn RetainedUseGeneration>>,
        use_generation: Option<UseCapabilityGeneration>,
    ) -> Result<CapabilityScope<C>, CapabilityScopeError> {
        self.inner.ensure_active()?;
        ceiling.ensure_within(&self.inner.ceiling)?;
        let id = CapabilityScopeId::child(&self.inner.id, C::KIND, local_id)?;
        let cancellation = self.inner.supervisor.cancellation().child_token();
        let supervisor =
            EffectSupervisor::new(id.to_string(), cancellation, self.inner.supervisor.policy());
        if let Some(lease) = generation_lease {
            supervisor.register_generation_lease(lease)?;
        }
        let child = Arc::new(ScopeInner {
            id,
            kind: C::KIND,
            parent_id: Some(self.inner.id.clone()),
            set: Arc::clone(&self.inner.set),
            ceiling,
            use_generation,
            supervisor,
            parent_registration: Mutex::new(None),
        });
        let registration_id = self
            .inner
            .supervisor
            .register_child(Box::new(ChildScopeOwner {
                inner: Arc::clone(&child),
            }))?;
        child.set_parent_registration(self.inner.supervisor.downgrade(), registration_id);
        Ok(CapabilityScope {
            inner: child,
            _kind: PhantomData,
        })
    }
}

impl<K: ScopeKind> Drop for CapabilityScope<K> {
    fn drop(&mut self) {
        self.inner.supervisor.cancel();
    }
}

fn validate_scope_local_id(value: &str) -> Result<(), CapabilityScopeError> {
    if value.is_empty() {
        return Err(CapabilityScopeError::InvalidScopeId {
            reason: "it is empty",
        });
    }
    if value.len() > MAX_CAPABILITY_IDENTIFIER_BYTES {
        return Err(CapabilityScopeError::BoundExceeded {
            field: "scope_local_id",
            max: MAX_CAPABILITY_IDENTIFIER_BYTES,
        });
    }
    if !value.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
    }) || !value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        || !value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(CapabilityScopeError::InvalidScopeId {
            reason: "it contains non-canonical characters or boundaries",
        });
    }
    Ok(())
}
