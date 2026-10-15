// Frozen original schema1 reader gate; the remaining old payload fields are not needed by it.
use serde::Deserialize;
const LOOP_CHECKPOINT_SCHEMA_VERSION: u32 = 1;
#[derive(Deserialize)]
pub struct LegacyLoopReader {
    schema_version: u32,
    run_id: String,
    capability_binding: Option<crate::capability::RunCapabilityBindingV1>,
    standing_context_digest: Option<String>,
}
impl LegacyLoopReader {
    pub fn ensure_loadable(&self) -> anyhow::Result<()> {
        if self.schema_version > LOOP_CHECKPOINT_SCHEMA_VERSION {
            anyhow::bail!(
                "loop checkpoint for run {} has schema version {} but this build supports at \
                 most {}; refusing to resume from an incompatible future checkpoint",
                self.run_id,
                self.schema_version,
                LOOP_CHECKPOINT_SCHEMA_VERSION
            );
        }
        if let Some(binding) = &self.capability_binding {
            binding.validate().map_err(|error| {
                anyhow::anyhow!(
                    "loop checkpoint for run {} has an invalid capability binding: {error}",
                    self.run_id
                )
            })?;
        }
        if let Some(digest) = &self.standing_context_digest {
            crate::content_digest::validate_digest(digest).map_err(|_| {
                anyhow::anyhow!(
                    "loop checkpoint for run {} has an invalid standing context digest",
                    self.run_id
                )
            })?;
        }
        Ok(())
    }
}
