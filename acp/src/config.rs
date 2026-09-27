//! Resolve ACL layers the same way `a3s code` / the thin TUI scaffold does.
//!
//! Also merges:
//! - the current Claude provider from a local CC Switch database
//! - models from a logged-in Grok CLI account (`~/.grok`)

use std::path::{Path, PathBuf};

use a3s_code_core::CodeConfig;
use anyhow::{anyhow, Context, Result};

use crate::cc_switch;
use crate::grok_account;

/// Merged launch identity for the ACP agent process.
#[derive(Debug, Clone)]
pub struct LaunchConfig {
    pub workspace: PathBuf,
    pub model_id: String,
    pub config: CodeConfig,
}

/// Load ACL from `--config`, else `~/.a3s/config.acl` + workspace `.a3s/config.acl`.
///
/// When no ACL is found, CC Switch and/or a logged-in Grok account alone can launch.
pub fn load_launch_config(
    workspace: PathBuf,
    explicit_config: Option<PathBuf>,
) -> Result<LaunchConfig> {
    let mut config = match layer_sources(&workspace, explicit_config.as_deref()) {
        Ok(sources) => CodeConfig::from_acl(&sources.join("\n"))
            .map_err(|error| anyhow!("failed to parse merged A3S ACL: {error}"))?,
        Err(error) => {
            let mut empty = CodeConfig::default();
            let mut imported = false;
            match cc_switch::merge_into_config(&mut empty) {
                Ok(true) => imported = true,
                Ok(false) => {}
                Err(merge_error) => {
                    tracing::warn!(error = %merge_error, "CC Switch bootstrap skipped");
                }
            }
            match grok_account::merge_into_config(&mut empty) {
                Ok(true) => imported = true,
                Ok(false) => {}
                Err(merge_error) => {
                    tracing::warn!(error = %merge_error, "Grok account bootstrap skipped");
                }
            }
            if imported {
                empty
            } else {
                return Err(error);
            }
        }
    };

    if let Err(error) = cc_switch::merge_into_config(&mut config) {
        tracing::warn!(error = %error, "CC Switch import skipped");
    }
    if let Err(error) = grok_account::merge_into_config(&mut config) {
        tracing::warn!(error = %error, "Grok account import skipped");
    }
    apply_default_model_env(&mut config)?;

    let model_id = config
        .default_model
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            anyhow!(
                "default_model must be set in provider/model format \
                 (configure .a3s/config.acl, CC Switch, or log in with Grok CLI)"
            )
        })?;
    if !model_id.contains('/') {
        return Err(anyhow!(
            "default_model must use the provider/model format (got {model_id})"
        ));
    }

    let catalog_count = config.list_models().len();
    tracing::info!(
        providers = config.providers.len(),
        models = catalog_count,
        default_model = %model_id,
        "ACP launch model catalog ready"
    );

    Ok(LaunchConfig {
        workspace,
        model_id,
        config,
    })
}

fn layer_sources(workspace: &Path, explicit: Option<&Path>) -> Result<Vec<String>> {
    if let Some(path) = explicit {
        return Ok(vec![read_layer(path)?]);
    }
    let mut sources = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let user = home
        .as_ref()
        .map(|h| h.join(".a3s/config.acl"))
        .filter(|path| path.is_file());
    if let Some(path) = user.as_deref() {
        sources.push(read_layer(path)?);
    }
    if let Some(path) = workspace_config_path(workspace) {
        let skip = user
            .as_deref()
            .is_some_and(|user_path| same_file(user_path, &path));
        if !skip {
            sources.push(read_layer(&path)?);
        }
    }
    if sources.is_empty() {
        return Err(anyhow!(
            "A3S ACL configuration was not found; pass --config or create ~/.a3s/config.acl"
        ));
    }
    Ok(sources)
}

fn read_layer(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("could not read A3S ACL {}", path.display()))
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

fn apply_default_model_env(config: &mut CodeConfig) -> Result<()> {
    let Some(value) = std::env::var_os("A3S_DEFAULT_MODEL") else {
        return Ok(());
    };
    if value.is_empty() {
        return Ok(());
    }
    let model = value
        .into_string()
        .map_err(|_| anyhow!("A3S_DEFAULT_MODEL must be valid UTF-8"))?;
    let Some((provider, id)) = model.split_once('/') else {
        return Err(anyhow!(
            "A3S_DEFAULT_MODEL must use the provider/model format"
        ));
    };
    // Model ids may themselves contain `/` (e.g. boyue/bailian/deepseek-v4.1-flash).
    if provider.is_empty() || id.is_empty() {
        return Err(anyhow!(
            "A3S_DEFAULT_MODEL must use a non-empty provider/model identifier"
        ));
    }
    config.default_model = Some(model);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_launch_config_reads_explicit_acl() {
        let dir = std::env::temp_dir().join(format!("a3s-acp-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.acl");
        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(
            file,
            r#"
providers "mock" {{
  type = "openai_compatible"
  base_url = "http://127.0.0.1:9"
  api_key = "test"
  models = ["mock/gpt"]
}}
default_model = "mock/gpt"
"#
        )
        .unwrap();

        // Avoid picking up real local imports during the unit test.
        std::env::set_var("A3S_DISABLE_CC_SWITCH", "1");
        std::env::set_var("A3S_DISABLE_GROK_ACCOUNT", "1");
        let launch = load_launch_config(dir.clone(), Some(config_path)).unwrap();
        std::env::remove_var("A3S_DISABLE_CC_SWITCH");
        std::env::remove_var("A3S_DISABLE_GROK_ACCOUNT");
        assert_eq!(launch.workspace, dir);
        assert_eq!(launch.model_id, "mock/gpt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_launch_config_keeps_slash_in_model_id() {
        let dir = std::env::temp_dir().join(format!("a3s-acp-slash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("config.acl");
        let mut file = std::fs::File::create(&config_path).unwrap();
        writeln!(
            file,
            r#"
providers "boyue" {{
  api_key = "test"
  base_url = "http://127.0.0.1:9"
  models "bailian/deepseek-v4.1-flash" {{
    name = "DeepSeek V4.1 Flash"
  }}
}}
default_model = "boyue/bailian/deepseek-v4.1-flash"
"#
        )
        .unwrap();

        std::env::set_var("A3S_DISABLE_CC_SWITCH", "1");
        std::env::set_var("A3S_DISABLE_GROK_ACCOUNT", "1");
        let launch = load_launch_config(dir.clone(), Some(config_path)).unwrap();
        std::env::remove_var("A3S_DISABLE_CC_SWITCH");
        std::env::remove_var("A3S_DISABLE_GROK_ACCOUNT");
        assert_eq!(launch.model_id, "boyue/bailian/deepseek-v4.1-flash");
        assert_eq!(launch.config.list_models().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
