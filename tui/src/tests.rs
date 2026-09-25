use std::sync::{Arc, Mutex};

use a3s_code_core::llm::{LlmClient, LlmResponse, Message, StreamEvent, ToolDefinition};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::model::resolve_acl_file;
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

#[test]
fn acl_default_model_wins_over_xai_api_key() {
    let (path, acl) = fixture_acl();
    let file_model = file_default_model(&acl).to_string();
    let file_provider = file_model.split('/').next().expect("provider").to_string();
    let resolved = resolve_acl_file(&path).expect("acl model");
    println!("ACL_MODEL={file_model}");
    println!("RESOLVED_MODEL={}", resolved.model_id);
    println!("PROVIDER={}", resolved.provider);
    std::env::set_var("XAI_API_KEY", "should-not-select-a-model");
    let with_key = resolve_acl_file(&path).expect("acl model with key set");
    println!("WITH_XAI_KEY_MODEL={}", with_key.model_id);
    println!("WITH_XAI_KEY_PROVIDER={}", with_key.provider);
    std::env::remove_var("XAI_API_KEY");
    assert_eq!(resolved.model_id, file_model);
    assert_eq!(resolved.provider, file_provider);
    assert_eq!(with_key.model_id, file_model);
    assert_eq!(with_key.provider, file_provider);
}

#[tokio::test]
async fn tui_turn_records_fact_log_user_message_with_acl_model() {
    let workspace = tempfile_dir();
    let (_path, acl) = fixture_acl();
    let file_model = file_default_model(&acl).to_string();
    let prompt = "hello from the tui";
    let invocations = Arc::new(Mutex::new(Vec::new()));
    let text = submit_configured_turn(
        &workspace,
        &acl,
        Some(Arc::new(Scripted)),
        Arc::clone(&invocations),
        prompt,
    )
    .await
    .expect("tui turn");
    let seen = invocations.lock().unwrap().clone();
    println!("ACL_MODEL={file_model}");
    println!(
        "CLIENT_MODEL={}",
        seen.first().map(String::as_str).unwrap_or("")
    );
    println!("REPLY={text}");
    let facts = a3s_code_core::fact_control::read_workspace_facts(&workspace, "tui-turn")
        .expect("fact log");
    let user = facts
        .iter()
        .find(|fact| fact.kind == "user.message")
        .expect("user.message fact");
    println!("FACT_KIND={}", user.kind);
    println!("FACT_TEXT={}", user.payload["text"]);
    assert_eq!(seen.first().map(String::as_str), Some(file_model.as_str()));
    assert_eq!(user.payload["text"], prompt);
    let _ = std::fs::remove_dir_all(&workspace);
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
