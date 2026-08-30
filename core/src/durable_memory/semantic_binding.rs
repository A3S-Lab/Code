use crate::embedding::{EmbeddingError, EmbeddingExecutorConfig, EmbeddingProviderDescriptor};
use a3s_memory::repository::{MemoryRepositoryError, MAX_QUERY_LIMIT};
use a3s_memory::vector::{
    VectorIndexDescriptor, VectorIndexError, VectorMetric, VectorMutationConsistency,
    VectorNormalization,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1: &str =
    "a3s.code.memory.semantic-recall-binding.v1";
pub const DURABLE_MEMORY_SEMANTIC_FUSION_PROFILE_V1: &str = "a3s.code.memory.hybrid.rrf-k60.v1";

const EXECUTION_POLICY_DIGEST_DOMAIN: &str = "a3s.code.memory.semantic-embedding-execution.v1";
const SERVING_GENERATION_DIGEST_DOMAIN: &str = "a3s.code.memory.semantic-serving-generation.v1";
const MAX_DESCRIPTOR_TEXT_BYTES: usize = 256;
const MAX_SEMANTIC_EMBEDDING_DIMENSION: usize = 65_536;

/// Bounded candidate policy for one host-owned semantic recall generation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DurableMemorySemanticRecallPolicy {
    candidate_limit: usize,
    min_score: f32,
}

impl DurableMemorySemanticRecallPolicy {
    pub fn try_new(
        candidate_limit: usize,
        min_score: f32,
    ) -> Result<Self, DurableMemorySemanticError> {
        let policy = Self {
            candidate_limit,
            min_score,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn candidate_limit(self) -> usize {
        self.candidate_limit
    }

    pub fn min_score(self) -> f32 {
        self.min_score
    }

    pub(crate) fn validate(self) -> Result<(), DurableMemorySemanticError> {
        if !(1..=MAX_QUERY_LIMIT).contains(&self.candidate_limit) {
            return Err(invalid(
                "policy.candidateLimit",
                format!("must be between 1 and {MAX_QUERY_LIMIT}"),
            ));
        }
        if !self.min_score.is_finite() || !(0.0..=1.0).contains(&self.min_score) {
            return Err(invalid(
                "policy.minScore",
                "must be finite and between 0 and 1",
            ));
        }
        Ok(())
    }
}

/// Secret-free semantic authority and retrieval identity persisted with a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DurableMemorySemanticBindingV1 {
    schema: String,
    authority_digest: String,
    embedding: EmbeddingProviderDescriptor,
    embedding_execution_digest: String,
    vector_index: VectorIndexDescriptor,
    policy: DurableMemorySemanticRecallPolicy,
    fusion_profile: String,
}

impl DurableMemorySemanticBindingV1 {
    pub(crate) fn new(
        authority_digest: String,
        embedding: EmbeddingProviderDescriptor,
        embedding_config: EmbeddingExecutorConfig,
        vector_index: VectorIndexDescriptor,
        policy: DurableMemorySemanticRecallPolicy,
    ) -> Result<Self, DurableMemorySemanticError> {
        let binding = Self {
            schema: DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1.to_string(),
            authority_digest,
            embedding,
            embedding_execution_digest: execution_policy_digest(embedding_config)?,
            vector_index,
            policy,
            fusion_profile: DURABLE_MEMORY_SEMANTIC_FUSION_PROFILE_V1.to_string(),
        };
        binding.validate()?;
        Ok(binding)
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn authority_digest(&self) -> &str {
        &self.authority_digest
    }

    pub fn embedding(&self) -> &EmbeddingProviderDescriptor {
        &self.embedding
    }

    pub fn embedding_execution_digest(&self) -> &str {
        &self.embedding_execution_digest
    }

    pub fn vector_index(&self) -> &VectorIndexDescriptor {
        &self.vector_index
    }

    pub fn policy(&self) -> DurableMemorySemanticRecallPolicy {
        self.policy
    }

    pub fn fusion_profile(&self) -> &str {
        &self.fusion_profile
    }

    pub(crate) fn serving_generation_digest(&self) -> Result<String, DurableMemorySemanticError> {
        let encoded = serde_json::to_vec(self).map_err(|error| {
            invalid(
                "binding.servingGenerationDigest",
                format!("could not encode semantic binding: {error}"),
            )
        })?;
        let mut hasher = Sha256::new();
        hasher.update(SERVING_GENERATION_DIGEST_DOMAIN.as_bytes());
        hasher.update([0]);
        hasher.update(encoded);
        Ok(format!("sha256:{:x}", hasher.finalize()))
    }

    pub(crate) fn validate(&self) -> Result<(), DurableMemorySemanticError> {
        if self.schema != DURABLE_MEMORY_SEMANTIC_BINDING_SCHEMA_V1 {
            return Err(invalid("binding.schema", "is unsupported"));
        }
        if !valid_sha256(&self.authority_digest) {
            return Err(invalid(
                "binding.authorityDigest",
                "must be canonical lowercase SHA-256",
            ));
        }
        validate_descriptor_text("binding.embedding.provider", &self.embedding.provider)?;
        validate_descriptor_text("binding.embedding.model", &self.embedding.model)?;
        let revision = self.embedding.revision.as_deref().ok_or_else(|| {
            invalid(
                "binding.embedding.revision",
                "is required for durable semantic serving",
            )
        })?;
        validate_descriptor_text("binding.embedding.revision", revision)?;
        if !(1..=MAX_SEMANTIC_EMBEDDING_DIMENSION).contains(&self.embedding.dimension) {
            return Err(invalid(
                "binding.embedding.dimension",
                format!("must be between 1 and {MAX_SEMANTIC_EMBEDDING_DIMENSION}"),
            ));
        }
        if !valid_sha256(&self.embedding_execution_digest) {
            return Err(invalid(
                "binding.embeddingExecutionDigest",
                "must be canonical lowercase SHA-256",
            ));
        }
        if self.vector_index.dimension != self.embedding.dimension {
            return Err(invalid(
                "binding.vectorIndex.dimension",
                "must match the embedding generation",
            ));
        }
        if self.vector_index.metric != VectorMetric::Cosine
            || self.vector_index.normalization != VectorNormalization::Unit
        {
            return Err(invalid(
                "binding.vectorIndex",
                "semantic recall v1 requires unit-normalized cosine search",
            ));
        }
        if self.vector_index.max_records == 0 || self.vector_index.max_bytes == 0 {
            return Err(invalid(
                "binding.vectorIndex",
                "record and byte budgets must be greater than zero",
            ));
        }
        let vector_bytes = self
            .vector_index
            .dimension
            .checked_mul(std::mem::size_of::<f32>())
            .ok_or_else(|| invalid("binding.vectorIndex.dimension", "size overflow"))?;
        if vector_bytes > self.vector_index.max_bytes {
            return Err(invalid(
                "binding.vectorIndex.maxBytes",
                "must admit at least one vector",
            ));
        }
        self.policy.validate()?;
        if self.fusion_profile != DURABLE_MEMORY_SEMANTIC_FUSION_PROFILE_V1 {
            return Err(invalid("binding.fusionProfile", "is unsupported"));
        }
        Ok(())
    }
}

/// Typed failures at the host-owned semantic recall boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DurableMemorySemanticError {
    #[error("invalid durable-memory semantic configuration for {field}: {reason}")]
    InvalidConfiguration { field: &'static str, reason: String },
    #[error("durable-memory semantic embedding failed: {0}")]
    Embedding(#[from] EmbeddingError),
    #[error("durable-memory semantic vector operation failed: {0}")]
    Vector(#[from] VectorIndexError),
    #[error("durable-memory semantic repository verification failed: {0}")]
    Repository(#[from] MemoryRepositoryError),
    #[error("durable-memory semantic index revision changed during verification")]
    IndexRevisionChanged,
    #[error("durable-memory repository changed during semantic index refresh")]
    RepositoryChangedDuringRefresh,
    #[error(
        "durable-memory semantic refresh requires {required:?} vector mutation consistency, but the backend provides {actual:?}"
    )]
    MutationConsistencyInsufficient {
        required: VectorMutationConsistency,
        actual: VectorMutationConsistency,
    },
}

impl DurableMemorySemanticError {
    pub fn redacted_message(&self) -> &'static str {
        match self {
            Self::InvalidConfiguration { .. } => "invalid semantic recall configuration",
            Self::Embedding(_) => "semantic query embedding failed",
            Self::Vector(_) => "semantic vector search failed",
            Self::Repository(_) => "semantic repository verification failed",
            Self::IndexRevisionChanged => "semantic index revision changed",
            Self::RepositoryChangedDuringRefresh => {
                "semantic repository changed during index refresh"
            }
            Self::MutationConsistencyInsufficient { .. } => {
                "semantic vector mutation consistency is insufficient"
            }
        }
    }
}

fn execution_policy_digest(
    config: EmbeddingExecutorConfig,
) -> Result<String, DurableMemorySemanticError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DigestInput {
        max_batch_inputs: usize,
        max_batch_text_bytes: usize,
        max_input_text_bytes: usize,
        max_request_inputs: usize,
        max_request_text_bytes: usize,
        max_batch_vector_bytes: usize,
        max_request_vector_bytes: usize,
        max_retries: u32,
        base_retry_delay_secs: u64,
        base_retry_delay_nanos: u32,
        max_retry_delay_secs: u64,
        max_retry_delay_nanos: u32,
        request_timeout_secs: u64,
        request_timeout_nanos: u32,
    }

    let input = DigestInput {
        max_batch_inputs: config.max_batch_inputs,
        max_batch_text_bytes: config.max_batch_text_bytes,
        max_input_text_bytes: config.max_input_text_bytes,
        max_request_inputs: config.max_request_inputs,
        max_request_text_bytes: config.max_request_text_bytes,
        max_batch_vector_bytes: config.max_batch_vector_bytes,
        max_request_vector_bytes: config.max_request_vector_bytes,
        max_retries: config.max_retries,
        base_retry_delay_secs: config.base_retry_delay.as_secs(),
        base_retry_delay_nanos: config.base_retry_delay.subsec_nanos(),
        max_retry_delay_secs: config.max_retry_delay.as_secs(),
        max_retry_delay_nanos: config.max_retry_delay.subsec_nanos(),
        request_timeout_secs: config.request_timeout.as_secs(),
        request_timeout_nanos: config.request_timeout.subsec_nanos(),
    };
    let encoded = serde_json::to_vec(&input).map_err(|error| {
        invalid(
            "binding.embeddingExecutionDigest",
            format!("could not encode execution policy: {error}"),
        )
    })?;
    let mut hasher = Sha256::new();
    hasher.update(EXECUTION_POLICY_DIGEST_DOMAIN.as_bytes());
    hasher.update([0]);
    hasher.update(encoded);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn validate_descriptor_text(
    field: &'static str,
    value: &str,
) -> Result<(), DurableMemorySemanticError> {
    if value.is_empty()
        || value.len() > MAX_DESCRIPTOR_TEXT_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(invalid(
            field,
            "must be bounded, trimmed, non-empty text without control characters",
        ));
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

pub(crate) fn invalid(
    field: &'static str,
    reason: impl Into<String>,
) -> DurableMemorySemanticError {
    DurableMemorySemanticError::InvalidConfiguration {
        field,
        reason: reason.into(),
    }
}
