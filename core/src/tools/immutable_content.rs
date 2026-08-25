//! Host-injected retention for authorized immutable Tool content.
//!
//! The adapter is deliberately narrower than an object-store client. A host
//! binds it to an already-authorized content authority, while Code owns exact
//! byte measurement, digest validation, cancellation, and the Tool-result
//! reference that enters replayable metadata. Provider credentials, tenant
//! resolution, retention policy, and object lifecycle stay outside Core.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use thiserror::Error;

pub const IMMUTABLE_CONTENT_ADAPTER_BINDING_SCHEMA_V1: &str =
    "a3s.code.immutable-content-adapter-binding.v1";
pub const IMMUTABLE_CONTENT_DESCRIPTOR_SCHEMA_V1: &str = "a3s.code.immutable-content-descriptor.v1";
pub const IMMUTABLE_CONTENT_REFERENCE_SCHEMA_V1: &str = "a3s.code.immutable-content-reference.v1";
pub const TOOL_RESULT_CONTENT_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

const BINDING_DIGEST_DOMAIN: &str = "a3s.code.immutable-content-adapter-binding.v1";
const DESCRIPTOR_DIGEST_DOMAIN: &str = "a3s.code.immutable-content-descriptor.v1";
const REFERENCE_DIGEST_DOMAIN: &str = "a3s.code.immutable-content-reference.v1";
const MAX_PROVIDER_NAME_BYTES: usize = 128;
const MAX_REFERENCE_URI_BYTES: usize = 4 * 1024;
const MAX_MEDIA_TYPE_BYTES: usize = 255;

/// Validation and provider failures at the immutable-content boundary.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ImmutableContentError {
    #[error("invalid immutable content adapter binding: {0}")]
    InvalidBinding(String),
    #[error("invalid immutable content descriptor: {0}")]
    InvalidDescriptor(String),
    #[error("immutable content adapter failed: {0}")]
    Provider(String),
    #[error("immutable content reference drifted: {0}")]
    ReferenceDrift(String),
    #[error("immutable content retention was cancelled")]
    Cancelled,
}

pub type ImmutableContentResult<T> = std::result::Result<T, ImmutableContentError>;

impl ImmutableContentError {
    /// Bounded message safe for Tool errors and telemetry. Provider-supplied
    /// detail is intentionally excluded because it could repeat raw content.
    pub fn redacted_message(&self) -> &'static str {
        match self {
            Self::InvalidBinding(_) => "invalid immutable content adapter binding",
            Self::InvalidDescriptor(_) => "invalid immutable content descriptor",
            Self::Provider(_) => "immutable content provider failure",
            Self::ReferenceDrift(_) => "immutable content reference drift",
            Self::Cancelled => "immutable content retention cancelled",
        }
    }
}

/// Secret-free identity of the host authority bound to one Code session.
///
/// `authority_digest` is opaque to Code. The embedding host computes it from
/// its authorized provider/namespace/profile binding and must not include
/// plaintext tenant identifiers, credentials, endpoints, or object paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImmutableContentAdapterBindingV1 {
    pub schema: String,
    pub authority_digest: String,
    pub maximum_bytes: u64,
    pub binding_digest: String,
}

impl ImmutableContentAdapterBindingV1 {
    pub fn new(
        authority_digest: impl Into<String>,
        maximum_bytes: u64,
    ) -> ImmutableContentResult<Self> {
        let mut binding = Self {
            schema: IMMUTABLE_CONTENT_ADAPTER_BINDING_SCHEMA_V1.to_string(),
            authority_digest: authority_digest.into(),
            maximum_bytes,
            binding_digest: String::new(),
        };
        binding.binding_digest = binding.expected_digest()?;
        binding.validate()?;
        Ok(binding)
    }

    pub fn validate(&self) -> ImmutableContentResult<()> {
        if self.schema != IMMUTABLE_CONTENT_ADAPTER_BINDING_SCHEMA_V1 {
            return Err(invalid_binding("schema is unsupported"));
        }
        if !valid_sha256(&self.authority_digest) {
            return Err(invalid_binding(
                "authority_digest must be canonical lowercase SHA-256",
            ));
        }
        if self.maximum_bytes == 0 {
            return Err(invalid_binding("maximum_bytes must be positive"));
        }
        if !valid_sha256(&self.binding_digest) {
            return Err(invalid_binding(
                "binding_digest must be canonical lowercase SHA-256",
            ));
        }
        if self.binding_digest != self.expected_digest()? {
            return Err(invalid_binding(
                "binding_digest does not bind the authority and byte ceiling",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> ImmutableContentResult<String> {
        #[derive(Serialize)]
        struct DigestInput<'a> {
            schema: &'a str,
            authority_digest: &'a str,
            maximum_bytes: u64,
        }

        canonical_digest(
            BINDING_DIGEST_DOMAIN,
            &DigestInput {
                schema: &self.schema,
                authority_digest: &self.authority_digest,
                maximum_bytes: self.maximum_bytes,
            },
        )
        .map_err(invalid_binding)
    }
}

/// Closed purpose for one retained original-content object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImmutableContentKindV1 {
    ToolResultOriginal,
    ToolChangeBefore,
    ToolChangeAfter,
}

/// Content identity computed by Code before a provider is called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImmutableContentDescriptorV1 {
    pub schema: String,
    pub kind: ImmutableContentKindV1,
    pub media_type: String,
    pub size_bytes: u64,
    pub content_digest: String,
    pub descriptor_digest: String,
}

impl ImmutableContentDescriptorV1 {
    pub fn new(
        kind: ImmutableContentKindV1,
        media_type: impl Into<String>,
        content: &[u8],
    ) -> ImmutableContentResult<Self> {
        let size_bytes = u64::try_from(content.len()).map_err(|_| {
            invalid_descriptor("content size cannot be represented by the v1 byte counter")
        })?;
        let mut descriptor = Self {
            schema: IMMUTABLE_CONTENT_DESCRIPTOR_SCHEMA_V1.to_string(),
            kind,
            media_type: media_type.into(),
            size_bytes,
            content_digest: sha256(content),
            descriptor_digest: String::new(),
        };
        descriptor.descriptor_digest = descriptor.expected_digest()?;
        descriptor.validate_for(content)?;
        Ok(descriptor)
    }

    pub fn validate(&self) -> ImmutableContentResult<()> {
        if self.schema != IMMUTABLE_CONTENT_DESCRIPTOR_SCHEMA_V1 {
            return Err(invalid_descriptor("schema is unsupported"));
        }
        if self.media_type != TOOL_RESULT_CONTENT_MEDIA_TYPE
            || !valid_plain_value(&self.media_type, MAX_MEDIA_TYPE_BYTES)
        {
            return Err(invalid_descriptor(
                "media_type is not the exact v1 UTF-8 Tool-content type",
            ));
        }
        if !valid_sha256(&self.content_digest) || !valid_sha256(&self.descriptor_digest) {
            return Err(invalid_descriptor(
                "content and descriptor digests must be canonical lowercase SHA-256",
            ));
        }
        if self.descriptor_digest != self.expected_digest()? {
            return Err(invalid_descriptor(
                "descriptor_digest does not bind the exact content identity",
            ));
        }
        Ok(())
    }

    pub fn validate_for(&self, content: &[u8]) -> ImmutableContentResult<()> {
        self.validate()?;
        let size_bytes = u64::try_from(content.len()).map_err(|_| {
            invalid_descriptor("content size cannot be represented by the v1 byte counter")
        })?;
        if self.size_bytes != size_bytes || self.content_digest != sha256(content) {
            return Err(invalid_descriptor(
                "descriptor does not match the exact supplied content",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> ImmutableContentResult<String> {
        #[derive(Serialize)]
        struct DigestInput<'a> {
            schema: &'a str,
            kind: ImmutableContentKindV1,
            media_type: &'a str,
            size_bytes: u64,
            content_digest: &'a str,
        }

        canonical_digest(
            DESCRIPTOR_DIGEST_DOMAIN,
            &DigestInput {
                schema: &self.schema,
                kind: self.kind,
                media_type: &self.media_type,
                size_bytes: self.size_bytes,
                content_digest: &self.content_digest,
            },
        )
        .map_err(invalid_descriptor)
    }
}

/// Provider-neutral immutable reference returned by the authorized adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImmutableContentReferenceV1 {
    pub schema: String,
    pub binding_digest: String,
    pub uri: String,
    pub content_digest: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub reference_digest: String,
}

impl ImmutableContentReferenceV1 {
    pub fn new(
        binding: &ImmutableContentAdapterBindingV1,
        descriptor: &ImmutableContentDescriptorV1,
        uri: impl Into<String>,
    ) -> ImmutableContentResult<Self> {
        binding.validate()?;
        descriptor.validate()?;
        ensure_within_binding(binding, descriptor)?;
        let mut reference = Self {
            schema: IMMUTABLE_CONTENT_REFERENCE_SCHEMA_V1.to_string(),
            binding_digest: binding.binding_digest.clone(),
            uri: uri.into(),
            content_digest: descriptor.content_digest.clone(),
            media_type: descriptor.media_type.clone(),
            size_bytes: descriptor.size_bytes,
            reference_digest: String::new(),
        };
        reference.reference_digest = reference.expected_digest()?;
        reference.validate_for(binding, descriptor)?;
        Ok(reference)
    }

    pub fn validate(&self) -> ImmutableContentResult<()> {
        if self.schema != IMMUTABLE_CONTENT_REFERENCE_SCHEMA_V1 {
            return Err(reference_drift("schema is unsupported"));
        }
        if !valid_reference_uri(&self.uri) {
            return Err(reference_drift(
                "uri must be a bounded absolute logical reference without userinfo, query, fragment, whitespace, control, or backslash characters",
            ));
        }
        let content_digest = self
            .content_digest
            .strip_prefix("sha256:")
            .unwrap_or_default();
        if !self.uri.contains(content_digest) {
            return Err(reference_drift(
                "uri is not content-addressed by the exact SHA-256 digest",
            ));
        }
        if !valid_sha256(&self.binding_digest)
            || !valid_sha256(&self.content_digest)
            || !valid_sha256(&self.reference_digest)
        {
            return Err(reference_drift(
                "binding, content, and reference digests must be canonical lowercase SHA-256",
            ));
        }
        if self.media_type != TOOL_RESULT_CONTENT_MEDIA_TYPE
            || !valid_plain_value(&self.media_type, MAX_MEDIA_TYPE_BYTES)
        {
            return Err(reference_drift(
                "media_type is not the exact v1 UTF-8 Tool-content type",
            ));
        }
        if self.reference_digest != self.expected_digest()? {
            return Err(reference_drift(
                "reference_digest does not bind the logical URI and content identity",
            ));
        }
        Ok(())
    }

    pub fn validate_for(
        &self,
        binding: &ImmutableContentAdapterBindingV1,
        descriptor: &ImmutableContentDescriptorV1,
    ) -> ImmutableContentResult<()> {
        binding.validate()?;
        descriptor.validate()?;
        ensure_within_binding(binding, descriptor)?;
        self.validate()?;
        if self.binding_digest != binding.binding_digest
            || self.content_digest != descriptor.content_digest
            || self.media_type != descriptor.media_type
            || self.size_bytes != descriptor.size_bytes
        {
            return Err(reference_drift(
                "reference does not match the session binding and exact content descriptor",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> ImmutableContentResult<String> {
        #[derive(Serialize)]
        struct DigestInput<'a> {
            schema: &'a str,
            binding_digest: &'a str,
            uri: &'a str,
            content_digest: &'a str,
            media_type: &'a str,
            size_bytes: u64,
        }

        canonical_digest(
            REFERENCE_DIGEST_DOMAIN,
            &DigestInput {
                schema: &self.schema,
                binding_digest: &self.binding_digest,
                uri: &self.uri,
                content_digest: &self.content_digest,
                media_type: &self.media_type,
                size_bytes: self.size_bytes,
            },
        )
        .map_err(reference_drift)
    }
}

/// Borrowed provider request. Its `Debug` representation excludes content.
pub struct ImmutableContentWriteRequestV1<'a> {
    binding: &'a ImmutableContentAdapterBindingV1,
    descriptor: &'a ImmutableContentDescriptorV1,
    content: &'a [u8],
}

impl std::fmt::Debug for ImmutableContentWriteRequestV1<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImmutableContentWriteRequestV1")
            .field("binding", self.binding)
            .field("descriptor", self.descriptor)
            .field("content", &"<redacted>")
            .finish()
    }
}

impl<'a> ImmutableContentWriteRequestV1<'a> {
    fn new(
        binding: &'a ImmutableContentAdapterBindingV1,
        descriptor: &'a ImmutableContentDescriptorV1,
        content: &'a [u8],
    ) -> ImmutableContentResult<Self> {
        binding.validate()?;
        descriptor.validate_for(content)?;
        ensure_within_binding(binding, descriptor)?;
        Ok(Self {
            binding,
            descriptor,
            content,
        })
    }

    pub fn binding(&self) -> &ImmutableContentAdapterBindingV1 {
        self.binding
    }

    pub fn descriptor(&self) -> &ImmutableContentDescriptorV1 {
        self.descriptor
    }

    pub fn content(&self) -> &[u8] {
        self.content
    }
}

/// Host port for create-only, exact-replay immutable content retention.
///
/// The host must scope this object to an authorization already resolved
/// outside Code. Repeating an identical descriptor must either return the
/// byte-equivalent reference or fail; it must never overwrite another object.
#[async_trait::async_trait]
pub trait ImmutableContentAdapter: Send + Sync {
    fn name(&self) -> &str;

    async fn put(
        &self,
        request: &ImmutableContentWriteRequestV1<'_>,
    ) -> ImmutableContentResult<ImmutableContentReferenceV1>;
}

/// Runtime pairing of a durable authority binding with one host adapter.
#[derive(Clone)]
pub struct ImmutableContentAdapterSession {
    binding: ImmutableContentAdapterBindingV1,
    adapter: Arc<dyn ImmutableContentAdapter>,
}

impl std::fmt::Debug for ImmutableContentAdapterSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImmutableContentAdapterSession")
            .field("binding", &self.binding)
            .field("adapter", &self.adapter.name())
            .finish()
    }
}

impl ImmutableContentAdapterSession {
    pub fn new(
        binding: ImmutableContentAdapterBindingV1,
        adapter: Arc<dyn ImmutableContentAdapter>,
    ) -> ImmutableContentResult<Self> {
        binding.validate()?;
        if !valid_plain_value(adapter.name(), MAX_PROVIDER_NAME_BYTES) {
            return Err(ImmutableContentError::Provider(
                "adapter name is empty, unbounded, or contains control characters".to_string(),
            ));
        }
        Ok(Self { binding, adapter })
    }

    pub fn binding(&self) -> &ImmutableContentAdapterBindingV1 {
        &self.binding
    }

    pub fn adapter_name(&self) -> &str {
        self.adapter.name()
    }

    pub async fn put(
        &self,
        kind: ImmutableContentKindV1,
        media_type: &str,
        content: &[u8],
    ) -> ImmutableContentResult<ImmutableContentReferenceV1> {
        let descriptor = ImmutableContentDescriptorV1::new(kind, media_type, content)?;
        let request = ImmutableContentWriteRequestV1::new(&self.binding, &descriptor, content)?;
        let reference = self.adapter.put(&request).await?;
        reference.validate_for(&self.binding, &descriptor)?;
        Ok(reference)
    }
}

fn ensure_within_binding(
    binding: &ImmutableContentAdapterBindingV1,
    descriptor: &ImmutableContentDescriptorV1,
) -> ImmutableContentResult<()> {
    if descriptor.size_bytes > binding.maximum_bytes {
        return Err(invalid_descriptor(
            "content exceeds the session's immutable-content byte ceiling",
        ));
    }
    Ok(())
}

fn canonical_digest(value_domain: &str, value: &impl Serialize) -> Result<String, String> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| format!("could not encode canonical digest input: {error}"))?;
    let mut hasher = Sha256::new();
    hasher.update(value_domain.as_bytes());
    hasher.update([0]);
    hasher.update(encoded);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn sha256(content: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(content))
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

fn valid_plain_value(value: &str, maximum_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum_bytes
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn valid_reference_uri(value: &str) -> bool {
    if !valid_plain_value(value, MAX_REFERENCE_URI_BYTES)
        || value.chars().any(char::is_whitespace)
        || value.contains(['?', '#', '@', '\\'])
    {
        return false;
    }
    let Some((scheme, remainder)) = value.split_once("://") else {
        return false;
    };
    let mut characters = scheme.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
        && !remainder.is_empty()
}

fn invalid_binding(message: impl Into<String>) -> ImmutableContentError {
    ImmutableContentError::InvalidBinding(message.into())
}

fn invalid_descriptor(message: impl Into<String>) -> ImmutableContentError {
    ImmutableContentError::InvalidDescriptor(message.into())
}

fn reference_drift(message: impl Into<String>) -> ImmutableContentError {
    ImmutableContentError::ReferenceDrift(message.into())
}
