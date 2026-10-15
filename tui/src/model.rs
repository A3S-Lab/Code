//! ACL model selection for the TUI.
//!
//! The product configuration is the A3S ACL file. An `XAI_API_KEY` in the
//! environment is not consulted.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use a3s_code_core::config::ProviderConfig;
use a3s_code_core::CodeConfig;

/// Model identity taken from ACL `default_model`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModel {
    pub model_id: String,
    pub provider: String,
}

/// Resolve the model the TUI will call.
///
/// `default_model` is `provider/model`. The provider is the segment before
/// the first slash. This function does not read `XAI_API_KEY`.
pub fn resolve_acl_model(acl: &str) -> Result<ResolvedModel, String> {
    let config = CodeConfig::from_acl(acl).map_err(|error| error.to_string())?;
    let model_id = config
        .default_model
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "default_model must be set in provider/model format".to_string())?;
    let provider = model_id
        .split('/')
        .next()
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown")
        .to_string();
    Ok(ResolvedModel { model_id, provider })
}

/// Read an ACL file and resolve `default_model`.
///
/// This is the file entry the full-screen TUI uses at launch. It does not
/// read `XAI_API_KEY`.
pub fn resolve_acl_file(path: &Path) -> Result<ResolvedModel, String> {
    let acl = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    resolve_acl_model(&acl)
}

/// Inputs for the same ACL stack `a3s code` merges before the first turn.
///
/// An explicit config replaces the user and workspace layers. Otherwise the
/// user file is read first and the workspace file overlays it. `A3S_DEFAULT_MODEL`
/// then replaces `default_model`. `XAI_API_KEY` is not read.
#[derive(Debug, Clone)]
pub struct LaunchLayers {
    pub workspace: PathBuf,
    pub explicit_config: Option<PathBuf>,
    pub home: Option<PathBuf>,
}

/// Merged ACL text, model identity, and provider names from [`LaunchLayers`].
#[derive(Debug, Clone)]
pub struct MergedLaunch {
    pub model_id: String,
    pub provider: String,
    pub provider_names: Vec<String>,
    pub config: CodeConfig,
}

/// Merge the CLI layer stack and return the model the TUI will call.
pub fn merge_launch_layers(layers: &LaunchLayers) -> Result<MergedLaunch, String> {
    let sources = layer_sources(layers)?;
    let layer_acls = sources
        .iter()
        .map(|layer| layer.acl.as_str())
        .collect::<Vec<_>>();
    let mut config = CodeConfig::from_acl_layers(&layer_acls)
        .map_err(|error| format!("failed to parse merged A3S ACL: {error}"))?;
    // Host-authoritative blocks, including continuation policy, stay with the
    // trusted layers — the same choke point the ACP launch path applies
    // (ratified 2026-10-09). The thin scaffold has no tracing; the
    // withheld signal rides stderr.
    let withheld =
        a3s_code_core::config::withhold_untrusted_host_authoritative_blocks(&mut config, &sources);
    if !withheld.is_empty() {
        eprintln!(
            "workspace configuration layer sets host-authoritative block(s) {withheld:?}; \
             host-authoritative settings stay with the host layer \
             (~/.a3s/config.acl or --config)"
        );
    }
    normalize_providers(&mut config.providers);
    apply_default_model_env(&mut config)?;
    let model_id = config
        .default_model
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "default_model must be set in provider/model format".to_string())?;
    let Some((provider, model)) = model_id.split_once('/') else {
        return Err(format!(
            "merged model `{model_id}` must use provider/model format"
        ));
    };
    let provider = provider.to_string();
    let declared = config
        .find_provider(&provider)
        .and_then(|provider_config| provider_config.find_model(model))
        .is_some();
    if provider.is_empty() || model.is_empty() || !declared {
        return Err(format!(
            "merged model `{model_id}` is not declared by the merged providers"
        ));
    }
    let mut provider_names: Vec<String> = config
        .providers
        .iter()
        .map(|provider| provider.name.clone())
        .collect();
    provider_names.sort();
    Ok(MergedLaunch {
        model_id,
        provider,
        provider_names,
        config,
    })
}

fn layer_sources(layers: &LaunchLayers) -> Result<Vec<a3s_code_core::config::LayerAcl>, String> {
    if let Some(path) = layers.explicit_config.as_deref() {
        return Ok(vec![trusted_layer(read_layer(path)?)]);
    }
    let mut sources = Vec::new();
    let user = layers
        .home
        .as_deref()
        .map(|home| home.join(".a3s/config.acl"))
        .filter(|path| path.is_file());
    if let Some(path) = user.as_deref() {
        sources.push(trusted_layer(read_layer(path)?));
    }
    if let Some(path) = workspace_config_path(&layers.workspace) {
        if user
            .as_deref()
            .is_none_or(|user_path| !same_file(user_path, &path))
        {
            // An automatically discovered workspace layer is untrusted
            // repository content: it may configure the agent, never the
            // host-authoritative blocks.
            sources.push(a3s_code_core::config::LayerAcl {
                trusted: false,
                acl: read_layer(&path)?,
            });
        }
    }
    if sources.is_empty() {
        return Err(
            "A3S ACL configuration was not found; pass --config or create ~/.a3s/config.acl"
                .to_string(),
        );
    }
    Ok(sources)
}

fn trusted_layer(acl: String) -> a3s_code_core::config::LayerAcl {
    a3s_code_core::config::LayerAcl { trusted: true, acl }
}

/// Size-bounded, symlink-refusing layer read (core's `bounded_io` is
/// `pub(crate)`, so the thin scaffold keeps a local twin — the same split
/// the ACP layer and the CLI made).
fn read_layer(path: &Path) -> Result<String, String> {
    const MAX_ACL_LAYER_BYTES: u64 = 1024 * 1024;
    let read = || -> std::io::Result<String> {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.is_symlink() {
            return Err(std::io::Error::other("layer must be a regular file"));
        }
        if metadata.len() > MAX_ACL_LAYER_BYTES {
            return Err(std::io::Error::other("layer exceeds the 1 MiB bound"));
        }
        let file = std::fs::File::open(path)?;
        // take() bounds the read even when the file grows between stat
        // and read.
        let mut acl = String::new();
        std::io::Read::read_to_string(&mut file.take(MAX_ACL_LAYER_BYTES + 1), &mut acl)?;
        Ok(acl)
    };
    read().map_err(|error| format!("could not read A3S ACL {}: {error}", path.display()))
}

fn workspace_config_path(workspace: &Path) -> Option<PathBuf> {
    workspace
        .ancestors()
        .map(|directory| directory.join(".a3s/config.acl"))
        .find(|candidate| candidate.is_file())
}

fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn apply_default_model_env(config: &mut CodeConfig) -> Result<(), String> {
    let Some(value) = std::env::var_os("A3S_DEFAULT_MODEL") else {
        return Ok(());
    };
    if value.is_empty() {
        return Ok(());
    }
    let model = value
        .into_string()
        .map_err(|_| "A3S_DEFAULT_MODEL must be valid UTF-8".to_string())?;
    let Some((provider, id)) = model.split_once('/') else {
        return Err("A3S_DEFAULT_MODEL must use the provider/model format".to_string());
    };
    // Model ids may themselves contain `/` (e.g. boyue/bailian/...); the
    // core resolver splits on the first slash, so both parts only need to
    // be non-empty. (The ACP path already accepts these ids.)
    if provider.is_empty() || id.is_empty() {
        return Err("A3S_DEFAULT_MODEL must use a non-empty provider/model identifier".to_string());
    }
    config.default_model = Some(model);
    Ok(())
}

fn normalize_providers(providers: &mut Vec<ProviderConfig>) {
    let mut merged: Vec<ProviderConfig> = Vec::new();
    for provider in std::mem::take(providers) {
        if let Some(existing) = merged
            .iter_mut()
            .find(|candidate| candidate.name == provider.name)
        {
            merge_provider(existing, provider);
        } else {
            merged.push(provider);
        }
    }
    *providers = merged;
}

fn merge_provider(base: &mut ProviderConfig, overlay: ProviderConfig) {
    if overlay.api_key.is_some() {
        base.api_key = overlay.api_key;
    }
    if overlay.base_url.is_some() {
        base.base_url = overlay.base_url;
    }
    if overlay.session_id_header.is_some() {
        base.session_id_header = overlay.session_id_header;
    }
    base.headers.extend(overlay.headers);
    for model in overlay.models {
        if let Some(index) = base
            .models
            .iter()
            .position(|candidate| candidate.id == model.id)
        {
            base.models[index] = model;
        } else {
            base.models.push(model);
        }
    }
}
