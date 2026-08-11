use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MANIFEST_SHA256: &str =
    "sha256:ac858d66c68b16442b16e8fd8a11caada06207e810ff64d98358b1a3887fb2c9";
const PACKAGE_DIGEST_DOMAIN: &str = "agentic.ontology.r0-contract-fixture-package.v1";
const TASK_PROOF_DIGEST_DOMAIN: &str = "agentic.ontology.r0-code-task-proof.v1";
const CLOUD_AUDIT_DIGEST_DOMAIN: &str = "agentic.ontology.r0-cloud-audit-link.v1";
const CONTRACT_DIGEST_DOMAIN: &str = "agentic.ontology.r0-cross-project-contract.v1";
const MAX_CONTRACT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractError {
    Invalid,
    BindingMismatch,
    DigestMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Contract {
    pub schema: String,
    pub candidate: CandidateBinding,
    pub generation: GenerationBinding,
    pub knowledge: KnowledgeBinding,
    pub code: CodeBinding,
    pub cloud: CloudBinding,
    pub contract_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CandidateBinding {
    pub handoff_schema: String,
    pub handoff_digest: String,
    pub package_id: String,
    pub package_version: String,
    pub knowledge_revision_digest: String,
    pub package_digest: String,
    pub manifest_digest: String,
    pub requested_channel: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationBinding {
    pub receipt_schema: String,
    pub receipt_digest: String,
    pub issuer: String,
    pub host_target: String,
    pub handoff_digest: String,
    pub package_id: String,
    pub package_version: String,
    pub lifecycle_generation: u64,
    pub package_lock_digest: String,
    pub registry_generation: u64,
    pub registry_snapshot_digest: String,
    pub generation_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KnowledgeBinding {
    pub schema: String,
    pub surface_id: String,
    pub format_version: String,
    pub content_digest: String,
    pub search_schema: String,
    pub read_schema: String,
    pub citation_schema: String,
    pub lifecycle_generation: u64,
    pub generation_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodeBinding {
    pub schema: String,
    pub task_proof_schema: String,
    pub agent_protocol: String,
    pub agent_release_identity: String,
    pub session_id: String,
    pub run_id: String,
    pub package_id: String,
    pub package_version: String,
    pub lifecycle_generation: u64,
    pub generation_digest: String,
    pub capability_snapshot_digest: String,
    pub command_receipt_digest: String,
    pub event_stream_digest: String,
    pub result_digest: String,
    pub task_proof_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudBinding {
    pub schema: String,
    pub organization_id: String,
    pub workspace_id: String,
    pub package_release_id: String,
    pub agent_execution_id: String,
    pub handoff_digest: String,
    pub generation_receipt_digest: String,
    pub task_proof_digest: String,
    pub audit_digest: String,
}

impl Contract {
    pub fn parse(bytes: &[u8]) -> Result<Self, ContractError> {
        if bytes.is_empty() || bytes.len() > MAX_CONTRACT_BYTES {
            return Err(ContractError::Invalid);
        }
        let contract = serde_json::from_slice::<Self>(bytes).map_err(|_| ContractError::Invalid)?;
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<(), ContractError> {
        if self.schema != "agentic.ontology.r0-cross-project-contract.v1"
            || self.candidate.handoff_schema != "agentic.ontology.a3s-use-handoff.v1"
            || self.generation.receipt_schema != "agentic.ontology.a3s-use-generation-receipt.v1"
            || self.generation.issuer != "a3s-use"
            || self.knowledge.schema != "agentic.ontology.r0-knowledge-lease-binding.v1"
            || self.knowledge.search_schema != "a3s.use.okf-knowledge-search-request.v1"
            || self.knowledge.read_schema != "a3s.use.okf-knowledge-read-request.v1"
            || self.knowledge.citation_schema != "a3s.use.okf-knowledge-citation.v1"
            || self.knowledge.format_version != "0.2"
            || self.code.schema != "agentic.ontology.r0-code-session-binding.v1"
            || self.code.task_proof_schema != "agentic.ontology.r0-code-task-proof.v1"
            || self.code.agent_protocol != "a3s.code.agent.v1"
            || self.cloud.schema != "agentic.ontology.r0-cloud-audit-link.v1"
            || !matches!(
                self.candidate.requested_channel.as_str(),
                "stable" | "beta" | "nightly"
            )
            || self.generation.lifecycle_generation == 0
            || self.generation.registry_generation == 0
            || self.knowledge.lifecycle_generation == 0
            || !valid_package_id(&self.candidate.package_id)
            || self.candidate.package_version.trim().is_empty()
        {
            return Err(ContractError::Invalid);
        }
        for id in [
            &self.generation.host_target,
            &self.knowledge.surface_id,
            &self.code.session_id,
            &self.code.run_id,
            &self.cloud.organization_id,
            &self.cloud.workspace_id,
            &self.cloud.package_release_id,
            &self.cloud.agent_execution_id,
        ] {
            if !valid_id(id) {
                return Err(ContractError::Invalid);
            }
        }
        for digest in [
            &self.candidate.handoff_digest,
            &self.candidate.knowledge_revision_digest,
            &self.candidate.package_digest,
            &self.candidate.manifest_digest,
            &self.generation.receipt_digest,
            &self.generation.handoff_digest,
            &self.generation.package_lock_digest,
            &self.generation.registry_snapshot_digest,
            &self.generation.generation_digest,
            &self.knowledge.content_digest,
            &self.knowledge.generation_digest,
            &self.code.agent_release_identity,
            &self.code.generation_digest,
            &self.code.capability_snapshot_digest,
            &self.code.command_receipt_digest,
            &self.code.event_stream_digest,
            &self.code.result_digest,
            &self.code.task_proof_digest,
            &self.cloud.handoff_digest,
            &self.cloud.generation_receipt_digest,
            &self.cloud.task_proof_digest,
            &self.cloud.audit_digest,
            &self.contract_digest,
        ] {
            if !valid_digest(digest) {
                return Err(ContractError::Invalid);
            }
        }
        if self.generation.handoff_digest != self.candidate.handoff_digest
            || self.generation.package_id != self.candidate.package_id
            || self.generation.package_version != self.candidate.package_version
            || self.knowledge.lifecycle_generation != self.generation.lifecycle_generation
            || self.knowledge.generation_digest != self.generation.generation_digest
            || self.code.package_id != self.candidate.package_id
            || self.code.package_version != self.candidate.package_version
            || self.code.lifecycle_generation != self.generation.lifecycle_generation
            || self.code.generation_digest != self.generation.generation_digest
            || self.cloud.handoff_digest != self.candidate.handoff_digest
            || self.cloud.generation_receipt_digest != self.generation.receipt_digest
            || self.cloud.task_proof_digest != self.code.task_proof_digest
        {
            return Err(ContractError::BindingMismatch);
        }
        if self.code.task_proof_digest != self.task_proof_digest()
            || self.cloud.audit_digest != self.cloud_audit_digest()
            || self.contract_digest != self.complete_digest()
        {
            return Err(ContractError::DigestMismatch);
        }
        Ok(())
    }

    fn task_proof_digest(&self) -> String {
        canonical_digest(
            TASK_PROOF_DIGEST_DOMAIN,
            &(
                &self.code.task_proof_schema,
                &self.code.agent_protocol,
                &self.code.agent_release_identity,
                &self.code.session_id,
                &self.code.run_id,
                &self.code.package_id,
                &self.code.package_version,
                self.code.lifecycle_generation,
                &self.code.generation_digest,
                &self.code.capability_snapshot_digest,
                &self.code.command_receipt_digest,
                &self.code.event_stream_digest,
                &self.code.result_digest,
            ),
        )
    }

    fn cloud_audit_digest(&self) -> String {
        canonical_digest(
            CLOUD_AUDIT_DIGEST_DOMAIN,
            &(
                &self.cloud.organization_id,
                &self.cloud.workspace_id,
                &self.cloud.package_release_id,
                &self.cloud.agent_execution_id,
                &self.cloud.handoff_digest,
                &self.cloud.generation_receipt_digest,
                &self.cloud.task_proof_digest,
            ),
        )
    }

    fn complete_digest(&self) -> String {
        canonical_digest(
            CONTRACT_DIGEST_DOMAIN,
            &(
                &self.schema,
                &self.candidate,
                &self.generation,
                &self.knowledge,
                &self.code,
                &self.cloud,
            ),
        )
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FixtureManifest {
    schema: String,
    package: String,
    contract_schema: String,
    components: Vec<FixtureComponent>,
    package_digest: String,
    files: Vec<FixtureFile>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FixtureComponent {
    component: String,
    version: String,
    boundary: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FixtureFile {
    path: String,
    bytes: u64,
    sha256: String,
}

pub fn verify_fixture_package(root: &Path) -> Result<(), String> {
    let manifest_bytes = std::fs::read(root.join("manifest.json")).map_err(|e| e.to_string())?;
    if sha256(&manifest_bytes) != MANIFEST_SHA256 {
        return Err("vendored manifest digest drifted".into());
    }
    let manifest: FixtureManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|e| e.to_string())?;
    if manifest.schema != "agentic.ontology.r0-contract-fixture-manifest.v1"
        || manifest.package != "r0-cross-project-v1"
        || manifest.contract_schema != "agentic.ontology.r0-cross-project-contract.v1"
        || manifest
            .components
            .iter()
            .map(|component| {
                (
                    component.component.as_str(),
                    component.version.as_str(),
                    component.boundary.as_str(),
                )
            })
            .collect::<Vec<_>>()
            != [
                (
                    "a3s-cloud-contracts",
                    "0.1.0",
                    "agentic.ontology.r0-cloud-audit-link.v1",
                ),
                ("a3s-code-core", "6.8.0", "a3s.code.agent.v1"),
                (
                    "a3s-use",
                    "0.3.0",
                    "agentic.ontology.a3s-use-generation-receipt.v1",
                ),
                (
                    "agentic-ontology-a3s",
                    "0.1.0",
                    "agentic.ontology.r0-cross-project-contract.v1",
                ),
            ]
        || manifest.files.is_empty()
        || !manifest
            .files
            .windows(2)
            .all(|pair| pair[0].path < pair[1].path)
    {
        return Err("vendored fixture manifest is invalid".into());
    }
    let mut declared = Vec::with_capacity(manifest.files.len());
    for file in &manifest.files {
        if !safe_relative_path(&file.path) || !valid_digest(&file.sha256) {
            return Err(format!("unsafe fixture entry {}", file.path));
        }
        let bytes = std::fs::read(root.join(&file.path)).map_err(|e| e.to_string())?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != file.bytes
            || sha256(&bytes) != file.sha256
        {
            return Err(format!("fixture bytes drifted for {}", file.path));
        }
        declared.push(file.path.clone());
    }
    let mut actual = package_files(root)?;
    actual.retain(|path| path != "manifest.json");
    if actual != declared {
        return Err("vendored fixture file set drifted".into());
    }
    if canonical_digest(PACKAGE_DIGEST_DOMAIN, &manifest.files) != manifest.package_digest {
        return Err("vendored package digest drifted".into());
    }
    Ok(())
}

pub fn canonical_digest<T: Serialize + ?Sized>(domain: &str, value: &T) -> String {
    let encoded = serde_json::to_vec(value).expect("fixture value must serialize");
    let mut hasher = Sha256::new();
    hasher.update(b"agentic-ontology-canonical-v1\0");
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain.as_bytes());
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    format!("sha256:{:x}", hasher.finalize())
}

pub fn read_fixture(root: &Path, path: &str) -> Vec<u8> {
    std::fs::read(root.join(path)).expect("read R0 fixture")
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.is_ascii()
        && !value.bytes().any(|byte| byte.is_ascii_control())
}

fn valid_package_id(value: &str) -> bool {
    value.split_once('/').is_some_and(|(publisher, name)| {
        !publisher.is_empty()
            && !name.is_empty()
            && [publisher, name].into_iter().all(|segment| {
                segment.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_')
                })
            })
    })
}

fn safe_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn package_files(root: &Path) -> Result<Vec<String>, String> {
    fn visit(root: &Path, path: &Path, files: &mut Vec<String>) -> Result<(), String> {
        let mut entries = std::fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let file_type = entry.file_type().map_err(|e| e.to_string())?;
            if file_type.is_symlink() {
                return Err("fixture package must not contain symlinks".into());
            }
            if file_type.is_dir() {
                visit(root, &entry.path(), files)?;
            } else if file_type.is_file() {
                files.push(
                    entry
                        .path()
                        .strip_prefix(root)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            } else {
                return Err("fixture package contains an unsupported entry".into());
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    files.sort();
    Ok(files)
}
