//! LSP Client
//!
//! JSON-RPC client for communicating with language servers.

use crate::lsp::protocol::*;
use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot, RwLock};

/// LSP client for communicating with a language server
pub struct LspClient {
    /// Language name
    pub language: String,
    /// Child process
    child: RwLock<Option<Child>>,
    /// Stdin writer
    stdin_tx: mpsc::Sender<Vec<u8>>,
    /// Pending requests
    pending: Arc<RwLock<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Notification receiver (for future use)
    #[allow(dead_code)]
    notification_rx: RwLock<Option<mpsc::Receiver<LspNotification>>>,
    /// Request ID counter
    next_id: AtomicU64,
    /// Connected flag
    connected: AtomicBool,
    /// Server capabilities
    capabilities: RwLock<ServerCapabilities>,
    /// Diagnostics cache (uri -> diagnostics)
    diagnostics: RwLock<HashMap<String, Vec<Diagnostic>>>,
}

impl LspClient {
    /// Spawn a new LSP client
    pub async fn spawn(
        language: String,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn LSP server: {} {:?}", command, args))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("No stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("No stdout"))?;

        let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(100);
        let (notification_tx, notification_rx) = mpsc::channel::<LspNotification>(100);
        let pending: Arc<RwLock<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        // Spawn stdin writer task
        let mut stdin_writer = stdin;
        tokio::spawn(async move {
            while let Some(msg) = stdin_rx.recv().await {
                if let Err(e) = stdin_writer.write_all(&msg).await {
                    tracing::error!("Failed to write to LSP stdin: {}", e);
                    break;
                }
                if let Err(e) = stdin_writer.flush().await {
                    tracing::error!("Failed to flush LSP stdin: {}", e);
                    break;
                }
            }
        });

        // Spawn stdout reader task
        let pending_clone = pending.clone();
        let lang_clone = language.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut headers = String::new();
            let mut content_length: usize = 0;

            loop {
                headers.clear();

                // Read headers
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line).await {
                        Ok(0) => {
                            tracing::debug!("LSP {} stdout closed", lang_clone);
                            return;
                        }
                        Ok(_) => {
                            if line == "\r\n" || line == "\n" {
                                break;
                            }
                            if line.to_lowercase().starts_with("content-length:") {
                                if let Some(len_str) = line.split(':').nth(1) {
                                    content_length = len_str.trim().parse().unwrap_or(0);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to read LSP {} header: {}", lang_clone, e);
                            return;
                        }
                    }
                }

                if content_length == 0 {
                    continue;
                }

                // Read content
                let mut content = vec![0u8; content_length];
                if let Err(e) = tokio::io::AsyncReadExt::read_exact(&mut reader, &mut content).await
                {
                    tracing::error!("Failed to read LSP {} content: {}", lang_clone, e);
                    return;
                }

                let content_str = match String::from_utf8(content) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!("Invalid UTF-8 from LSP {}: {}", lang_clone, e);
                        continue;
                    }
                };

                // Try to parse as response
                if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&content_str) {
                    if let Some(id) = response.id {
                        let mut pending = pending_clone.write().await;
                        if let Some(tx) = pending.remove(&id) {
                            let _ = tx.send(response);
                        }
                    }
                    continue;
                }

                // Try to parse as notification
                if let Ok(notification) = serde_json::from_str::<JsonRpcNotification>(&content_str)
                {
                    let lsp_notif = LspNotification::from_json_rpc(&notification);
                    let _ = notification_tx.send(lsp_notif).await;
                    continue;
                }

                tracing::warn!("Unknown LSP {} message: {}", lang_clone, content_str);
            }
        });

        Ok(Self {
            language,
            child: RwLock::new(Some(child)),
            stdin_tx,
            pending,
            notification_rx: RwLock::new(Some(notification_rx)),
            next_id: AtomicU64::new(1),
            connected: AtomicBool::new(true),
            capabilities: RwLock::new(ServerCapabilities::default()),
            diagnostics: RwLock::new(HashMap::new()),
        })
    }

    /// Get next request ID
    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Send a request and wait for response
    async fn request(&self, method: &str, params: serde_json::Value) -> Result<JsonRpcResponse> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(anyhow!("LSP client not connected"));
        }

        let id = self.next_id();
        let request = JsonRpcRequest::new(id, method, Some(params));

        // Create response channel
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.write().await;
            pending.insert(id, tx);
        }

        // Serialize and send
        let content = serde_json::to_string(&request)?;
        let message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);

        self.stdin_tx
            .send(message.into_bytes())
            .await
            .map_err(|_| anyhow!("Failed to send request"))?;

        // Wait for response
        let response = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| anyhow!("Request timeout"))?
            .map_err(|_| anyhow!("Response channel closed"))?;

        Ok(response)
    }

    /// Send a notification (no response expected)
    async fn notify(&self, method: &str, params: serde_json::Value) -> Result<()> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(anyhow!("LSP client not connected"));
        }

        let notification = JsonRpcNotification::new(method, Some(params));
        let content = serde_json::to_string(&notification)?;
        let message = format!("Content-Length: {}\r\n\r\n{}", content.len(), content);

        self.stdin_tx
            .send(message.into_bytes())
            .await
            .map_err(|_| anyhow!("Failed to send notification"))?;

        Ok(())
    }

    /// Initialize the LSP connection
    pub async fn initialize(&self, root_uri: &str) -> Result<InitializeResult> {
        let params = InitializeParams {
            process_id: Some(std::process::id()),
            root_uri: Some(root_uri.to_string()),
            capabilities: ClientCapabilities::default(),
            initialization_options: None,
        };

        let response = self
            .request("initialize", serde_json::to_value(&params)?)
            .await?;

        if let Some(error) = response.error {
            return Err(anyhow!(
                "LSP initialize error: {} ({})",
                error.message,
                error.code
            ));
        }

        let result: InitializeResult =
            serde_json::from_value(response.result.ok_or_else(|| anyhow!("No result"))?)?;

        // Store capabilities
        {
            let mut caps = self.capabilities.write().await;
            *caps = result.capabilities.clone();
        }

        // Send initialized notification
        self.notify("initialized", serde_json::json!({})).await?;

        tracing::info!(
            "LSP {} initialized: {:?}",
            self.language,
            result.server_info
        );

        Ok(result)
    }

    /// Get hover information
    pub async fn hover(&self, uri: &str, line: u32, character: u32) -> Result<Option<Hover>> {
        let params = TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position: Position::new(line, character),
        };

        let response = self
            .request("textDocument/hover", serde_json::to_value(&params)?)
            .await?;

        if let Some(error) = response.error {
            return Err(anyhow!(
                "LSP hover error: {} ({})",
                error.message,
                error.code
            ));
        }

        match response.result {
            Some(serde_json::Value::Null) => Ok(None),
            Some(result) => Ok(Some(serde_json::from_value(result)?)),
            None => Ok(None),
        }
    }

    /// Go to definition
    pub async fn goto_definition(
        &self,
        uri: &str,
        line: u32,
        character: u32,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let params = TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position: Position::new(line, character),
        };

        let response = self
            .request("textDocument/definition", serde_json::to_value(&params)?)
            .await?;

        if let Some(error) = response.error {
            return Err(anyhow!(
                "LSP definition error: {} ({})",
                error.message,
                error.code
            ));
        }

        match response.result {
            Some(serde_json::Value::Null) => Ok(None),
            Some(result) => Ok(Some(serde_json::from_value(result)?)),
            None => Ok(None),
        }
    }

    /// Find references
    pub async fn find_references(
        &self,
        uri: &str,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Result<Vec<Location>> {
        let params = ReferenceParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
            position: Position::new(line, character),
            context: ReferenceContext {
                include_declaration,
            },
        };

        let response = self
            .request("textDocument/references", serde_json::to_value(&params)?)
            .await?;

        if let Some(error) = response.error {
            return Err(anyhow!(
                "LSP references error: {} ({})",
                error.message,
                error.code
            ));
        }

        match response.result {
            Some(serde_json::Value::Null) => Ok(vec![]),
            Some(result) => Ok(serde_json::from_value(result)?),
            None => Ok(vec![]),
        }
    }

    /// Get document symbols
    pub async fn document_symbols(&self, uri: &str) -> Result<Vec<DocumentSymbol>> {
        let params = serde_json::json!({
            "textDocument": { "uri": uri }
        });

        let response = self.request("textDocument/documentSymbol", params).await?;

        if let Some(error) = response.error {
            return Err(anyhow!(
                "LSP documentSymbol error: {} ({})",
                error.message,
                error.code
            ));
        }

        match response.result {
            Some(serde_json::Value::Null) => Ok(vec![]),
            Some(result) => Ok(serde_json::from_value(result)?),
            None => Ok(vec![]),
        }
    }

    /// Search workspace symbols
    pub async fn workspace_symbols(&self, query: &str) -> Result<Vec<SymbolInformation>> {
        let params = WorkspaceSymbolParams {
            query: query.to_string(),
        };

        let response = self
            .request("workspace/symbol", serde_json::to_value(&params)?)
            .await?;

        if let Some(error) = response.error {
            return Err(anyhow!(
                "LSP workspaceSymbol error: {} ({})",
                error.message,
                error.code
            ));
        }

        match response.result {
            Some(serde_json::Value::Null) => Ok(vec![]),
            Some(result) => Ok(serde_json::from_value(result)?),
            None => Ok(vec![]),
        }
    }

    /// Notify document opened
    pub async fn did_open(&self, uri: &str, language_id: &str, text: &str) -> Result<()> {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.to_string(),
                language_id: language_id.to_string(),
                version: 1,
                text: text.to_string(),
            },
        };

        self.notify("textDocument/didOpen", serde_json::to_value(&params)?)
            .await
    }

    /// Notify document closed
    pub async fn did_close(&self, uri: &str) -> Result<()> {
        let params = DidCloseTextDocumentParams {
            text_document: TextDocumentIdentifier {
                uri: uri.to_string(),
            },
        };

        self.notify("textDocument/didClose", serde_json::to_value(&params)?)
            .await
    }

    /// Get cached diagnostics for a URI
    pub async fn get_diagnostics(&self, uri: &str) -> Vec<Diagnostic> {
        let diags = self.diagnostics.read().await;
        diags.get(uri).cloned().unwrap_or_default()
    }

    /// Update diagnostics cache (called from notification handler)
    pub async fn update_diagnostics(&self, uri: String, diagnostics: Vec<Diagnostic>) {
        let mut diags = self.diagnostics.write().await;
        diags.insert(uri, diagnostics);
    }

    /// Get server capabilities
    pub async fn capabilities(&self) -> ServerCapabilities {
        self.capabilities.read().await.clone()
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    /// Close the client
    pub async fn close(&self) -> Result<()> {
        self.connected.store(false, Ordering::SeqCst);

        // Send shutdown request
        let _ = self.request("shutdown", serde_json::json!(null)).await;

        // Send exit notification
        let _ = self.notify("exit", serde_json::json!(null)).await;

        // Kill child process
        let mut child_guard = self.child.write().await;
        if let Some(mut child) = child_guard.take() {
            let _ = child.kill().await;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lsp_client_spawn_invalid() {
        let result = LspClient::spawn(
            "test".to_string(),
            "nonexistent_lsp_server_12345",
            &[],
            &HashMap::new(),
        )
        .await;
        assert!(result.is_err());
    }
}
