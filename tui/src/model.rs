//! ACL model selection for the TUI.
//!
//! The product configuration is the A3S ACL file. An `XAI_API_KEY` in the
//! environment is not consulted.

use std::path::Path;

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
