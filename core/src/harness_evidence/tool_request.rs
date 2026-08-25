use super::{measure, require_digest, HarnessEvidenceError};
use serde::{Deserialize, Serialize};

pub const TOOL_REQUEST_SNAPSHOT_V1_SCHEMA: &str = "a3s.code.tool-request-snapshot.v1";

const TOOL_REQUEST_SNAPSHOT_DOMAIN: &str = "a3s.code.tool-request-snapshot.v1";
const TOOL_REQUEST_ID_DOMAIN: &str = "a3s.code.tool-request-id.v1";
const TOOL_REQUEST_NAME_DOMAIN: &str = "a3s.code.tool-request-name.v1";
const TOOL_REQUEST_ARGUMENTS_DOMAIN: &str = "a3s.code.tool-request-arguments.v1";

/// Runtime path that submitted a governed Tool request.
///
/// Version 1 keeps trusted and governed host control-plane requests distinct,
/// including their nested descendants, so replay consumers do not have to
/// infer authority from a Tool name or call identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRequestOriginV1 {
    Agent,
    Nested,
    HostDirectTrusted,
    HostDirectGoverned,
    HostDirectNestedTrusted,
    HostDirectNestedGoverned,
}

/// Immutable, bounded evidence for one validated Tool request.
///
/// The snapshot binds the correlation identifiers, origin, and exact JSON
/// arguments through domain-separated digests. It deliberately retains none
/// of those plaintext values; the surrounding event owns the Tool identifier
/// and name needed for lifecycle correlation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolRequestSnapshotV1 {
    pub schema: String,
    pub origin: ToolRequestOriginV1,
    pub tool_id_digest: String,
    pub tool_name_digest: String,
    pub arguments_bytes: u64,
    pub arguments_digest: String,
    pub snapshot_digest: String,
}

impl ToolRequestSnapshotV1 {
    pub(crate) fn capture(
        tool_id: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
        origin: ToolRequestOriginV1,
    ) -> Result<Self, HarnessEvidenceError> {
        let arguments = measure(TOOL_REQUEST_ARGUMENTS_DOMAIN, arguments)?;
        let mut snapshot = Self {
            schema: TOOL_REQUEST_SNAPSHOT_V1_SCHEMA.to_owned(),
            origin,
            tool_id_digest: measure(TOOL_REQUEST_ID_DOMAIN, tool_id)?.digest,
            tool_name_digest: measure(TOOL_REQUEST_NAME_DOMAIN, tool_name)?.digest,
            arguments_bytes: arguments.bytes,
            arguments_digest: arguments.digest,
            snapshot_digest: String::new(),
        };
        snapshot.snapshot_digest = snapshot.expected_digest()?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> Result<(), HarnessEvidenceError> {
        if self.schema != TOOL_REQUEST_SNAPSHOT_V1_SCHEMA {
            return Err(HarnessEvidenceError::UnsupportedSchema);
        }
        if self.arguments_bytes == 0 {
            return Err(HarnessEvidenceError::InvalidContents(
                "serialized Tool request arguments are non-empty",
            ));
        }
        for (field, digest) in [
            ("tool_id_digest", self.tool_id_digest.as_str()),
            ("tool_name_digest", self.tool_name_digest.as_str()),
            ("arguments_digest", self.arguments_digest.as_str()),
            ("snapshot_digest", self.snapshot_digest.as_str()),
        ] {
            require_digest(field, digest)?;
        }
        if self.snapshot_digest != self.expected_digest()? {
            return Err(HarnessEvidenceError::DigestMismatch("snapshot_digest"));
        }
        Ok(())
    }

    /// Validate this snapshot against the correlated Tool event and the exact
    /// post-hook arguments submitted to governance and execution.
    pub fn validate_against(
        &self,
        tool_id: &str,
        tool_name: &str,
        arguments: &serde_json::Value,
        origin: ToolRequestOriginV1,
    ) -> Result<(), HarnessEvidenceError> {
        self.validate()?;
        if self.origin != origin {
            return Err(HarnessEvidenceError::InvalidContents(
                "Tool request origins agree",
            ));
        }
        if self.tool_id_digest != measure(TOOL_REQUEST_ID_DOMAIN, tool_id)?.digest {
            return Err(HarnessEvidenceError::DigestMismatch("tool_id_digest"));
        }
        if self.tool_name_digest != measure(TOOL_REQUEST_NAME_DOMAIN, tool_name)?.digest {
            return Err(HarnessEvidenceError::DigestMismatch("tool_name_digest"));
        }
        let arguments = measure(TOOL_REQUEST_ARGUMENTS_DOMAIN, arguments)?;
        if self.arguments_digest != arguments.digest {
            return Err(HarnessEvidenceError::DigestMismatch("arguments_digest"));
        }
        if self.arguments_bytes != arguments.bytes {
            return Err(HarnessEvidenceError::InvalidContents(
                "Tool request argument byte measurements agree",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<String, HarnessEvidenceError> {
        #[derive(Serialize)]
        struct Identity<'a> {
            schema: &'a str,
            origin: ToolRequestOriginV1,
            tool_id_digest: &'a str,
            tool_name_digest: &'a str,
            arguments_bytes: u64,
            arguments_digest: &'a str,
        }

        Ok(measure(
            TOOL_REQUEST_SNAPSHOT_DOMAIN,
            &Identity {
                schema: &self.schema,
                origin: self.origin,
                tool_id_digest: &self.tool_id_digest,
                tool_name_digest: &self.tool_name_digest,
                arguments_bytes: self.arguments_bytes,
                arguments_digest: &self.arguments_digest,
            },
        )?
        .digest)
    }
}
