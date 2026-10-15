//! Closed kernel for A3S Code.
//!
//! This crate owns operation identity, the logical clock, and domain-separated
//! digests. `a3s-code-core` re-exports these types and keeps Agent event
//! adaptation, the compatibility event log, and capability projection.
//!
//! The crate has no product-profile features. Browser engines, S3, and the
//! native vector index are not dependencies of this package.

pub mod clock;
pub mod content_digest;
pub mod identity;

pub use clock::{LogicalClock, ManualLogicalClock, SystemLogicalClock};
pub use content_digest::{digest_bytes, digest_json, validate_digest, InvalidDigest};
pub use identity::{
    ArtifactRef, CapabilityStamp, CoreIdentity, CoreIdentityError, EvidenceCursor, OperationId,
    SourceRevision, CORE_EVENT_IDENTITY_DIGEST_DOMAIN_V1, CORE_EVENT_IDENTITY_SCHEMA_V1,
    CORE_EVENT_PAYLOAD_DIGEST_DOMAIN_V1, CORE_IDENTITY_MAX_ARTIFACT_BYTES,
    CORE_IDENTITY_MAX_EVENT_TYPE_BYTES, CORE_IDENTITY_MAX_ID_BYTES,
    CORE_IDENTITY_MAX_MEDIA_TYPE_BYTES, CORE_IDENTITY_MAX_PAYLOAD_BYTES, CORE_IDENTITY_SCHEMA_V1,
};

#[cfg(test)]
mod manifest_boundary {
    #[test]
    fn manifest_does_not_name_browser_s3_or_zvec() {
        let manifest = include_str!("../Cargo.toml");
        for name in [
            "a3s-search",
            "a3s-vec",
            "aws-sdk-s3",
            "aws-sdk",
            "zvec",
            "lightpanda",
            "chromium",
            "rquickjs",
            "opentelemetry",
        ] {
            assert!(!manifest.contains(name), "kernel manifest names {name}");
        }
    }
}
