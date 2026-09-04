#[cfg(unix)]
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::sync::{mpsc, Semaphore};

use super::protocol::{
    CallToolParams, CallToolResult, InitializeResult, JsonRpcNotification, JsonRpcRequest,
    JsonRpcResponse, ListToolsResult, McpNotification, McpTool, McpToolAnnotations,
    ServerCapabilities, ServerInfo, ToolContent, PROTOCOL_VERSION,
};
use super::transport::McpTransport;
use super::{McpBinding, McpClient};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecordedMcpCall {
    pub(crate) name: String,
    pub(crate) arguments: Option<serde_json::Value>,
}

/// Deterministic exact-client transport shared by MCP unit and Run tests.
pub(crate) struct RecordingMcpTransport {
    version: Box<str>,
    tools: Vec<McpTool>,
    connected: AtomicBool,
    block_calls: AtomicBool,
    calls: Mutex<Vec<RecordedMcpCall>>,
    close_count: AtomicUsize,
    pub(crate) call_entered: Semaphore,
    pub(crate) release_call: Semaphore,
}

impl RecordingMcpTransport {
    pub(crate) fn new(version: impl Into<String>, tools: Vec<McpTool>) -> Arc<Self> {
        Arc::new(Self {
            version: version.into().into_boxed_str(),
            tools,
            connected: AtomicBool::new(true),
            block_calls: AtomicBool::new(false),
            calls: Mutex::new(Vec::new()),
            close_count: AtomicUsize::new(0),
            call_entered: Semaphore::new(0),
            release_call: Semaphore::new(0),
        })
    }

    pub(crate) fn block_calls(&self) {
        self.block_calls.store(true, Ordering::Release);
    }

    pub(crate) fn disconnect(&self) {
        self.connected.store(false, Ordering::Release);
    }

    pub(crate) fn calls(&self) -> Vec<RecordedMcpCall> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn close_count(&self) -> usize {
        self.close_count.load(Ordering::Acquire)
    }

    fn response(id: u64, result: serde_json::Value) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }
}

#[async_trait]
impl McpTransport for RecordingMcpTransport {
    async fn request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        if !self.is_connected() {
            return Err(anyhow!("recording MCP transport is disconnected"));
        }

        match request.method.as_str() {
            "initialize" => Ok(Self::response(
                request.id,
                serde_json::to_value(InitializeResult {
                    protocol_version: PROTOCOL_VERSION.to_string(),
                    capabilities: ServerCapabilities::default(),
                    server_info: ServerInfo {
                        name: "recording-mcp".to_string(),
                        version: self.version.to_string(),
                    },
                })?,
            )),
            "tools/list" => Ok(Self::response(
                request.id,
                serde_json::to_value(ListToolsResult {
                    tools: self.tools.clone(),
                })?,
            )),
            "tools/call" => {
                let parameters: CallToolParams = serde_json::from_value(
                    request
                        .params
                        .ok_or_else(|| anyhow!("tools/call parameters are missing"))?,
                )?;
                self.calls
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(RecordedMcpCall {
                        name: parameters.name,
                        arguments: parameters.arguments,
                    });
                self.call_entered.add_permits(1);
                if self.block_calls.load(Ordering::Acquire) {
                    self.release_call.acquire().await?.forget();
                }
                if !self.is_connected() {
                    return Err(anyhow!("recording MCP transport closed during tool call"));
                }
                Ok(Self::response(
                    request.id,
                    serde_json::to_value(CallToolResult {
                        content: vec![ToolContent::Text {
                            text: self.version.to_string(),
                        }],
                        ..CallToolResult::default()
                    })?,
                ))
            }
            method => Err(anyhow!("unsupported recording MCP method '{method}'")),
        }
    }

    async fn notify(&self, _notification: JsonRpcNotification) -> Result<()> {
        if self.is_connected() {
            Ok(())
        } else {
            Err(anyhow!("recording MCP transport is disconnected"))
        }
    }

    fn notifications(&self) -> mpsc::Receiver<McpNotification> {
        let (_sender, receiver) = mpsc::channel(1);
        receiver
    }

    async fn close(&self) -> Result<()> {
        if self.connected.swap(false, Ordering::AcqRel) {
            self.close_count.fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }
}

pub(crate) fn mcp_tool(name: &str, description: &str) -> McpTool {
    McpTool {
        name: name.to_string(),
        title: None,
        description: Some(description.to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "generation": { "type": "string" }
            },
            "additionalProperties": false
        }),
        output_schema: None,
        annotations: Some(McpToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            open_world_hint: Some(false),
            ..McpToolAnnotations::default()
        }),
        icons: Vec::new(),
        meta: None,
    }
}

pub(crate) async fn ready_binding(
    server_name: &str,
    version: &str,
    tools: Vec<McpTool>,
) -> (Arc<McpBinding>, Arc<RecordingMcpTransport>, Arc<McpClient>) {
    let transport = RecordingMcpTransport::new(version, tools);
    let client = Arc::new(McpClient::new(
        server_name.to_string(),
        Arc::clone(&transport) as Arc<dyn McpTransport>,
    ));
    client.initialize().await.unwrap();
    let tools = client.list_tools().await.unwrap();
    let binding = Arc::new(McpBinding::new(server_name, Arc::clone(&client), tools).unwrap());
    (binding, transport, client)
}

#[cfg(unix)]
pub(crate) fn compile_fake_server(output: &Path) {
    static BINARY: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

    let binary = BINARY.get_or_init(|| {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp_fake_server.rs");
        let build_dir = tempfile::tempdir().expect("fake MCP server build directory");
        let binary = build_dir.path().join(if cfg!(windows) {
            "mcp-fake-server.exe"
        } else {
            "mcp-fake-server"
        });
        let mut command = std::process::Command::new("rustc");
        command
            .arg("--edition=2021")
            .arg(source)
            .arg("-o")
            .arg(&binary);
        let result = crate::tools::process::output_std_with_native_gate(&mut command)
            .expect("rustc must be available while Cargo tests are running");
        assert!(
            result.status.success(),
            "failed to compile fake MCP server: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        std::fs::read(binary).expect("read compiled fake MCP server")
    });

    std::fs::write(output, binary).expect("write fake MCP server fixture");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o755))
            .expect("make fake MCP server executable");
    }
}

#[cfg(unix)]
pub(crate) fn fixture_started_pids(log: &str) -> Vec<u32> {
    log.lines()
        .filter(|line| line.contains("\"event\":\"process_started\""))
        .filter_map(|line| {
            line.split_once("\"pid\":")?
                .1
                .trim_end_matches('}')
                .parse()
                .ok()
        })
        .collect()
}

#[cfg(unix)]
pub(crate) fn process_exists(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}
