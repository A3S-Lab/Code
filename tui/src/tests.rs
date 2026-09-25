use std::sync::Arc;

use a3s_code_core::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::model::{merge_launch_layers, LaunchLayers};
use crate::session::submit_configured_turn;

fn fixture_acl() -> (std::path::PathBuf, String) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/model.acl");
    let text = std::fs::read_to_string(&path).expect("fixture acl");
    (path, text)
}

fn file_default_model(acl: &str) -> &str {
    acl.lines()
        .find(|line| line.trim_start().starts_with("default_model"))
        .and_then(|line| line.split('"').nth(1))
        .filter(|value| !value.is_empty())
        .expect("fixture default_model")
}

struct Scripted;

#[async_trait]
impl LlmClient for Scripted {
    async fn complete(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> anyhow::Result<LlmResponse> {
        Ok(reply("scripted-ok"))
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel_token: CancellationToken,
    ) -> anyhow::Result<mpsc::Receiver<StreamEvent>> {
        let (tx, rx) = mpsc::channel(2);
        let response = reply("scripted-ok");
        tokio::spawn(async move {
            let _ = tx.send(StreamEvent::TextDelta("scripted-ok".into())).await;
            let _ = tx.send(StreamEvent::Done(response)).await;
        });
        Ok(rx)
    }
}

fn reply(text: &str) -> LlmResponse {
    LlmResponse {
        message: Message::assistant(text),
        usage: Default::default(),
        stop_reason: None,
        token_logprobs: Vec::new(),
        meta: None,
    }
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

#[test]
fn acl_default_model_wins_over_xai_api_key() {
    let _lock = env_lock();
    let _env = EnvVar::set("A3S_DEFAULT_MODEL", None);
    let (path, acl) = fixture_acl();
    let file_model = file_default_model(&acl).to_string();
    let file_provider = file_model.split('/').next().expect("provider").to_string();
    let layers = LaunchLayers {
        workspace: tempfile_dir(),
        explicit_config: Some(path),
        home: None,
    };
    let resolved = merge_launch_layers(&layers).expect("acl model");
    println!("ACL_MODEL={file_model}");
    println!("RESOLVED_MODEL={}", resolved.model_id);
    println!("PROVIDER={}", resolved.provider);
    let _key = EnvVar::set("XAI_API_KEY", Some("should-not-select-a-model"));
    let with_key = merge_launch_layers(&layers).expect("acl model with key set");
    println!("WITH_XAI_KEY_MODEL={}", with_key.model_id);
    println!("WITH_XAI_KEY_PROVIDER={}", with_key.provider);
    assert_eq!(resolved.model_id, file_model);
    assert_eq!(resolved.provider, file_provider);
    assert_eq!(with_key.model_id, file_model);
    assert_eq!(with_key.provider, file_provider);
}

#[test]
fn tui_turn_records_fact_log_user_message_with_acl_model() {
    let _lock = env_lock();
    let workspace = tempfile_dir();
    let home = tempfile_dir();
    let user_acl = r#"
default_model = "userprov/user-model"
providers "userprov" {
  apiKey = "user-key"
  models "user-model" {
    name = "User"
  }
}
"#;
    let workspace_acl = r#"
providers "workprov" {
  apiKey = env("ANTHROPIC_API_KEY")
  baseUrl = "http://127.0.0.1:9"
  models "work-model" {
    name = "Work"
  }
}
"#;
    write_acl(&home.join(".a3s/config.acl"), user_acl);
    write_acl(&workspace.join(".a3s/config.acl"), workspace_acl);
    let merged_model = "workprov/work-model";
    let _env = EnvVar::set("A3S_DEFAULT_MODEL", Some(merged_model));
    let _api_key = EnvVar::set("ANTHROPIC_API_KEY", None);
    let expected = cli_merged_model(user_acl, workspace_acl, Some(merged_model));
    let layers = LaunchLayers {
        workspace: workspace.clone(),
        explicit_config: None,
        home: Some(home.clone()),
    };
    let prompt = "hello from the tui";
    let turn = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(submit_configured_turn(
            &layers,
            Some(Arc::new(Scripted)),
            prompt,
        ))
        .expect("tui turn");
    println!("ACL_MODEL={expected}");
    println!("SESSION_MODEL={}", turn.model_name);
    println!("CLIENT_MODEL={}", turn.model_name);
    println!("REPLY={}", turn.text);
    let facts = a3s_code_core::fact_control::read_workspace_facts(&workspace, "tui-turn")
        .expect("fact log");
    let user = facts
        .iter()
        .find(|fact| fact.kind == "user.message")
        .expect("user.message fact");
    println!("FACT_KIND={}", user.kind);
    println!("FACT_TEXT={}", user.payload["text"]);
    println!("PROVIDERS={}", turn.provider_names.join(","));
    assert_eq!(turn.model_name, expected);
    assert!(turn.provider_names.iter().any(|name| name == "userprov"));
    assert!(turn.provider_names.iter().any(|name| name == "workprov"));
    assert_eq!(user.payload["text"], prompt);
    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_dir_all(&home);
}

/// CLI precedence: workspace `default_model` overlays the user layer, then
/// `A3S_DEFAULT_MODEL` replaces both. This oracle reads the fixture text; it
/// does not call the TUI merge.
fn cli_merged_model(user_acl: &str, workspace_acl: &str, env_model: Option<&str>) -> String {
    if let Some(model) = env_model.filter(|value| !value.is_empty()) {
        return model.to_string();
    }
    file_default_model_optional(workspace_acl)
        .or_else(|| file_default_model_optional(user_acl))
        .expect("layered default_model")
        .to_string()
}

fn file_default_model_optional(acl: &str) -> Option<&str> {
    acl.lines()
        .find(|line| line.trim_start().starts_with("default_model"))
        .and_then(|line| line.split('"').nth(1))
        .filter(|value| !value.is_empty())
}

fn write_acl(path: &std::path::Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("acl directory");
    }
    std::fs::write(path, contents).expect("write acl");
}

struct EnvVar {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVar {
    fn set(key: &'static str, value: Option<&str>) -> Self {
        let previous = std::env::var_os(key);
        match value {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
        Self { key, previous }
    }
}

impl Drop for EnvVar {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

fn tempfile_dir() -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "a3s-code-tui-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).expect("workspace");
    path
}
