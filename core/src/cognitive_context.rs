//! Exact-generation cognitive-package context injected by an embedding host.
//!
//! A3S Code deliberately does not install packages, resolve Registry state, or
//! choose a "latest" generation. The host supplies a provider and one complete
//! immutable binding obtained from A3S Use. Every request and response repeats
//! that binding, and Code validates cited, bounded Markdown before it can enter
//! the model context.

use crate::context::{
    ContextItem, ContextProvider, ContextProviderFailureMode, ContextQuery, ContextResult,
    ContextType,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
use thiserror::Error;

pub const COGNITIVE_PACKAGE_BINDING_SCHEMA: &str = "a3s.code.cognitive-package-session-binding.v1";
pub const COGNITIVE_KNOWLEDGE_BINDING_SCHEMA: &str =
    "agentic.ontology.r0-knowledge-lease-binding.v1";
pub const COGNITIVE_CONTEXT_REQUEST_SCHEMA: &str = "a3s.code.cognitive-context-request.v1";
pub const COGNITIVE_CONTEXT_RESPONSE_SCHEMA: &str = "a3s.code.cognitive-context-response.v1";
pub const COGNITIVE_CONTEXT_REQUEST_DIGEST_DOMAIN: &str = "a3s.code.cognitive-context-request.v1";
pub const OKF_KNOWLEDGE_SEARCH_REQUEST_SCHEMA: &str = "a3s.use.okf-knowledge-search-request.v1";
pub const OKF_KNOWLEDGE_READ_REQUEST_SCHEMA: &str = "a3s.use.okf-knowledge-read-request.v1";
pub const OKF_KNOWLEDGE_CITATION_SCHEMA: &str = "a3s.use.okf-knowledge-citation.v1";
pub const COGNITIVE_CITATION_METADATA: &str = "a3s.cognitive.citation";
pub const COGNITIVE_PACKAGE_BINDING_METADATA: &str = "a3s.cognitive.package_binding";

const CAPABILITY_SNAPSHOT_DIGEST_DOMAIN: &str = "a3s.use.capability-snapshot.v1";
const CANONICAL_DIGEST_PREFIX: &[u8] = b"agentic-ontology-canonical-v1\0";
const MAX_QUERY_BYTES: usize = 4 * 1024;
const MAX_PROVIDER_NAME_BYTES: usize = 128;
const MAX_PACKAGE_VERSION_BYTES: usize = 128;
const MAX_SURFACE_ID_BYTES: usize = 256;
const MAX_FORMAT_VERSION_BYTES: usize = 64;
const MAX_DOCUMENT_PATH_BYTES: usize = 512;
const MAX_HEADING_BYTES: usize = 128;
const MAX_EVIDENCE_IDS: usize = 256;
const MAX_CONTEXT_RESULTS: usize = 4;
const MAX_CONTEXT_DOCUMENT_BYTES: usize = 6 * 1024;
const MAX_CONTEXT_TOTAL_BYTES: usize = 6 * 1024;

/// Validation failures at the Code-owned cognitive context boundary.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CognitiveContextError {
    #[error("invalid cognitive package binding: {0}")]
    InvalidBinding(String),
    #[error("invalid cognitive context limits: {0}")]
    InvalidLimits(String),
    #[error("invalid cognitive context request: {0}")]
    InvalidRequest(String),
    #[error("cognitive context provider failed: {0}")]
    Provider(String),
    #[error("cognitive context response drifted: {0}")]
    ResponseDrift(String),
}

pub type CognitiveContextResult<T> = std::result::Result<T, CognitiveContextError>;

/// Exact A3S Use Knowledge surface visible to this Code session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitiveKnowledgeBindingV1 {
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

impl CognitiveKnowledgeBindingV1 {
    pub fn new(
        surface_id: impl Into<String>,
        format_version: impl Into<String>,
        content_digest: impl Into<String>,
        lifecycle_generation: u64,
        generation_digest: impl Into<String>,
    ) -> CognitiveContextResult<Self> {
        let binding = Self {
            schema: COGNITIVE_KNOWLEDGE_BINDING_SCHEMA.to_string(),
            surface_id: surface_id.into(),
            format_version: format_version.into(),
            content_digest: content_digest.into(),
            search_schema: OKF_KNOWLEDGE_SEARCH_REQUEST_SCHEMA.to_string(),
            read_schema: OKF_KNOWLEDGE_READ_REQUEST_SCHEMA.to_string(),
            citation_schema: OKF_KNOWLEDGE_CITATION_SCHEMA.to_string(),
            lifecycle_generation,
            generation_digest: generation_digest.into(),
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> CognitiveContextResult<()> {
        if self.schema != COGNITIVE_KNOWLEDGE_BINDING_SCHEMA
            || self.search_schema != OKF_KNOWLEDGE_SEARCH_REQUEST_SCHEMA
            || self.read_schema != OKF_KNOWLEDGE_READ_REQUEST_SCHEMA
            || self.citation_schema != OKF_KNOWLEDGE_CITATION_SCHEMA
        {
            return Err(invalid_binding(
                "Knowledge schema negotiation is not the exact R0 typed search/read/citation contract",
            ));
        }
        if !valid_machine_id(&self.surface_id, MAX_SURFACE_ID_BYTES) {
            return Err(invalid_binding("Knowledge surface id is invalid"));
        }
        if !valid_plain_value(&self.format_version, MAX_FORMAT_VERSION_BYTES) {
            return Err(invalid_binding("Knowledge format version is invalid"));
        }
        if self.lifecycle_generation == 0 {
            return Err(invalid_binding(
                "lifecycle generation must be an exact non-zero generation",
            ));
        }
        if !valid_sha256(&self.content_digest) || !valid_sha256(&self.generation_digest) {
            return Err(invalid_binding(
                "Knowledge content and generation identities must be SHA-256 digests",
            ));
        }
        Ok(())
    }
}

/// Prompt-injection bounds frozen into the durable session binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitiveContextLimits {
    pub max_results: usize,
    pub max_document_bytes: usize,
    pub max_total_bytes: usize,
}

impl CognitiveContextLimits {
    pub fn new(
        max_results: usize,
        max_document_bytes: usize,
        max_total_bytes: usize,
    ) -> CognitiveContextResult<Self> {
        let limits = Self {
            max_results,
            max_document_bytes,
            max_total_bytes,
        };
        limits.validate()?;
        Ok(limits)
    }

    pub fn validate(&self) -> CognitiveContextResult<()> {
        if self.max_results == 0 || self.max_results > MAX_CONTEXT_RESULTS {
            return Err(CognitiveContextError::InvalidLimits(format!(
                "max_results must be between 1 and {MAX_CONTEXT_RESULTS}"
            )));
        }
        if self.max_document_bytes == 0 || self.max_document_bytes > MAX_CONTEXT_DOCUMENT_BYTES {
            return Err(CognitiveContextError::InvalidLimits(format!(
                "max_document_bytes must be between 1 and {MAX_CONTEXT_DOCUMENT_BYTES}"
            )));
        }
        if self.max_total_bytes == 0 || self.max_total_bytes > MAX_CONTEXT_TOTAL_BYTES {
            return Err(CognitiveContextError::InvalidLimits(format!(
                "max_total_bytes must be between 1 and {MAX_CONTEXT_TOTAL_BYTES}"
            )));
        }
        Ok(())
    }
}

impl Default for CognitiveContextLimits {
    fn default() -> Self {
        Self {
            max_results: MAX_CONTEXT_RESULTS,
            max_document_bytes: MAX_CONTEXT_DOCUMENT_BYTES,
            max_total_bytes: MAX_CONTEXT_TOTAL_BYTES,
        }
    }
}

/// Durable exact-generation identity attached to one A3S Code session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitivePackageBindingV1 {
    pub schema: String,
    pub package_id: String,
    pub package_version: String,
    pub lifecycle_generation: u64,
    pub generation_digest: String,
    pub capability_snapshot_digest: String,
    pub knowledge: CognitiveKnowledgeBindingV1,
    pub limits: CognitiveContextLimits,
}

impl CognitivePackageBindingV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        package_id: impl Into<String>,
        package_version: impl Into<String>,
        lifecycle_generation: u64,
        generation_digest: impl Into<String>,
        capability_snapshot_digest: impl Into<String>,
        knowledge: CognitiveKnowledgeBindingV1,
        limits: CognitiveContextLimits,
    ) -> CognitiveContextResult<Self> {
        let binding = Self {
            schema: COGNITIVE_PACKAGE_BINDING_SCHEMA.to_string(),
            package_id: package_id.into(),
            package_version: package_version.into(),
            lifecycle_generation,
            generation_digest: generation_digest.into(),
            capability_snapshot_digest: capability_snapshot_digest.into(),
            knowledge,
            limits,
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> CognitiveContextResult<()> {
        if self.schema != COGNITIVE_PACKAGE_BINDING_SCHEMA {
            return Err(invalid_binding("session binding schema is unsupported"));
        }
        if !valid_package_id(&self.package_id) {
            return Err(invalid_binding(
                "package id must be a canonical publisher/name identity",
            ));
        }
        if !valid_plain_value(&self.package_version, MAX_PACKAGE_VERSION_BYTES) {
            return Err(invalid_binding("package version is invalid"));
        }
        if self.lifecycle_generation == 0 {
            return Err(invalid_binding(
                "lifecycle generation must be an exact non-zero generation",
            ));
        }
        if !valid_sha256(&self.generation_digest) || !valid_sha256(&self.capability_snapshot_digest)
        {
            return Err(invalid_binding(
                "generation and capability snapshot identities must be SHA-256 digests",
            ));
        }
        self.knowledge.validate()?;
        self.limits.validate()?;
        if self.knowledge.lifecycle_generation != self.lifecycle_generation
            || self.knowledge.generation_digest != self.generation_digest
        {
            return Err(invalid_binding(
                "Knowledge surface belongs to a different lifecycle generation",
            ));
        }
        let expected = capability_snapshot_digest(self)?;
        if self.capability_snapshot_digest != expected {
            return Err(invalid_binding(
                "capability snapshot digest does not bind this package, generation, and Knowledge surface",
            ));
        }
        Ok(())
    }
}

/// One exact request from Code to the host-injected cognitive provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitiveContextRequestV1 {
    pub schema: String,
    pub session_id: String,
    pub query: String,
    pub binding: CognitivePackageBindingV1,
    pub request_digest: String,
}

impl CognitiveContextRequestV1 {
    pub fn new(
        session_id: impl Into<String>,
        query: impl Into<String>,
        binding: CognitivePackageBindingV1,
    ) -> CognitiveContextResult<Self> {
        let mut request = Self {
            schema: COGNITIVE_CONTEXT_REQUEST_SCHEMA.to_string(),
            session_id: session_id.into(),
            query: query.into(),
            binding,
            request_digest: String::new(),
        };
        request.request_digest = request.digest()?;
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> CognitiveContextResult<()> {
        self.binding.validate()?;
        if self.schema != COGNITIVE_CONTEXT_REQUEST_SCHEMA
            || !valid_machine_id(&self.session_id, 256)
            || self.query.trim() != self.query
            || self.query.is_empty()
            || self.query.len() > MAX_QUERY_BYTES
            || self
                .query
                .chars()
                .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        {
            return Err(CognitiveContextError::InvalidRequest(
                "request schema, session, or query is invalid or unbounded".to_string(),
            ));
        }
        if self.request_digest != self.digest()? {
            return Err(CognitiveContextError::InvalidRequest(
                "request digest does not bind the exact query and session generation".to_string(),
            ));
        }
        Ok(())
    }

    fn digest(&self) -> CognitiveContextResult<String> {
        canonical_digest(
            COGNITIVE_CONTEXT_REQUEST_DIGEST_DOMAIN,
            &(
                self.schema.as_str(),
                self.session_id.as_str(),
                self.query.as_str(),
                &self.binding,
            ),
        )
    }
}

/// Use-owned citation projected into the cross-project R0 carrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitiveKnowledgeCitationV1 {
    pub schema: String,
    pub package_id: String,
    pub package_version: String,
    pub lifecycle_generation: u64,
    pub generation_digest: String,
    pub surface_id: String,
    pub content_digest: String,
    pub document_path: String,
    pub heading: String,
    pub evidence_ids: Vec<String>,
    pub citation_digest: String,
}

impl CognitiveKnowledgeCitationV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        binding: &CognitivePackageBindingV1,
        document_path: impl Into<String>,
        heading: impl Into<String>,
        evidence_ids: Vec<String>,
    ) -> CognitiveContextResult<Self> {
        let mut citation = Self {
            schema: OKF_KNOWLEDGE_CITATION_SCHEMA.to_string(),
            package_id: binding.package_id.clone(),
            package_version: binding.package_version.clone(),
            lifecycle_generation: binding.lifecycle_generation,
            generation_digest: binding.generation_digest.clone(),
            surface_id: binding.knowledge.surface_id.clone(),
            content_digest: binding.knowledge.content_digest.clone(),
            document_path: document_path.into(),
            heading: heading.into(),
            evidence_ids,
            citation_digest: String::new(),
        };
        citation.citation_digest = citation.digest()?;
        citation.validate_for(binding)?;
        Ok(citation)
    }

    pub fn validate_for(&self, binding: &CognitivePackageBindingV1) -> CognitiveContextResult<()> {
        binding.validate()?;
        if self.schema != OKF_KNOWLEDGE_CITATION_SCHEMA
            || self.package_id != binding.package_id
            || self.package_version != binding.package_version
            || self.lifecycle_generation != binding.lifecycle_generation
            || self.generation_digest != binding.generation_digest
            || self.surface_id != binding.knowledge.surface_id
            || self.content_digest != binding.knowledge.content_digest
        {
            return Err(response_drift(
                "citation does not belong to the session's exact package generation and Knowledge surface",
            ));
        }
        if !valid_markdown_path(&self.document_path)
            || self.heading.trim() != self.heading
            || self.heading.is_empty()
            || self.heading.len() > MAX_HEADING_BYTES
            || self.heading.chars().any(char::is_control)
            || self.evidence_ids.is_empty()
            || self.evidence_ids.len() > MAX_EVIDENCE_IDS
            || !self.evidence_ids.iter().all(|value| valid_sha256(value))
            || !self.evidence_ids.windows(2).all(|pair| pair[0] < pair[1])
        {
            return Err(response_drift(
                "citation path, heading, or canonical evidence identity set is invalid",
            ));
        }
        if self.citation_digest != self.digest()? {
            return Err(response_drift("citation digest was substituted"));
        }
        Ok(())
    }

    fn digest(&self) -> CognitiveContextResult<String> {
        canonical_digest(
            OKF_KNOWLEDGE_CITATION_SCHEMA,
            &(
                self.package_id.as_str(),
                self.package_version.as_str(),
                self.lifecycle_generation,
                &self.generation_digest,
                self.surface_id.as_str(),
                self.content_digest.as_str(),
                self.document_path.as_str(),
                self.heading.as_str(),
                &self.evidence_ids,
            ),
        )
    }
}

/// One complete bounded Markdown read and its unchanged citation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitiveContextDocumentV1 {
    pub citation: CognitiveKnowledgeCitationV1,
    pub source_digest: String,
    pub content: String,
    pub byte_count: usize,
}

impl CognitiveContextDocumentV1 {
    pub fn new(
        citation: CognitiveKnowledgeCitationV1,
        content: impl Into<String>,
    ) -> CognitiveContextResult<Self> {
        let content = content.into();
        let document = Self {
            citation,
            source_digest: sha256(content.as_bytes()),
            byte_count: content.len(),
            content,
        };
        if document.content.is_empty() {
            return Err(response_drift("cognitive document is empty"));
        }
        Ok(document)
    }

    fn validate_for(&self, request: &CognitiveContextRequestV1) -> CognitiveContextResult<()> {
        self.citation.validate_for(&request.binding)?;
        if self.content.is_empty()
            || self.byte_count != self.content.len()
            || self.byte_count > request.binding.limits.max_document_bytes
            || !valid_sha256(&self.source_digest)
            || self.source_digest != sha256(self.content.as_bytes())
        {
            return Err(response_drift(
                "document bytes do not match the cited bounded source read",
            ));
        }
        Ok(())
    }
}

/// Exact response that Code validates before converting it to prompt context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CognitiveContextResponseV1 {
    pub schema: String,
    pub request_digest: String,
    pub binding: CognitivePackageBindingV1,
    pub documents: Vec<CognitiveContextDocumentV1>,
    pub truncated: bool,
}

impl CognitiveContextResponseV1 {
    pub fn new(
        request: &CognitiveContextRequestV1,
        documents: Vec<CognitiveContextDocumentV1>,
        truncated: bool,
    ) -> CognitiveContextResult<Self> {
        let response = Self {
            schema: COGNITIVE_CONTEXT_RESPONSE_SCHEMA.to_string(),
            request_digest: request.request_digest.clone(),
            binding: request.binding.clone(),
            documents,
            truncated,
        };
        response.validate_for(request)?;
        Ok(response)
    }

    pub fn validate_for(&self, request: &CognitiveContextRequestV1) -> CognitiveContextResult<()> {
        request.validate()?;
        self.binding.validate()?;
        if self.schema != COGNITIVE_CONTEXT_RESPONSE_SCHEMA
            || self.request_digest != request.request_digest
            || self.binding != request.binding
            || self.documents.is_empty()
            || self.documents.len() > request.binding.limits.max_results
        {
            return Err(response_drift(
                "response schema, request, generation, or result count drifted",
            ));
        }

        let mut total_bytes = 0usize;
        let mut citations = HashSet::with_capacity(self.documents.len());
        for document in &self.documents {
            document.validate_for(request)?;
            total_bytes = total_bytes
                .checked_add(document.byte_count)
                .ok_or_else(|| {
                    response_drift("response byte accounting overflowed its bounded integer")
                })?;
            if !citations.insert(document.citation.citation_digest.as_str()) {
                return Err(response_drift("response repeats a cited document"));
            }
        }
        if total_bytes > request.binding.limits.max_total_bytes {
            return Err(response_drift(
                "response exceeds the session's total cognitive context byte bound",
            ));
        }
        Ok(())
    }
}

/// Host port. Implementations typically hold an A3S Use exact-generation
/// Knowledge lease; Code sees only bounded cited reads and never the Registry,
/// package filesystem, ontology graph, or a personal-memory fallback.
#[async_trait::async_trait]
pub trait CognitiveContextProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn query(
        &self,
        request: &CognitiveContextRequestV1,
    ) -> CognitiveContextResult<CognitiveContextResponseV1>;
}

/// Runtime pairing of one durable binding with one non-serializable host port.
#[derive(Clone)]
pub struct CognitiveContextSession {
    binding: CognitivePackageBindingV1,
    provider: Arc<dyn CognitiveContextProvider>,
}

impl std::fmt::Debug for CognitiveContextSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CognitiveContextSession")
            .field("binding", &self.binding)
            .field("provider", &self.provider.name())
            .finish()
    }
}

impl CognitiveContextSession {
    pub fn new(
        binding: CognitivePackageBindingV1,
        provider: Arc<dyn CognitiveContextProvider>,
    ) -> CognitiveContextResult<Self> {
        binding.validate()?;
        if !valid_plain_value(provider.name(), MAX_PROVIDER_NAME_BYTES) {
            return Err(CognitiveContextError::Provider(
                "provider name is empty, unbounded, or contains control characters".to_string(),
            ));
        }
        Ok(Self { binding, provider })
    }

    pub fn binding(&self) -> &CognitivePackageBindingV1 {
        &self.binding
    }

    pub fn provider_name(&self) -> &str {
        self.provider.name()
    }
}

#[async_trait::async_trait]
impl ContextProvider for CognitiveContextSession {
    fn name(&self) -> &str {
        self.provider.name()
    }

    fn failure_mode(&self) -> ContextProviderFailureMode {
        ContextProviderFailureMode::FailClosed
    }

    fn cognitive_package_binding(&self) -> Option<&CognitivePackageBindingV1> {
        Some(&self.binding)
    }

    async fn query(&self, query: &ContextQuery) -> anyhow::Result<ContextResult> {
        if !query.context_types.is_empty() && !query.context_types.contains(&ContextType::Resource)
        {
            return Err(CognitiveContextError::InvalidRequest(
                "cognitive packages expose cited resources only".to_string(),
            )
            .into());
        }
        let session_id = query.session_id.as_deref().unwrap_or_default();
        let request =
            CognitiveContextRequestV1::new(session_id, query.query.clone(), self.binding.clone())?;
        let response = self.provider.query(&request).await?;
        response.validate_for(&request)?;

        let mut result = ContextResult::new(self.provider.name());
        result.truncated = response.truncated;
        for (position, document) in response.documents.into_iter().enumerate() {
            let citation_json = serde_json::to_value(&document.citation)?;
            let binding_json = serde_json::to_value(&self.binding)?;
            let source_digest = document.source_digest.clone();
            let rendered = format!(
                "[cognitive citation={} document={} heading={}]\n\n{}",
                document.citation.citation_digest,
                document.citation.document_path,
                document.citation.heading,
                document.content
            );
            let token_count = rendered.len().div_ceil(4).max(1);
            let relevance = (1.0_f32 - (position as f32 * 0.02)).max(0.9);
            result.add_item(
                ContextItem::new(
                    document.citation.citation_digest.clone(),
                    ContextType::Resource,
                    rendered,
                )
                .with_source(format!(
                    "a3s-use://citation/{}",
                    document
                        .citation
                        .citation_digest
                        .strip_prefix("sha256:")
                        .unwrap_or(&document.citation.citation_digest)
                ))
                .with_metadata(COGNITIVE_CITATION_METADATA, citation_json)
                .with_metadata(COGNITIVE_PACKAGE_BINDING_METADATA, binding_json)
                .with_metadata("a3s.cognitive.source_digest", source_digest.into())
                .with_provenance("a3s-use-cognitive-package")
                .with_priority(1.0)
                .with_trust(1.0)
                .with_freshness(1.0)
                .with_relevance(relevance)
                .with_token_count(token_count),
            );
        }
        Ok(result)
    }
}

fn capability_snapshot_digest(
    binding: &CognitivePackageBindingV1,
) -> CognitiveContextResult<String> {
    canonical_digest(
        CAPABILITY_SNAPSHOT_DIGEST_DOMAIN,
        &(
            binding.package_id.as_str(),
            binding.package_version.as_str(),
            binding.lifecycle_generation,
            &binding.generation_digest,
            binding.knowledge.surface_id.as_str(),
            binding.knowledge.format_version.as_str(),
            binding.knowledge.content_digest.as_str(),
        ),
    )
}

fn canonical_digest<T: Serialize + ?Sized>(
    domain: &str,
    value: &T,
) -> CognitiveContextResult<String> {
    let encoded = serde_json::to_vec(value).map_err(|error| {
        CognitiveContextError::InvalidBinding(format!(
            "canonical identity could not be serialized: {error}"
        ))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(CANONICAL_DIGEST_PREFIX);
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain.as_bytes());
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn invalid_binding(message: impl Into<String>) -> CognitiveContextError {
    CognitiveContextError::InvalidBinding(message.into())
}

fn response_drift(message: impl Into<String>) -> CognitiveContextError {
    CognitiveContextError::ResponseDrift(message.into())
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn valid_machine_id(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.is_ascii()
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}

fn valid_plain_value(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.trim() == value
        && !value.chars().any(char::is_control)
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

fn valid_markdown_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_DOCUMENT_PATH_BYTES
        && value.ends_with(".md")
        && !value.starts_with('/')
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
        && value
            .split('/')
            .all(|component| !component.is_empty() && !matches!(component, "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> String {
        format!("sha256:{}", format!("{byte:02x}").repeat(32))
    }

    fn binding() -> CognitivePackageBindingV1 {
        let knowledge =
            CognitiveKnowledgeBindingV1::new("domain-knowledge", "0.2", digest(1), 7, digest(2))
                .unwrap();
        let mut binding = CognitivePackageBindingV1 {
            schema: COGNITIVE_PACKAGE_BINDING_SCHEMA.to_string(),
            package_id: "contra-sense/handbook".to_string(),
            package_version: "0.1.0".to_string(),
            lifecycle_generation: 7,
            generation_digest: digest(2),
            capability_snapshot_digest: String::new(),
            knowledge,
            limits: CognitiveContextLimits::default(),
        };
        binding.capability_snapshot_digest = capability_snapshot_digest(&binding).unwrap();
        binding.validate().unwrap();
        binding
    }

    fn response(request: &CognitiveContextRequestV1) -> CognitiveContextResponseV1 {
        let citation = CognitiveKnowledgeCitationV1::new(
            &request.binding,
            "concepts/retry-policy.md",
            "Retry policy",
            vec![digest(3)],
        )
        .unwrap();
        let document = CognitiveContextDocumentV1::new(
            citation,
            "Retry only before an observable side effect.",
        )
        .unwrap();
        CognitiveContextResponseV1::new(request, vec![document], false).unwrap()
    }

    #[test]
    fn exact_binding_rejects_latest_and_capability_drift() {
        let mut unpinned = binding();
        unpinned.lifecycle_generation = 0;
        unpinned.knowledge.lifecycle_generation = 0;
        assert!(matches!(
            unpinned.validate(),
            Err(CognitiveContextError::InvalidBinding(_))
        ));

        let mut drifted = binding();
        drifted.knowledge.content_digest = digest(9);
        assert!(matches!(
            drifted.validate(),
            Err(CognitiveContextError::InvalidBinding(_))
        ));
    }

    #[test]
    fn response_rejects_generation_citation_and_source_drift() {
        let request = CognitiveContextRequestV1::new("session-1", "retry", binding()).unwrap();

        let mut generation = response(&request);
        generation.binding.lifecycle_generation += 1;
        assert!(matches!(
            generation.validate_for(&request),
            Err(CognitiveContextError::ResponseDrift(_))
                | Err(CognitiveContextError::InvalidBinding(_))
        ));

        let mut citation = response(&request);
        citation.documents[0].citation.heading = "Substituted".to_string();
        assert!(matches!(
            citation.validate_for(&request),
            Err(CognitiveContextError::ResponseDrift(_))
        ));

        let mut source = response(&request);
        source.documents[0].content.push_str(" changed");
        source.documents[0].byte_count = source.documents[0].content.len();
        assert!(matches!(
            source.validate_for(&request),
            Err(CognitiveContextError::ResponseDrift(_))
        ));
    }

    #[test]
    fn response_rejects_empty_duplicate_and_unbounded_documents() {
        let request = CognitiveContextRequestV1::new("session-1", "retry", binding()).unwrap();
        assert!(CognitiveContextResponseV1::new(&request, Vec::new(), false).is_err());

        let valid = response(&request);
        let duplicate = vec![valid.documents[0].clone(), valid.documents[0].clone()];
        assert!(CognitiveContextResponseV1::new(&request, duplicate, false).is_err());

        let citation = CognitiveKnowledgeCitationV1::new(
            &request.binding,
            "concepts/large.md",
            "Large",
            vec![digest(4)],
        )
        .unwrap();
        let large = CognitiveContextDocumentV1::new(
            citation,
            "x".repeat(request.binding.limits.max_document_bytes + 1),
        )
        .unwrap();
        assert!(CognitiveContextResponseV1::new(&request, vec![large], false).is_err());
    }
}
