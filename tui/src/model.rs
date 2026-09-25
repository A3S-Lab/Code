//! ACL model selection for the TUI.
//!
//! The product configuration is the A3S ACL file. An `XAI_API_KEY` in the
//! environment is not consulted.

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
    let mut config = CodeConfig::from_acl(&sources.join("\n"))
        .map_err(|error| format!("failed to parse merged A3S ACL: {error}"))?;
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

fn layer_sources(layers: &LaunchLayers) -> Result<Vec<String>, String> {
    if let Some(path) = layers.explicit_config.as_deref() {
        return Ok(vec![read_layer(path)?]);
    }
    let mut sources = Vec::new();
    let user = layers
        .home
        .as_deref()
        .map(|home| home.join(".a3s/config.acl"))
        .filter(|path| path.is_file());
    if let Some(path) = user.as_deref() {
        sources.push(read_layer(path)?);
    }
    if let Some(path) = workspace_config_path(&layers.workspace) {
        if user
            .as_deref()
            .is_none_or(|user_path| !same_file(user_path, &path))
        {
            sources.push(read_layer(&path)?);
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

fn read_layer(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("could not read A3S ACL {}: {error}", path.display()))
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
    if provider.is_empty() || id.is_empty() || id.contains('/') {
        return Err(
            "A3S_DEFAULT_MODEL must use one non-empty provider/model identifier".to_string(),
        );
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
