//! Environment-sourced harness configuration.
//!
//! Credentials are read from the process environment only. Nothing here is
//! serialized into a report except the host and an opaque endpoint digest.

use super::boyue::{BoyueEndpoint, DEFAULT_MODEL, DEFAULT_USD_PER_MILLION_TOKENS};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::time::Duration;

/// Default Redis database. Database 15 keeps qualification keys away from a
/// shared development instance's primary data.
pub const DEFAULT_REDIS_URL: &str = "redis://127.0.0.1:6379/15";
const DEFAULT_WRITERS: usize = 8;
/// The harness never renews the epoch lease, so the TTL has to outlive a full
/// qualification run (real embedding round trips dominate it).
const DEFAULT_LEASE_TTL_SECONDS: u64 = 600;

/// Fully resolved harness inputs.
pub struct HostConfig {
    pub endpoint: BoyueEndpoint,
    pub api_key: String,
    pub redis_url: String,
    pub redis_database: Option<u32>,
    pub key_prefix: String,
    pub pack_directory: PathBuf,
    pub revision: String,
    pub concurrent_writers: usize,
    pub lease_ttl: Duration,
}

impl HostConfig {
    pub fn from_environment() -> Result<Self> {
        let api_key = required("BOYUE_API_KEY")?;
        let base_url = required("BOYUE_BASE_URL")?;
        let model =
            optional("A3S_DM_PROD1_EMBED_MODEL").unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let usd_per_million_tokens = optional("A3S_DM_PROD1_USD_PER_MTOK")
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .unwrap_or(DEFAULT_USD_PER_MILLION_TOKENS);
        let redis_url =
            optional("A3S_DM_PROD1_REDIS_URL").unwrap_or_else(|| DEFAULT_REDIS_URL.to_string());
        let redis_database = parse_database(&redis_url);
        if redis_database.unwrap_or(0) == 0
            && optional("A3S_DM_PROD1_ALLOW_DEFAULT_DB").as_deref() != Some("1")
        {
            bail!(
                "refusing to run against Redis database 0; point A3S_DM_PROD1_REDIS_URL at a \
                 scratch database such as {DEFAULT_REDIS_URL}, or set \
                 A3S_DM_PROD1_ALLOW_DEFAULT_DB=1 to override"
            );
        }
        let revision = optional("A3S_DM_PROD1_REVISION").unwrap_or_else(|| "unpinned".to_string());
        let pack_directory = optional("A3S_DM_PROD1_PACK_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/tmp/dm-prod1-host-{revision}")));
        let run_id = uuid::Uuid::new_v4().simple().to_string();
        Ok(Self {
            endpoint: BoyueEndpoint {
                base_url,
                model,
                usd_per_million_tokens,
            },
            api_key,
            redis_url,
            redis_database,
            key_prefix: format!("a3s:dm-prod1:{}", &run_id[..12]),
            pack_directory,
            revision,
            concurrent_writers: optional("A3S_DM_PROD1_WRITERS")
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|writers| (2..=64).contains(writers))
                .unwrap_or(DEFAULT_WRITERS),
            lease_ttl: Duration::from_secs(
                optional("A3S_DM_PROD1_LEASE_TTL_SECONDS")
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|seconds| (1..=600).contains(seconds))
                    .unwrap_or(DEFAULT_LEASE_TTL_SECONDS),
            ),
        })
    }

    /// Redis endpoint without any inline credential, safe to record.
    pub fn redacted_redis_target(&self) -> String {
        match redis::parse_redis_url(&self.redis_url) {
            Some(url) => {
                let host = url.host_str().unwrap_or("unknown");
                let port = url.port().unwrap_or(6379);
                format!("redis://{host}:{port}/{}", self.redis_database.unwrap_or(0))
            }
            None => "unparsed".to_string(),
        }
    }
}

fn required(key: &str) -> Result<String> {
    let value = std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    value.with_context(|| {
        format!("{key} must be exported; source scripts/harbor/.env before running the harness")
    })
}

fn optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_database(url: &str) -> Option<u32> {
    redis::parse_redis_url(url)
        .and_then(|url| url.path_segments()?.next_back()?.parse::<u32>().ok())
}
