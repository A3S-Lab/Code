//! `fs/read_text_file` bridge: text reads see the client's unsaved buffers.

use super::A3sCodeAgent;
use a3s_code_core::workspace::{WorkspacePath, WorkspaceReadOverlay};
use agent_client_protocol::{self as acp, Client as _};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const READ_TIMEOUT: Duration = Duration::from_secs(10);

struct ReadAsk {
    path: PathBuf,
    reply: oneshot::Sender<Option<String>>,
}

struct ClientReadOverlay {
    root: PathBuf,
    asks: mpsc::Sender<ReadAsk>,
}

#[async_trait::async_trait]
impl WorkspaceReadOverlay for ClientReadOverlay {
    async fn read_text(&self, path: &WorkspacePath) -> Option<String> {
        let (reply, answer) = oneshot::channel();
        let ask = ReadAsk {
            path: self.root.join(path.as_str()),
            reply,
        };
        self.asks.send(ask).await.ok()?;
        answer.await.ok().flatten()
    }
}

impl A3sCodeAgent {
    /// Route the session's text reads through the client when it declared
    /// `fs.readTextFile`. Must run on the ACP `LocalSet`.
    pub(super) fn with_client_reads(
        &self,
        options: a3s_code_core::SessionOptions,
        session_id: &str,
        workspace: &std::path::Path,
    ) -> a3s_code_core::SessionOptions {
        if !self.client_reads_files.get() {
            return options;
        }
        let Some(client) = self.client.borrow().clone() else {
            return options;
        };
        let (asks, mut pending) = mpsc::channel::<ReadAsk>(16);
        let session_id = acp::SessionId::new(session_id.to_string());
        tokio::task::spawn_local(async move {
            while let Some(ask) = pending.recv().await {
                let request = acp::ReadTextFileRequest::new(session_id.clone(), ask.path.clone());
                let content = match tokio::time::timeout(
                    READ_TIMEOUT,
                    client.read_text_file(request),
                )
                .await
                {
                    Ok(Ok(response)) => Some(response.content),
                    Ok(Err(error)) => {
                        tracing::debug!(path = %ask.path.display(), %error, "client read refused; using the workspace backend");
                        None
                    }
                    Err(_) => {
                        tracing::warn!(path = %ask.path.display(), "client read timed out; using the workspace backend");
                        None
                    }
                };
                let _ = ask.reply.send(content);
            }
        });
        options.with_workspace_read_overlay(Arc::new(ClientReadOverlay {
            root: workspace.to_path_buf(),
            asks,
        }))
    }
}
