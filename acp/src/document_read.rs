use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a3s_code_core::capability::{WorkspaceOperation, WorkspaceOperationAccess};
use a3s_code_core::tools::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use agent_client_protocol::{AgentSideConnection, Client, ExtRequest};
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};

const TEXT_LIMIT: usize = 64 * 1024;
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

struct Call {
    access: Option<WorkspaceOperationAccess>,
    method: &'static str,
    params: Value,
    reply: oneshot::Sender<Result<Value>>,
}

pub(crate) struct DocumentBridge {
    sender: mpsc::Sender<Call>,
    receiver: Mutex<Option<mpsc::Receiver<Call>>>,
    worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
    runs: Mutex<HashMap<String, document_run::RetainedRun>>,
}

impl DocumentBridge {
    pub fn new() -> Arc<Self> {
        let (sender, receiver) = mpsc::channel(16);
        Arc::new(Self {
            sender,
            receiver: Mutex::new(Some(receiver)),
            worker: Mutex::new(None),
            runs: Mutex::new(HashMap::new()),
        })
    }

    pub fn connect(&self, client: Rc<AgentSideConnection>) {
        let receiver = self
            .receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(mut receiver) = receiver {
            let worker = tokio::task::spawn_local(async move {
                while let Some(call) = receiver.recv().await {
                    let result = async {
                        if let Some(access) = &call.access {
                            access.check()?;
                        }
                        let params = serde_json::value::to_raw_value(&call.params)?;
                        let response = tokio::time::timeout(
                            CALL_TIMEOUT,
                            client.ext_method(ExtRequest::new(call.method, Arc::from(params))),
                        )
                        .await
                        .context("document host timeout")??;
                        if let Some(access) = &call.access {
                            access.check()?;
                        }
                        anyhow::ensure!(
                            response.0.get().len() <= TEXT_LIMIT * 8 + 32768,
                            "document host response exceeds limit"
                        );
                        Ok(serde_json::from_str(response.0.get())?)
                    }
                    .await;
                    let _ = call.reply.send(result);
                }
            });
            *self
                .worker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(worker);
        }
    }

    pub fn stop(&self) {
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            worker.abort();
        }
        self.receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }

    async fn read(&self, access: WorkspaceOperationAccess, params: Value) -> Result<Value> {
        let cancellation = access.cancellation()?;
        let (reply, result) = oneshot::channel();
        let work = async {
            self.sender
                .send(Call {
                    access: Some(access.clone()),
                    method: "a3s/document/read",
                    params,
                    reply,
                })
                .await
                .context("document bridge closed")?;
            let value = result.await.context("document host unavailable")??;
            access.check()?;
            Ok(value)
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => anyhow::bail!("document read cancelled"),
            result = tokio::time::timeout(CALL_TIMEOUT, work) => result.context("document read timeout")?,
        }
    }

    pub async fn ask(
        &self,
        access: WorkspaceOperationAccess,
        method: &'static str,
        params: Value,
    ) -> Result<Value> {
        let cancellation = access.cancellation()?;
        let (reply, result) = oneshot::channel();
        let work = async {
            self.sender
                .send(Call {
                    access: Some(access.clone()),
                    method,
                    params,
                    reply,
                })
                .await
                .context("document bridge closed")?;
            let value = result.await.context("document host unavailable")??;
            access.check()?;
            Ok(value)
        };
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => anyhow::bail!("encyclopedia search cancelled"),
            result = tokio::time::timeout(CALL_TIMEOUT, work) => result.context("host request timeout")?,
        }
    }

    async fn release_run(&self, session: &str, run: &str) -> Result<()> {
        let (reply, result) = oneshot::channel();
        tokio::time::timeout(CALL_TIMEOUT, async {
            self.sender
                .send(Call {
                    access: None,
                    method: "a3s/document/release_run",
                    params: json!({"sessionId":session,"runId":run}),
                    reply,
                })
                .await
                .context("document bridge closed")?;
            let value = result.await.context("document host unavailable")??;
            anyhow::ensure!(
                value["released"] == true && value["runId"] == run,
                "document host did not acknowledge Run release"
            );
            Ok(())
        })
        .await
        .context("document Run release timeout")?
    }

    pub fn tool(self: &Arc<Self>, workspace: &Path, session: &str) -> Result<Arc<dyn Tool>> {
        Ok(Arc::new(DocumentReadTool {
            bridge: self.clone(),
            workspace: crate::plugin_workspace::WorkspaceBinding::new(workspace)?,
            session: session.to_owned(),
        }))
    }
}

struct DocumentReadTool {
    bridge: Arc<DocumentBridge>,
    workspace: crate::plugin_workspace::WorkspaceBinding,
    session: String,
}

impl Drop for DocumentBridge {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    file_path: String,
    #[serde(default)]
    retain_for_run: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    path: String,
    snapshot: Snapshot,
    released: bool,
    #[serde(default, rename = "runId")]
    run_id: Option<String>,
    #[serde(default)]
    diagnostics: Vec<document_diagnostics::Publication>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshot {
    document_id: String,
    uri: String,
    version: u64,
    text: String,
    dirty: bool,
    disk_conflict: bool,
}

#[async_trait]
impl Tool for DocumentReadTool {
    fn name(&self) -> &str {
        "studio_document_read"
    }
    fn description(&self) -> &str {
        "Read Studio's shared document text, version and advisory diagnostics from scoped publishers. An empty published set does not verify a document. Set retain_for_run to keep its model identity until Run cleanup."
    }
    fn parameters(&self) -> Value {
        json!({"type":"object","properties":{"file_path":{"type":"string"},"retain_for_run":{"type":"boolean","description":"Keep this model pinned until the current Run releases its resources."}},"required":["file_path"],"additionalProperties":false})
    }
    fn capabilities(&self, _: &Value) -> ToolCapabilities {
        // Fencing a cancelled RPC does not certify settlement of native blocking I/O.
        ToolCapabilities {
            read_only: true,
            idempotent: true,
            ..ToolCapabilities::conservative()
        }
    }
    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let result: Result<ToolOutput> = async {
            self.workspace.validate(&ctx.workspace)?;
            let access = ctx.workspace_operation(WorkspaceOperation::Read)?;
            let input: Input = serde_json::from_value(args.clone())?;
            anyhow::ensure!(!input.file_path.is_empty() && input.file_path.len() <= 4096 && !input.file_path.contains('\0'), "invalid document path");
            let retain_for_run = input.retain_for_run;
            let input = PathBuf::from(input.file_path);
            anyhow::ensure!(!input.components().any(|part| part == Component::ParentDir), "document not found");
            let path = if input.is_absolute() { input } else { self.workspace.root().join(input) };
            anyhow::ensure!(path.starts_with(self.workspace.root()), "document not found");
            let path = path.to_str().context("invalid document path")?;
            // A replaced root must stop this Turn before another model observes its result.
            ctx.register_capability_effect(self.workspace.clone())?;
            let run = if retain_for_run { Some(self.bridge.retain_run(ctx, &self.session)?) } else { None };
            let mut params = json!({"sessionId":self.session,"path":path});
            if let Some(run) = &run { params["runId"] = json!(run); }
            let value = self.bridge.read(access.clone(), params).await?;
            self.workspace.validate(&ctx.workspace)?;
            let response: Response = serde_json::from_value(value)?;
            anyhow::ensure!(response.released == run.is_none() && response.run_id == run && response.path == path, "document host did not acknowledge release or binding");
            let snapshot = response.snapshot;
            let canonical = std::fs::canonicalize(path).context("document not found")?;
            anyhow::ensure!(canonical.starts_with(self.workspace.root()), "document not found");
            let uri = url::Url::from_file_path(canonical).map_err(|()| anyhow::anyhow!("invalid document URI"))?;
            anyhow::ensure!(snapshot.uri == uri.as_str(), "document URI binding mismatch");
            anyhow::ensure!(!snapshot.document_id.is_empty() && snapshot.document_id.len() <= 128 && snapshot.uri.starts_with("file:") && snapshot.uri.len() <= 16384 && snapshot.version > 0 && snapshot.version <= 9_007_199_254_740_991 && snapshot.text.len() <= TEXT_LIMIT && !snapshot.text.contains('\0'), "invalid document observation");
            document_diagnostics::validate(&response.diagnostics, &snapshot)?;
            access.check()?;
            Ok(ToolOutput::success(json!({"documentId":snapshot.document_id,"uri":snapshot.uri,"version":snapshot.version,"text":snapshot.text,"dirty":snapshot.dirty,"diskConflict":snapshot.disk_conflict,"retainedForRun":retain_for_run,"diagnostics":response.diagnostics,"diagnosticsComplete":false}).to_string()))
        }.await;
        match result {
            Ok(output) => Ok(output),
            Err(error) => Ok(ToolOutput::error(
                error.to_string().chars().take(1024).collect::<String>(),
            )),
        }
    }
}

#[cfg(test)]
#[path = "document_read_tests.rs"]
mod tests;

#[path = "document_run.rs"]
mod document_run;

#[path = "document_diagnostics.rs"]
mod document_diagnostics;

#[cfg(test)]
#[path = "document_run_tests.rs"]
mod run_tests;
