use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::projection::CatalogInner;
use super::{
    CapabilityAdapterError, CapabilityCatalog, CapabilityCatalogStamp, CapabilityCommitReceipt,
    CapabilityEffect, CapabilityId, CapabilityProjection, CapabilityProjectionError,
    CapabilityReadinessPlan, CapabilitySet, CapabilityValue,
};

pub const MAX_CAPABILITY_TRANSACTION_EFFECTS: usize = 4_096;

/// Transaction state before fallible runtime preparation.
#[derive(Debug)]
pub struct Staged;

/// Transaction state after every adapter prepared successfully.
#[derive(Debug)]
pub struct Prepared;

/// Transaction state after the complete value projection passed validation.
#[derive(Debug)]
pub struct Validated;

/// One atomically returned runtime value and its reversible resources.
///
/// Adapters must return acquired resources in the same successful value. If
/// their future is cancelled before returning, they remain responsible for
/// cancellation-safe local RAII cleanup. Once returned, the transaction owns
/// every effect and transfers it to the catalog rollback queue on any failure.
#[must_use = "prepared capability effects must be transferred into a transaction"]
pub struct PreparedCapability {
    value: CapabilityValue,
    effects: Vec<Box<dyn CapabilityEffect>>,
}

impl PreparedCapability {
    pub fn new(value: CapabilityValue) -> Self {
        Self {
            value,
            effects: Vec::new(),
        }
    }

    pub fn push_effect<E>(&mut self, effect: E) -> Result<(), CapabilityAdapterError>
    where
        E: CapabilityEffect,
    {
        self.push_boxed_effect(Box::new(effect))
    }

    pub fn push_boxed_effect(
        &mut self,
        effect: Box<dyn CapabilityEffect>,
    ) -> Result<(), CapabilityAdapterError> {
        // The transaction checks the aggregate immediately after an adapter
        // returns. Keeping this append infallible ensures an oversized batch
        // is first transferred into catalog-owned rollback instead of dropping
        // already acquired effects inside the adapter.
        self.effects.push(effect);
        Ok(())
    }

    fn into_parts(self) -> (CapabilityValue, Vec<Box<dyn CapabilityEffect>>) {
        (self.value, self.effects)
    }
}

/// Surface-owned fallible preparation boundary.
///
/// Tool, Skill, MCP, and other concerns implement this trait beside their
/// native runtime types. The adapter does not resolve packages or dependencies;
/// it projects one descriptor from the already selected A3S Use snapshot. A
/// successful return is the surface readiness barrier: the adapter must not
/// report success while its value still depends on unfinished initialization.
#[async_trait]
pub trait CapabilityProjectionAdapter: Send + 'static {
    async fn prepare(
        self: Box<Self>,
        cancellation: CancellationToken,
    ) -> Result<PreparedCapability, CapabilityAdapterError>;
}

struct ReadyValueAdapter(CapabilityValue);

#[async_trait]
impl CapabilityProjectionAdapter for ReadyValueAdapter {
    async fn prepare(
        self: Box<Self>,
        _cancellation: CancellationToken,
    ) -> Result<PreparedCapability, CapabilityAdapterError> {
        Ok(PreparedCapability::new(self.0))
    }
}

struct TransactionBody {
    catalog: Arc<CatalogInner>,
    base: CapabilityCatalogStamp,
    target: Arc<CapabilitySet>,
    readiness: Arc<CapabilityReadinessPlan>,
    effects: Vec<Box<dyn CapabilityEffect>>,
    rollback_armed: bool,
}

impl Drop for TransactionBody {
    fn drop(&mut self) {
        if self.rollback_armed {
            let effects = std::mem::take(&mut self.effects);
            self.catalog.enqueue_rollback(effects);
        }
    }
}

/// Atomic capability contribution transaction guarded by Rust typestate.
///
/// Only [`CapabilityTxn<Validated>`] exposes `commit`. A prepared transaction
/// cannot publish, and dropping any uncommitted state transfers all completed
/// effects to the catalog-owned asynchronous rollback queue.
///
/// ```compile_fail
/// use a3s_code_core::capability::{CapabilityTxn, Prepared};
///
/// fn publish_without_validation(txn: CapabilityTxn<Prepared>) {
///     let _ = txn.commit();
/// }
/// ```
#[must_use = "capability transactions must be committed or drained as rollback"]
pub struct CapabilityTxn<S> {
    body: Option<TransactionBody>,
    staged: BTreeMap<CapabilityId, Box<dyn CapabilityProjectionAdapter>>,
    prepared: BTreeMap<CapabilityId, CapabilityValue>,
    projection: Option<Arc<CapabilityProjection>>,
    _state: PhantomData<S>,
}

impl CapabilityCatalog {
    pub fn begin(
        &self,
        target: Arc<CapabilitySet>,
    ) -> Result<CapabilityTxn<Staged>, CapabilityProjectionError> {
        let base = self.current_stamp();
        let expected = base
            .generation()
            .checked_next()
            .ok_or(CapabilityProjectionError::GenerationExhausted)?;
        if target.generation() != expected {
            return Err(CapabilityProjectionError::TargetGenerationMismatch {
                expected: expected.get(),
                actual: target.generation().get(),
            });
        }
        let readiness = Arc::new(CapabilityReadinessPlan::from_set(&target)?);
        Ok(CapabilityTxn {
            body: Some(TransactionBody {
                catalog: Arc::clone(&self.inner),
                base,
                target,
                readiness,
                effects: Vec::new(),
                rollback_armed: true,
            }),
            staged: BTreeMap::new(),
            prepared: BTreeMap::new(),
            projection: None,
            _state: PhantomData,
        })
    }
}

impl CapabilityTxn<Staged> {
    pub fn stage<A>(
        &mut self,
        id: CapabilityId,
        adapter: A,
    ) -> Result<&mut Self, CapabilityProjectionError>
    where
        A: CapabilityProjectionAdapter,
    {
        let body = self
            .body
            .as_ref()
            .ok_or(CapabilityProjectionError::InvalidTransactionState)?;
        if !body.target.contains(&id) {
            return Err(CapabilityProjectionError::UnknownStagedCapability {
                capability: id.to_string(),
            });
        }
        if self.staged.contains_key(&id) {
            return Err(CapabilityProjectionError::DuplicateStagedCapability {
                capability: id.to_string(),
            });
        }
        self.staged.insert(id, Box::new(adapter));
        Ok(self)
    }

    pub fn stage_value(
        &mut self,
        id: CapabilityId,
        value: CapabilityValue,
    ) -> Result<&mut Self, CapabilityProjectionError> {
        self.stage(id, ReadyValueAdapter(value))
    }

    pub async fn prepare(
        mut self,
        cancellation: CancellationToken,
    ) -> Result<CapabilityTxn<Prepared>, CapabilityProjectionError> {
        if cancellation.is_cancelled() {
            return Err(CapabilityProjectionError::Cancelled);
        }
        let body = self
            .body
            .as_ref()
            .ok_or(CapabilityProjectionError::InvalidTransactionState)?;
        if let Some((id, _)) = body
            .target
            .iter()
            .find(|(id, _)| !self.staged.contains_key(*id))
        {
            return Err(CapabilityProjectionError::MissingStagedCapability {
                capability: id.to_string(),
            });
        }
        let activation_order = body.readiness.activation_order().to_vec();
        for id in activation_order {
            let adapter = self.staged.remove(&id).ok_or_else(|| {
                CapabilityProjectionError::MissingStagedCapability {
                    capability: id.to_string(),
                }
            })?;
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    return Err(CapabilityProjectionError::Cancelled);
                }
                result = adapter.prepare(cancellation.clone()) => result,
            };
            let prepared = result.map_err(|error| CapabilityProjectionError::PrepareFailed {
                capability: id.to_string(),
                message: error.message().to_owned(),
            })?;
            let (value, mut effects) = prepared.into_parts();
            let body = self
                .body
                .as_mut()
                .ok_or(CapabilityProjectionError::InvalidTransactionState)?;
            body.effects.append(&mut effects);
            if body.effects.len() > MAX_CAPABILITY_TRANSACTION_EFFECTS {
                return Err(CapabilityProjectionError::EffectBoundExceeded {
                    max: MAX_CAPABILITY_TRANSACTION_EFFECTS,
                });
            }
            self.prepared.insert(id, value);
        }
        if cancellation.is_cancelled() {
            return Err(CapabilityProjectionError::Cancelled);
        }
        self.transition()
    }
}

impl CapabilityTxn<Prepared> {
    pub fn validate(mut self) -> Result<CapabilityTxn<Validated>, CapabilityProjectionError> {
        let target = Arc::clone(
            &self
                .body
                .as_ref()
                .ok_or(CapabilityProjectionError::InvalidTransactionState)?
                .target,
        );
        let readiness = Arc::clone(
            &self
                .body
                .as_ref()
                .ok_or(CapabilityProjectionError::InvalidTransactionState)?
                .readiness,
        );
        let values = std::mem::take(&mut self.prepared);
        self.projection = Some(CapabilityProjection::with_readiness(
            target, readiness, values,
        )?);
        self.transition()
    }
}

impl CapabilityTxn<Validated> {
    pub fn commit(mut self) -> Result<CapabilityCommitReceipt, CapabilityProjectionError> {
        let projection = self
            .projection
            .take()
            .ok_or(CapabilityProjectionError::InvalidTransactionState)?;
        let mut body = self
            .body
            .take()
            .ok_or(CapabilityProjectionError::InvalidTransactionState)?;
        let effects = std::mem::take(&mut body.effects);
        let result = body.catalog.publish(&body.base, projection, effects);
        // `publish` owns the effect batch on both success and CAS conflict.
        body.rollback_armed = false;
        result
    }
}

impl<S> CapabilityTxn<S> {
    fn transition<T>(mut self) -> Result<CapabilityTxn<T>, CapabilityProjectionError> {
        let body = self
            .body
            .take()
            .ok_or(CapabilityProjectionError::InvalidTransactionState)?;
        Ok(CapabilityTxn {
            body: Some(body),
            staged: std::mem::take(&mut self.staged),
            prepared: std::mem::take(&mut self.prepared),
            projection: self.projection.take(),
            _state: PhantomData,
        })
    }

    pub fn base(&self) -> Result<&CapabilityCatalogStamp, CapabilityProjectionError> {
        self.body
            .as_ref()
            .map(|body| &body.base)
            .ok_or(CapabilityProjectionError::InvalidTransactionState)
    }

    pub fn target(&self) -> Result<&CapabilitySet, CapabilityProjectionError> {
        self.body
            .as_ref()
            .map(|body| body.target.as_ref())
            .ok_or(CapabilityProjectionError::InvalidTransactionState)
    }
}

impl<S> fmt::Debug for CapabilityTxn<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityTxn")
            .field("base", &self.body.as_ref().map(|body| &body.base))
            .field(
                "target_generation",
                &self.body.as_ref().map(|body| body.target.generation()),
            )
            .field("staged", &self.staged.len())
            .field("prepared", &self.prepared.len())
            .field("validated", &self.projection.is_some())
            .finish()
    }
}
