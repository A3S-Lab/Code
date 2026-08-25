//! Shared admission boundary for host-consumed projected capability values.

use std::sync::Arc;

use super::AgentSession;
use crate::capability::{
    CapabilityDescriptor, CapabilityKind, CapabilityRuntimeError, CapabilityValue,
    SessionCapabilityRun,
};
use crate::error::Result;

pub(super) struct AdmittedHostCapability<T> {
    pub(super) descriptor: CapabilityDescriptor,
    pub(super) value: Arc<T>,
    pub(super) capability_run: SessionCapabilityRun,
}

impl AgentSession {
    /// Linearize host lookup, projection pinning, Use lease admission, and
    /// Session close for one non-model-visible capability category.
    pub(super) async fn admit_projected_host_capability<T>(
        &self,
        kind: CapabilityKind,
        public_name: &str,
        select: impl Fn(&CapabilityValue) -> Option<Arc<T>>,
    ) -> Result<Option<AdmittedHostCapability<T>>>
    where
        T: Send + Sync + 'static,
    {
        let (projection, descriptor, value, ceiling) = {
            let _admission = self
                .close_handle
                .immediate_extension_mutation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.is_closed() {
                return Err(CapabilityRuntimeError::SessionClosed.into());
            }
            let projection = self.capability_catalog.pin();
            let descriptor = projection
                .projection()
                .set()
                .iter()
                .find_map(|(_, descriptor)| {
                    (descriptor.id().kind() == kind && descriptor.public_name() == public_name)
                        .then(|| descriptor.clone())
                });
            let Some(descriptor) = descriptor else {
                return Ok(None);
            };
            let value = projection
                .projection()
                .iter()
                .find_map(|(id, value)| {
                    if id == descriptor.id() {
                        select(value)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| CapabilityRuntimeError::RuntimeValueInvalid {
                    kind,
                    public_name: public_name.to_owned(),
                    message: "the projected host value lost its immutable runtime binding"
                        .to_owned(),
                })?;
            let ceiling = self.capability_run_ceiling(projection.projection().set())?;
            (projection, descriptor, value, ceiling)
        };

        let run_local_id = format!("{}-{}", kind.as_str(), uuid::Uuid::new_v4());
        let capability_run = SessionCapabilityRun::admit(
            projection,
            "active",
            &run_local_id,
            ceiling,
            self.session_cancel.child_token(),
        )
        .await?;
        let closed_during_admission = {
            let _admission = self
                .close_handle
                .immediate_extension_mutation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.is_closed()
        };
        if closed_during_admission {
            if let Err(error) = capability_run.close().await {
                tracing::warn!(
                    error = %error,
                    kind = %kind,
                    "Projected host capability close failed after Session close won admission"
                );
            }
            return Err(CapabilityRuntimeError::SessionClosed.into());
        }

        Ok(Some(AdmittedHostCapability {
            descriptor,
            value,
            capability_run,
        }))
    }
}
