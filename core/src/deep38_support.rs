//! Shared pin and evidence line for the 38 deep live cases.
//!
//! The completion proves which model ran. It is not the pass condition.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::{Agent, CodeConfig, SessionOptions};

const REQUIRED_DEFAULT_MODEL: &str = "boyue/deepseek-v4-flash";
const REQUIRED_PROVIDER: &str = "boyue";
const REQUIRED_MODEL_ID: &str = "deepseek-v4-flash";
const ALTERNATE_FLASH_MODEL: &str = "boyue/bailian/deepseek-v4.1-flash";

fn repo_config_path() -> PathBuf {
    std::env::var_os("A3S_CONFIG_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../..")
                .join(".a3s/config.acl")
        })
}

fn resolve_provider_model(full: &str) -> Option<(&str, &str)> {
    let (provider, model) = full.split_once('/')?;
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    Some((provider, model))
}

fn provider_has_model(config: &CodeConfig, provider: &str, model_id: &str) -> bool {
    config
        .find_provider(provider)
        .map(|found| found.models.iter().any(|model| model.id == model_id))
        .unwrap_or(false)
}

pub(crate) fn load_pinned_config() -> CodeConfig {
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

    let pin = std::env::var("A3S_TEST_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .and_then(|full| {
            let resolved = if full == "boyue/bailian/deepseek-v4-flash" {
                eprintln!("A3S_TEST_MODEL={full} is not declared; using {ALTERNATE_FLASH_MODEL}");
                ALTERNATE_FLASH_MODEL.to_string()
            } else {
                full
            };
            let (provider_id, model_id) = resolve_provider_model(&resolved)?;
            if provider_has_model(&config, provider_id, model_id) {
                Some(resolved)
            } else {
                eprintln!(
                    "A3S_TEST_MODEL={resolved} is not declared in {}; falling back to {REQUIRED_DEFAULT_MODEL}",
                    path.display()
                );
                None
            }
        })
        .unwrap_or_else(|| REQUIRED_DEFAULT_MODEL.to_string());

    if config.default_model.as_deref() != Some(pin.as_str()) {
        eprintln!(
            "pinning default_model to {pin} (config had {:?})",
            config.default_model
        );
    }
    config.default_model = Some(pin.clone());
    eprintln!("using default_model={pin}");
    config
}

pub(crate) fn pinned_model(config: &CodeConfig) -> String {
    let pin = config
        .default_model
        .clone()
        .expect("deep38 pin must set default_model");
    assert!(
        pin == REQUIRED_DEFAULT_MODEL || pin == ALTERNATE_FLASH_MODEL,
        "deep38 must pin {REQUIRED_DEFAULT_MODEL} or {ALTERNATE_FLASH_MODEL}; got {pin}"
    );
    pin
}

/// One real completion. The returned string is the model pin, not a verdict.
pub(crate) async fn complete() -> String {
    let config = load_pinned_config();
    let model = pinned_model(&config);
    let agent = Agent::from_config(config)
        .await
        .expect("build real-provider agent");
    let workspace = tempfile::tempdir().expect("deep38 workspace");
    let session_id = format!(
        "deep38-complete-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    );
    let options = SessionOptions::new()
        .with_session_id(session_id)
        .with_model(model.clone())
        .with_max_tool_rounds(1)
        .with_temperature(0.0);
    let session = agent
        .session_async(workspace.path().to_string_lossy().as_ref(), Some(options))
        .await
        .expect("admit deep38 completion session");
    let completed = tokio::time::timeout(
        Duration::from_secs(180),
        session.send("Reply with the single word ready.", None),
    )
    .await;
    agent.close().await;
    completed
        .expect("model completion timed out")
        .expect("model completion");
    model
}

pub(crate) fn pass(id: &str, model: &str, effect: &str) {
    println!("DEEP38\t{id}\t{model}\t{effect}\tpassed");
}

/// Local HTTP/1.1 responder used by the model-adapter and S3 cases.
pub(crate) struct StatusHttp {
    pub addr: SocketAddr,
    shutdown: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl StatusHttp {
    pub(crate) async fn spawn(
        status: u16,
        reason: &'static str,
        content_type: &'static str,
        body: &'static str,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind deep38 status http");
        let addr = listener.local_addr().expect("status http addr");
        let shutdown = CancellationToken::new();
        let cancel = shutdown.clone();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((mut socket, _)) = accepted else { break };
                        let cancel = cancel.clone();
                        tokio::spawn(async move {
                            let _ = answer(&mut socket, status, reason, content_type, body, &cancel).await;
                        });
                    }
                }
            }
        });
        Self {
            addr,
            shutdown,
            task: Some(task),
        }
    }
}

impl Drop for StatusHttp {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn answer(
    socket: &mut tokio::net::TcpStream,
    status: u16,
    reason: &str,
    content_type: &str,
    body: &str,
    cancel: &CancellationToken,
) -> std::io::Result<()> {
    let mut buf = vec![0_u8; 64 * 1024];
    let mut filled = 0usize;
    let header_end = loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        if filled >= 4 {
            if let Some(offset) = buf[..filled]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
            {
                break offset + 4;
            }
        }
        if filled == buf.len() {
            break filled;
        }
        let read =
            tokio::time::timeout(Duration::from_secs(3), socket.read(&mut buf[filled..])).await;
        match read {
            Ok(Ok(0)) | Err(_) | Ok(Err(_)) => break filled,
            Ok(Ok(count)) => filled += count,
        }
    };
    let header = String::from_utf8_lossy(&buf[..header_end]);
    if let Some(length) = content_length(&header) {
        let mut remain = length.saturating_sub(filled.saturating_sub(header_end));
        while remain > 0 && !cancel.is_cancelled() {
            let mut chunk = vec![0_u8; remain.min(16 * 1024)];
            match tokio::time::timeout(Duration::from_secs(2), socket.read(&mut chunk)).await {
                Ok(Ok(0)) | Err(_) | Ok(Err(_)) => break,
                Ok(Ok(count)) => remain = remain.saturating_sub(count),
            }
        }
    }
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nx-amz-request-id: deep38\r\nx-amz-id-2: deep38\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    let _ = socket.shutdown().await;
    Ok(())
}

fn content_length(header: &str) -> Option<usize> {
    header.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("content-length") {
            value.trim().parse().ok()
        } else {
            None
        }
    })
}
