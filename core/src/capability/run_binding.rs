use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use super::{
    CapabilityCeiling, CapabilitySet, Sha256Digest, UseCapabilityGeneration,
    CAPABILITY_CEILING_SCHEMA, CAPABILITY_SET_SCHEMA, MAX_CAPABILITY_CANONICAL_BYTES,
    USE_CAPABILITY_SNAPSHOT_CURSOR_SCHEMA,
};

pub const RUN_CAPABILITY_BINDING_SCHEMA: &str = "a3s.code.run-capability-binding.v1";
pub const CAPABILITY_CEILING_DIGEST_DOMAIN: &str = "a3s.code.capability-ceiling-digest.v1";

const CAPABILITY_CEILING_DIGEST_PREFIX: &[u8] = b"a3s-code-capability-ceiling\0";

/// Stable validation and comparison failures for a persisted Run capability
/// identity.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RunCapabilityBindingError {
    #[error("invalid Run capability binding field '{field}': {message}")]
    InvalidField {
        field: &'static str,
        message: String,
    },
    #[error("Run capability binding cannot encode its authority ceiling: {0}")]
    Encoding(String),
    #[error(
        "Run capability binding drift (catalog generation {expected_generation} vs {actual_generation}, catalog digest mismatch: {catalog_digest_mismatch}, ceiling digest mismatch: {ceiling_digest_mismatch}, A3S Use generation mismatch: {use_generation_mismatch})"
    )]
    ContentDrift {
        expected_generation: u64,
        actual_generation: u64,
        catalog_digest_mismatch: bool,
        ceiling_digest_mismatch: bool,
        use_generation_mismatch: bool,
    },
}

/// Diagnostic copy of the exact upstream A3S Use snapshot cursor retained by
/// the Run. The catalog digest already binds these fields; keeping them
/// explicit lets a recovery host request the correct generation without
/// resolving `latest`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunUseCapabilityGenerationV1 {
    schema: String,
    generation: u64,
    revision: String,
    registry_revision: String,
}

impl RunUseCapabilityGenerationV1 {
    fn from_generation(generation: &UseCapabilityGeneration) -> Self {
        Self {
            schema: generation.schema().to_owned(),
            generation: generation.generation(),
            revision: generation.revision().to_string(),
            registry_revision: generation.registry_revision().to_string(),
        }
    }

    pub fn validate(&self) -> Result<(), RunCapabilityBindingError> {
        ensure_schema(
            "useGeneration.schema",
            &self.schema,
            USE_CAPABILITY_SNAPSHOT_CURSOR_SCHEMA,
        )?;
        ensure_digest("useGeneration.revision", &self.revision)?;
        ensure_digest("useGeneration.registryRevision", &self.registry_revision)?;
        Ok(())
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn registry_revision(&self) -> &str {
        &self.registry_revision
    }
}

/// Serializable identity of the complete immutable capability generation and
/// authority ceiling admitted for one Agent Run.
///
/// Runtime values, package paths, credentials, and leases are intentionally
/// absent. A recovery host reconstructs those values, then Code verifies this
/// identity before the target Run is created.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunCapabilityBindingV1 {
    schema: String,
    capability_set_schema: String,
    code_catalog_generation: u64,
    catalog_digest: String,
    capability_ceiling_schema: String,
    capability_ceiling_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    use_generation: Option<RunUseCapabilityGenerationV1>,
}

impl RunCapabilityBindingV1 {
    pub fn from_set_and_ceiling(
        set: &CapabilitySet,
        ceiling: &CapabilityCeiling,
    ) -> Result<Self, RunCapabilityBindingError> {
        if ceiling.catalog_digest() != set.digest() {
            return Err(invalid_field(
                "capabilityCeiling",
                "the authority ceiling belongs to another capability catalog",
            ));
        }
        Ok(Self {
            schema: RUN_CAPABILITY_BINDING_SCHEMA.to_owned(),
            capability_set_schema: set.schema().to_owned(),
            code_catalog_generation: set.generation().get(),
            catalog_digest: set.digest().to_string(),
            capability_ceiling_schema: ceiling.schema().to_owned(),
            capability_ceiling_digest: ceiling_digest(ceiling)?.to_string(),
            use_generation: set
                .use_capability_generation()
                .map(RunUseCapabilityGenerationV1::from_generation),
        })
    }

    pub fn validate(&self) -> Result<(), RunCapabilityBindingError> {
        ensure_schema("schema", &self.schema, RUN_CAPABILITY_BINDING_SCHEMA)?;
        ensure_schema(
            "capabilitySetSchema",
            &self.capability_set_schema,
            CAPABILITY_SET_SCHEMA,
        )?;
        ensure_digest("catalogDigest", &self.catalog_digest)?;
        ensure_schema(
            "capabilityCeilingSchema",
            &self.capability_ceiling_schema,
            CAPABILITY_CEILING_SCHEMA,
        )?;
        ensure_digest("capabilityCeilingDigest", &self.capability_ceiling_digest)?;
        if let Some(use_generation) = &self.use_generation {
            use_generation.validate()?;
        }
        Ok(())
    }

    pub fn ensure_matches(
        &self,
        set: &CapabilitySet,
        ceiling: &CapabilityCeiling,
    ) -> Result<(), RunCapabilityBindingError> {
        self.validate()?;
        let actual = Self::from_set_and_ceiling(set, ceiling)?;
        if self == &actual {
            return Ok(());
        }
        Err(RunCapabilityBindingError::ContentDrift {
            expected_generation: self.code_catalog_generation,
            actual_generation: actual.code_catalog_generation,
            catalog_digest_mismatch: self.catalog_digest != actual.catalog_digest,
            ceiling_digest_mismatch: self.capability_ceiling_digest
                != actual.capability_ceiling_digest,
            use_generation_mismatch: self.use_generation != actual.use_generation,
        })
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn capability_set_schema(&self) -> &str {
        &self.capability_set_schema
    }

    pub const fn code_catalog_generation(&self) -> u64 {
        self.code_catalog_generation
    }

    pub fn catalog_digest(&self) -> &str {
        &self.catalog_digest
    }

    pub fn capability_ceiling_schema(&self) -> &str {
        &self.capability_ceiling_schema
    }

    pub fn capability_ceiling_digest(&self) -> &str {
        &self.capability_ceiling_digest
    }

    pub fn use_generation(&self) -> Option<&RunUseCapabilityGenerationV1> {
        self.use_generation.as_ref()
    }
}

fn ceiling_digest(ceiling: &CapabilityCeiling) -> Result<Sha256Digest, RunCapabilityBindingError> {
    let encoded = serde_json::to_vec(ceiling)
        .map_err(|error| RunCapabilityBindingError::Encoding(error.to_string()))?;
    if encoded.len() as u64 > MAX_CAPABILITY_CANONICAL_BYTES {
        return Err(invalid_field(
            "capabilityCeiling",
            "the canonical authority ceiling exceeds the capability identity bound",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(CAPABILITY_CEILING_DIGEST_PREFIX);
    hasher.update(CAPABILITY_CEILING_DIGEST_DOMAIN.as_bytes());
    hasher.update([0]);
    hasher.update(encoded);
    Sha256Digest::new(format!("sha256:{:x}", hasher.finalize()))
        .map_err(|error| RunCapabilityBindingError::Encoding(error.to_string()))
}

fn ensure_schema(
    field: &'static str,
    actual: &str,
    expected: &str,
) -> Result<(), RunCapabilityBindingError> {
    if actual == expected {
        Ok(())
    } else {
        Err(invalid_field(field, "the schema is unsupported"))
    }
}

fn ensure_digest(field: &'static str, value: &str) -> Result<(), RunCapabilityBindingError> {
    Sha256Digest::new(value.to_owned())
        .map(|_| ())
        .map_err(|_| {
            invalid_field(
                field,
                "the value is not a canonical lowercase SHA-256 digest",
            )
        })
}

fn invalid_field(field: &'static str, message: impl Into<String>) -> RunCapabilityBindingError {
    RunCapabilityBindingError::InvalidField {
        field,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{
        CapabilityContribution, CapabilityExecutionCeiling, CodeCatalogGeneration,
        GovernanceCapabilityCeiling, WorkspaceCapabilityCeiling,
    };

    fn ceiling(set: &CapabilitySet, max_tool_rounds: usize) -> CapabilityCeiling {
        CapabilityCeiling::all(
            set,
            WorkspaceCapabilityCeiling::all(),
            GovernanceCapabilityCeiling::none_required(),
            CapabilityExecutionCeiling::new(max_tool_rounds, 4, None, None, None).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn binding_round_trips_and_rejects_catalog_or_ceiling_drift() {
        let first = CapabilitySet::empty().unwrap();
        let first_ceiling = ceiling(&first, 8);
        let binding = RunCapabilityBindingV1::from_set_and_ceiling(&first, &first_ceiling).unwrap();
        let encoded = serde_json::to_vec(&binding).unwrap();
        let decoded: RunCapabilityBindingV1 = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, binding);
        decoded.ensure_matches(&first, &first_ceiling).unwrap();

        let second = CapabilitySet::from_contributions(
            CodeCatalogGeneration::new(1),
            Vec::<CapabilityContribution>::new(),
        )
        .unwrap();
        assert!(matches!(
            binding.ensure_matches(&second, &ceiling(&second, 8)),
            Err(RunCapabilityBindingError::ContentDrift {
                catalog_digest_mismatch: true,
                ..
            })
        ));
        assert!(matches!(
            binding.ensure_matches(&first, &ceiling(&first, 7)),
            Err(RunCapabilityBindingError::ContentDrift {
                ceiling_digest_mismatch: true,
                ..
            })
        ));
    }

    #[test]
    fn malformed_persisted_digests_fail_validation() {
        let set = CapabilitySet::empty().unwrap();
        let mut value = serde_json::to_value(
            RunCapabilityBindingV1::from_set_and_ceiling(&set, &ceiling(&set, 8)).unwrap(),
        )
        .unwrap();
        value["catalogDigest"] = serde_json::Value::String("sha256:UPPER".into());
        let malformed: RunCapabilityBindingV1 = serde_json::from_value(value).unwrap();
        assert!(matches!(
            malformed.validate(),
            Err(RunCapabilityBindingError::InvalidField {
                field: "catalogDigest",
                ..
            })
        ));
    }
}
