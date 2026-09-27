//! Builds the Imagine tool configs (`image_gen`, `image_edit`, and the video tools) from the
//! resolved agent config.
//!
//! Every host that runs these tools goes through here: the embedded agent and the pager's tool
//! bridge for the daemon backend. The base URL, the client headers that attribute Build traffic
//! server-side, and the feature gates come from one place so the two hosts cannot drift.
//!
//! The configured `api_key` is the side-call bearer at build time, never `sampling_config.api_key`:
//! on a foreign-issuer login that field holds the foreign session token, which must not reach a
//! client that posts to `api.a3s`. The per-request provider still decides what goes on the wire.

use a3s_code_tools::implementations::grok_build::image_gen::ImageGenConfig;
use a3s_code_tools::implementations::grok_build::video_gen::VideoGenConfig;

use crate::agent::config::{AgentMode, Config, RuntimeResolutionContext};
use crate::agent::proxy_headers::inject_proxy_headers;
use crate::util::config::RemoteSettings;

pub struct MediaToolCredentials {
    /// `None`: the per-request provider is the only source, so a foreign login never reaches the client.
    pub static_bearer: Option<String>,
    /// Always `false` for a foreign-issuer login: that account is metered by its own gate, not A3S Code.
    pub tier_restricted: bool,
}

/// The config a host outside the embedded agent builds media tools from.
///
/// Parses the effective config and runs the same runtime resolution the agent runs at boot, which is
/// where the ZDR guard (`disable_zdr_incompatible_tools`, `zdr_video_output_s3`) and the media
/// concurrency caps are computed; a config that skipped it would advertise video output a ZDR customer
/// forbids. `remote_settings` is what the host has; `None` leaves the remote layers unset.
///
/// # Errors
///
/// Returns the config parse error text.
pub fn load_media_tool_config(
    raw_config: &toml::Value,
    remote_settings: Option<&RemoteSettings>,
) -> Result<Config, String> {
    let mut config = Config::new_from_toml_cfg(raw_config)?;
    config.remote_settings = remote_settings.cloned();
    config.resolve_runtime_fields(&RuntimeResolutionContext {
        raw_config,
        remote_settings,
        is_headless: config.mode == AgentMode::Headless,
        cli_subagents: None,
        cli_web_search_model: None,
        cli_session_summary_model: None,
        memory_enabled_override: None,
        disable_web_search: false,
        todo_gate: false,
        laziness_debug_log: None,
        storage_mode: None,
    });
    Ok(config)
}

pub fn image_gen_config(cfg: &Config, credentials: &MediaToolCredentials) -> ImageGenConfig {
    let base_url = cfg.endpoints.a3s_api_base_url.clone();
    ImageGenConfig::Enabled {
        api_key: credentials.static_bearer.clone(),
        extra_headers: media_headers(cfg, &base_url),
        base_url,
        image_gen_enabled: cfg.resolve_image_gen().value,
        image_edit_enabled: cfg.resolve_image_edit().value,
        model_override: cfg.resolve_image_gen_model_override(),
        edit_model_override: cfg.resolve_image_edit_model_override(),
        tier_restricted: credentials.tier_restricted,
    }
}

pub fn video_gen_config(cfg: &Config, credentials: &MediaToolCredentials) -> VideoGenConfig {
    if !cfg.resolve_video_gen().value {
        return VideoGenConfig::Disabled;
    }
    let zdr_video_output_s3 = cfg
        .disable_zdr_incompatible_tools
        .then(|| cfg.zdr_video_output_s3.clone())
        .flatten()
        .filter(|s3| s3.is_valid());
    // ZDR with no output bucket: advertised but fails at call time with ZDR_RESTRICTED_MESSAGE, not silently dropped
    let zdr_restricted = cfg.disable_zdr_incompatible_tools && zdr_video_output_s3.is_none();
    if zdr_restricted {
        tracing::info!("video_gen zdr-restricted by tools.disable_zdr_incompatible_tools");
    }
    let base_url = cfg.endpoints.a3s_api_base_url.clone();
    VideoGenConfig::Enabled {
        api_key: credentials.static_bearer.clone(),
        extra_headers: media_headers(cfg, &base_url),
        base_url,
        zdr_video_output_s3: zdr_video_output_s3.map(Box::new),
        tier_restricted: credentials.tier_restricted,
        zdr_restricted,
    }
}

fn media_headers(cfg: &Config, base_url: &str) -> indexmap::IndexMap<String, String> {
    let version = cfg
        .client_version
        .clone()
        .unwrap_or_else(|| a3s_code_version::VERSION.to_owned());
    let mut headers = indexmap::IndexMap::new();
    headers.insert("user-agent".to_owned(), format!("a3s-code-build/{version}"));
    inject_proxy_headers(
        &mut headers,
        cfg.client_version.as_deref(),
        cfg.endpoints.alpha_test_key.as_deref(),
        base_url,
    );
    headers
}

#[cfg(test)]
#[path = "media_tool_config_tests.rs"]
mod tests;
