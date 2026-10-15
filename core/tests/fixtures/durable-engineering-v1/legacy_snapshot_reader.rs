// Frozen schema gate from store/session_snapshot.rs before T008 (schema 1).
// The old reader ignores added JSON fields; its version gate must reject bound snapshots.
use anyhow::{bail, Result};
use serde::Deserialize;
const SESSION_SNAPSHOT_SCHEMA_VERSION: u32 = 1;
#[derive(Deserialize)]
pub struct LegacySnapshotHeader {
    pub schema_version: u32,
}
impl LegacySnapshotHeader {
    pub fn ensure_loadable(&self) -> Result<()> {
        if self.schema_version != SESSION_SNAPSHOT_SCHEMA_VERSION {
            bail!(
                "unsupported session snapshot schema version {}; expected {}",
                self.schema_version,
                SESSION_SNAPSHOT_SCHEMA_VERSION
            );
        }
        Ok(())
    }
}
