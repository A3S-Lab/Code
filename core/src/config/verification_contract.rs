//! Host-owned verification declarations, scoped to one physical workspace.

use super::CodeConfig;
use crate::error::{CodeError, Result};
use crate::verification::VerificationContractV1;
use a3s_acl::{Block, Value};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceVerificationContract {
    pub workspace: PathBuf,
    pub contract: VerificationContractV1,
}

fn invalid(message: impl Into<String>) -> CodeError {
    CodeError::Config(format!("verification_contract: {}", message.into()))
}

pub(super) fn parse(block: &Block) -> Result<WorkspaceVerificationContract> {
    if block.labels.len() != 1 || !block.blocks.is_empty() {
        return Err(invalid("requires one id label and no nested blocks"));
    }
    if block
        .attributes
        .keys()
        .any(|key| !matches!(key.as_str(), "workspace" | "command" | "covers"))
    {
        return Err(invalid(
            "attribute is not in the closed set workspace, command, covers",
        ));
    }
    let string = |key: &str| match block.attributes.get(key) {
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(text.clone()),
        _ => Err(invalid(format!("{key} requires a nonempty literal string"))),
    };
    let workspace = PathBuf::from(string("workspace")?);
    if !workspace.is_absolute() {
        return Err(invalid("workspace must be absolute"));
    }
    let command = string("command")?;
    let covers = match block.attributes.get("covers") {
        Some(Value::List(values)) if !values.is_empty() && values.len() <= 256 => values
            .iter()
            .map(|value| match value {
                Value::String(text) if !text.trim().is_empty() => Ok(text.clone()),
                _ => Err(invalid("covers requires nonempty literal path strings")),
            })
            .collect::<Result<Vec<_>>>()?,
        _ => return Err(invalid("covers requires 1 to 256 path strings")),
    };
    let contract = VerificationContractV1::new(&block.labels[0], command, covers)
        .map_err(|error| invalid(error.to_string()))?;
    Ok(WorkspaceVerificationContract {
        workspace,
        contract,
    })
}

impl CodeConfig {
    /// Resolve only host declarations for the actual physical workspace.
    /// Missing/unreadable declared roots fail before session activation.
    /// Aliases cannot bypass duplicate-id detection after canonicalization.
    pub fn verification_contracts_for_workspace(
        &self,
        workspace: &Path,
    ) -> Result<Vec<VerificationContractV1>> {
        if self.verification_contracts.is_empty() {
            return Ok(Vec::new());
        }
        let workspace = std::fs::canonicalize(workspace)
            .map_err(|error| invalid(format!("cannot resolve active workspace: {error}")))?;
        let mut selected: Vec<VerificationContractV1> = Vec::new();
        for scoped in &self.verification_contracts {
            scoped
                .contract
                .validate()
                .map_err(|error| invalid(error.to_string()))?;
            if !scoped.workspace.is_absolute() {
                return Err(invalid("workspace must be absolute"));
            }
            let declared = std::fs::canonicalize(&scoped.workspace)
                .map_err(|error| invalid(format!("cannot resolve declared workspace: {error}")))?;
            if !declared.is_dir() {
                return Err(invalid("declared workspace must be a directory"));
            }
            if declared != workspace {
                continue;
            }
            if selected
                .iter()
                .any(|current| current.id == scoped.contract.id)
            {
                return Err(invalid("duplicate id for the physical workspace"));
            }
            selected.push(scoped.contract.clone());
        }
        Ok(selected)
    }
}

#[cfg(test)]
mod tests;
