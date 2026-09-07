//! Cross-language capability batch wire format (SDK-CAP1).
//!
//! Hosts that cannot transport Rust trait objects stage serializable Skill
//! values into one atomic [`SessionCapabilityBatch`]. Tool, Hook, MCP, and
//! other callback-backed kinds remain Rust-host-only until typed adapters land.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::skills::{Skill, SkillKind};

use super::{
    CapabilityContribution, CapabilityDescriptor, CapabilityKind, CapabilityRuntimeError,
    CapabilitySet, CapabilitySetError, CapabilitySource, CapabilityValue, CodeCatalogGeneration,
    SessionCapabilityBatch, Sha256Digest,
};

pub const SDK_CAPABILITY_BATCH_SCHEMA: &str = "a3s.code.sdk_capability_batch.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SdkCapabilityBatchV1 {
    pub schema_version: u32,
    pub generation: u64,
    pub source_id: String,
    #[serde(default)]
    pub skills: Vec<SdkSkillCapabilityV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SdkSkillCapabilityV1 {
    pub local_id: String,
    pub name: String,
    #[serde(default)]
    pub kind: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SdkCapabilityCommitReceiptV1 {
    pub previous_generation: u64,
    pub committed_generation: u64,
    pub previous_digest: String,
    pub committed_digest: String,
}

impl SdkCapabilityCommitReceiptV1 {
    pub fn from_receipt(receipt: &super::CapabilityCommitReceipt) -> Self {
        Self {
            previous_generation: receipt.previous().generation().get(),
            committed_generation: receipt.committed().generation().get(),
            previous_digest: receipt.previous().digest().as_str().to_string(),
            committed_digest: receipt.committed().digest().as_str().to_string(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SdkCapabilityBatchError {
    #[error("unsupported SDK capability batch schema version {found}; expected 1")]
    UnsupportedSchema { found: u32 },
    #[error("SDK capability batch must stage at least one Skill")]
    EmptyBatch,
    #[error("SDK skill name must not be empty")]
    EmptySkillName,
    #[error("unknown skill kind '{kind}'; use instruction, persona, or tool")]
    UnknownSkillKind { kind: String },
    #[error(transparent)]
    CapabilitySet(#[from] CapabilitySetError),
    #[error(transparent)]
    Runtime(#[from] CapabilityRuntimeError),
}

impl SdkCapabilityBatchV1 {
    pub fn into_session_batch(self) -> Result<SessionCapabilityBatch, SdkCapabilityBatchError> {
        if self.schema_version != 1 {
            return Err(SdkCapabilityBatchError::UnsupportedSchema {
                found: self.schema_version,
            });
        }
        if self.skills.is_empty() {
            return Err(SdkCapabilityBatchError::EmptyBatch);
        }

        let source_revision = digest_bytes(self.source_id.as_bytes())?;
        let source = CapabilitySource::host(self.source_id.trim(), source_revision)?;
        let mut descriptors = Vec::with_capacity(self.skills.len());
        let mut staged = Vec::with_capacity(self.skills.len());

        for skill_spec in self.skills {
            let skill = skill_from_sdk(skill_spec)?;
            let local_id = skill.name.clone();
            let public_name = skill.name.clone();
            let surface = digest_bytes(
                format!("{}:{}:{}", skill.name, skill.kind_as_str(), skill.content).as_bytes(),
            )?;
            let descriptor = CapabilityDescriptor::new(
                &source,
                CapabilityKind::Skill,
                local_id,
                public_name,
                surface,
                [],
            )?;
            let id = descriptor.id().clone();
            descriptors.push(descriptor);
            staged.push((id, CapabilityValue::Skill(skill)));
        }

        let set = CapabilitySet::from_contributions(
            CodeCatalogGeneration::new(self.generation),
            [CapabilityContribution::new(source, descriptors)?],
        )?;
        let mut batch = SessionCapabilityBatch::new(set)?;
        for (id, value) in staged {
            batch.stage_value(id, value)?;
        }
        Ok(batch)
    }
}

fn skill_from_sdk(spec: SdkSkillCapabilityV1) -> Result<Arc<Skill>, SdkCapabilityBatchError> {
    let name = if spec.name.trim().is_empty() {
        spec.local_id.trim().to_string()
    } else {
        spec.name.trim().to_string()
    };
    if name.is_empty() {
        return Err(SdkCapabilityBatchError::EmptySkillName);
    }
    let kind = match spec.kind.trim() {
        "" | "instruction" => SkillKind::Instruction,
        "persona" => SkillKind::Persona,
        "tool" => SkillKind::Tool,
        other => {
            return Err(SdkCapabilityBatchError::UnknownSkillKind {
                kind: other.to_string(),
            })
        }
    };
    Ok(Arc::new(Skill {
        name,
        description: String::new(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind,
        content: spec.content,
        tags: Vec::new(),
        version: None,
    }))
}

trait SkillKindLabel {
    fn kind_as_str(&self) -> &'static str;
}

impl SkillKindLabel for Skill {
    fn kind_as_str(&self) -> &'static str {
        match self.kind {
            SkillKind::Instruction => "instruction",
            SkillKind::Persona => "persona",
            SkillKind::Tool => "tool",
        }
    }
}

fn digest_bytes(bytes: &[u8]) -> Result<Sha256Digest, CapabilitySetError> {
    Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_skill_batch_builds_session_capability_batch() {
        let batch = SdkCapabilityBatchV1 {
            schema_version: 1,
            generation: 3,
            source_id: "sdk-host".to_string(),
            skills: vec![SdkSkillCapabilityV1 {
                local_id: "type-hints".to_string(),
                name: "type-hints".to_string(),
                kind: "instruction".to_string(),
                content: "Prefer explicit types.".to_string(),
            }],
        }
        .into_session_batch()
        .expect("skill batch must build");
        assert_eq!(batch.len(), 1);
        assert_eq!(batch.target().generation().get(), 3);
    }

    #[test]
    fn sdk_batch_rejects_empty_and_bad_schema() {
        let empty = SdkCapabilityBatchV1 {
            schema_version: 1,
            generation: 1,
            source_id: "sdk-host".to_string(),
            skills: Vec::new(),
        };
        assert!(matches!(
            empty.into_session_batch(),
            Err(SdkCapabilityBatchError::EmptyBatch)
        ));
        let bad = SdkCapabilityBatchV1 {
            schema_version: 9,
            generation: 1,
            source_id: "sdk-host".to_string(),
            skills: vec![SdkSkillCapabilityV1 {
                local_id: "x".to_string(),
                name: "x".to_string(),
                kind: "instruction".to_string(),
                content: "x".to_string(),
            }],
        };
        assert!(matches!(
            bad.into_session_batch(),
            Err(SdkCapabilityBatchError::UnsupportedSchema { found: 9 })
        ));
    }
}
