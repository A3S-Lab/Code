//! Layer C live E2E model binding for the monorepo Flash default.
//!
//! Pins `boyue/bailian/deepseek-v4.1-flash` when that model exists under
//! `providers "boyue"`, so an external mid-suite edit of `default_model` cannot
//! silently switch the matrix onto a different Flash route.

use std::path::PathBuf;

use a3s_code_core::config::CodeConfig;

/// Goal-required Layer C default (`provider/model` form).
pub const REQUIRED_DEFAULT_MODEL: &str = "boyue/bailian/deepseek-v4.1-flash";
const REQUIRED_PROVIDER: &str = "boyue";
const REQUIRED_MODEL_ID: &str = "bailian/deepseek-v4.1-flash";

pub fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

/// Load ACL and pin the Layer C Flash model when the provider declares it.
pub fn load_pinned_layer_c_config() -> CodeConfig {
    let path = repo_config_path();
    let mut config = CodeConfig::from_file(&path)
        .unwrap_or_else(|error| panic!("failed to load {}: {error}", path.display()));
    let provider = config.find_provider(REQUIRED_PROVIDER).unwrap_or_else(|| {
        panic!(
            "{} must declare providers \"{REQUIRED_PROVIDER}\"",
            path.display()
        )
    });
    assert!(
        provider
            .models
            .iter()
            .any(|model| model.id == REQUIRED_MODEL_ID),
        "{} must declare {REQUIRED_PROVIDER} models \"{REQUIRED_MODEL_ID}\"",
        path.display()
    );
    if config.default_model.as_deref() != Some(REQUIRED_DEFAULT_MODEL) {
        eprintln!(
            "pinning default_model to {REQUIRED_DEFAULT_MODEL} (config had {:?})",
            config.default_model
        );
    }
    config.default_model = Some(REQUIRED_DEFAULT_MODEL.to_string());
    eprintln!("using default_model={REQUIRED_DEFAULT_MODEL}");
    config
}
