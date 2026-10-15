#![cfg(test)]

use super::*;
use a3s_code_core::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use a3s_code_core::{Agent, AgentSession, HarnessComposeOptions, SessionOptions};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Semaphore;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;

struct State {
    calls: Mutex<Vec<(String, Value)>>,
    context: Mutex<Option<ToolContext>>,
    roots: Mutex<Vec<String>>,
    closing: Semaphore,
    release: Semaphore,
    ack: bool,
    probe: Mutex<Option<Probe>>,
}
struct Host {
    state: Arc<State>,
    root: PathBuf,
}
#[async_trait(?Send)]
impl Client for Host {
    async fn request_permission(
        &self,
        _: agent_client_protocol::RequestPermissionRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::RequestPermissionResponse> {
        Err(agent_client_protocol::Error::internal_error())
    }
    async fn session_notification(
        &self,
        _: agent_client_protocol::SessionNotification,
    ) -> agent_client_protocol::Result<()> {
        Ok(())
    }
    async fn ext_method(
        &self,
        request: ExtRequest,
    ) -> agent_client_protocol::Result<agent_client_protocol::ExtResponse> {
        let params: Value = serde_json::from_str(request.params.get()).unwrap();
        assert_eq!(params["sessionId"], "document-session");
        let method = request.method.to_string();
        self.state
            .calls
            .lock()
            .unwrap()
            .push((method.clone(), params.clone()));
        let value = if method == "a3s/document/release_run" {
            self.state.closing.add_permits(1);
            self.state.release.acquire().await.unwrap().forget();
            let mut probe = self.state.probe.lock().unwrap();
            match probe.as_mut() {
                Some(probe) => {
                    probe.call(json!({"harness":{"operation":"release_run","params":params}}))
                }
                None => json!({"runId":params["runId"],"released":self.state.ack}),
            }
        } else {
            assert_eq!(method, "a3s/document/read");
            let mut probe = self.state.probe.lock().unwrap();
            match probe.as_mut() {
                Some(probe) => {
                    let value = probe.call(json!({"harness":{"operation":"read","params":params}}));
                    if let Some(lease) = probe.editor.take() {
                        probe.call(json!({"request":{"operation":"close","lease":lease}}));
                    }
                    value
                }
                None => {
                    json!({"path":self.root.join("note.txt"),"snapshot":{"documentId":"native-model","uri":url::Url::from_file_path(self.root.join("note.txt")).unwrap(),"version":1,"text":"clean text","dirty":false,"diskConflict":false},"runId":params["runId"],"released":false})
                }
            }
        };
        Ok(agent_client_protocol::ExtResponse::new(Arc::from(
            serde_json::value::to_raw_value(&value).unwrap(),
        )))
    }
}
struct Observe {
    tool: Arc<dyn Tool>,
    state: Arc<State>,
}
#[async_trait]
impl Tool for Observe {
    fn name(&self) -> &str {
        self.tool.name()
    }
    fn description(&self) -> &str {
        self.tool.description()
    }
    fn parameters(&self) -> Value {
        self.tool.parameters()
    }
    fn capabilities(&self, args: &Value) -> ToolCapabilities {
        self.tool.capabilities(args)
    }
    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        self.state
            .roots
            .lock()
            .unwrap()
            .push(ctx.capability_run_instance_id()?.to_string());
        *self.state.context.lock().unwrap() = Some(ctx.clone());
        self.tool.execute(args, ctx).await
    }
}
struct Model {
    calls: AtomicUsize,
    state: Arc<State>,
    results: Mutex<Vec<String>>,
    injected: bool,
}
#[async_trait]
impl LlmClient for Model {
    async fn complete(
        &self,
        messages: &[Message],
        _: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if !tools.iter().any(|tool| tool.name == "studio_document_read") {
            return Ok(response(vec![ContentBlock::Text {
                text: "auxiliary".into(),
            }]));
        }
        let turn = self.calls.fetch_add(1, Ordering::SeqCst);
        if turn > 0 {
            let content = messages
                .iter()
                .rev()
                .flat_map(|message| message.content.iter().rev())
                .find_map(|block| match block {
                    ContentBlock::ToolResult { content, .. } => Some(content.as_text()),
                    _ => None,
                })
                .context("document Tool result missing")?;
            self.results.lock().unwrap().push(content);
            if let Some(context) = self.state.context.lock().unwrap().clone() {
                anyhow::ensure!(
                    context.capability_run_instance_id().is_err(),
                    "retired Turn can mint a Run instance observation"
                );
                anyhow::ensure!(
                    context
                        .workspace_operation(WorkspaceOperation::Read)
                        .is_err(),
                    "closed Turn still grants reads"
                );
            } else {
                anyhow::ensure!(
                    self.state.calls.lock().unwrap().is_empty(),
                    "document executor ran without a captured Turn context"
                );
            }
            anyhow::ensure!(
                self.state.closing.available_permits() == 0,
                "Run resource retired at the Turn boundary"
            );
        }
        if turn < 2 {
            let mut input = json!({"file_path":"note.txt","retain_for_run":true});
            if self.injected {
                input["runId"] = json!("forged");
            }
            Ok(response(vec![ContentBlock::ToolUse {
                id: format!("read-{turn}"),
                name: "studio_document_read".into(),
                input,
            }]))
        } else {
            Ok(response(vec![ContentBlock::Text {
                text: "done".into(),
            }]))
        }
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("non-streaming fixture")
    }
}
fn response(content: Vec<ContentBlock>) -> LlmResponse {
    LlmResponse {
        message: Message {
            content,
            ..Message::assistant("")
        },
        usage: TokenUsage::default(),
        stop_reason: None,
        token_logprobs: vec![],
        meta: None,
    }
}
struct Fixture {
    session: Arc<AgentSession>,
    model: Arc<Model>,
    bridge: Arc<DocumentBridge>,
    state: Arc<State>,
    io: Vec<tokio::task::JoinHandle<()>>,
}
impl Fixture {
    async fn new(
        root: &Path,
        ack: bool,
        ready: bool,
        read: bool,
        injected: bool,
        probe: Option<Probe>,
    ) -> Self {
        let state = Arc::new(State {
            calls: Mutex::new(vec![]),
            context: Mutex::new(None),
            roots: Mutex::new(vec![]),
            closing: Semaphore::new(0),
            release: Semaphore::new(usize::from(ready)),
            ack,
            probe: Mutex::new(probe),
        });
        let bridge = DocumentBridge::new();
        let (left, right) = tokio::io::duplex(1024 * 1024);
        let (ar, aw) = tokio::io::split(left);
        let (cr, cw) = tokio::io::split(right);
        let (connection, agent_io) = agent_client_protocol::AgentSideConnection::new(
            crate::A3sCodeAgent::new(crate::agent::test_launch(None)),
            aw.compat_write(),
            ar.compat(),
            |future| {
                tokio::task::spawn_local(future);
            },
        );
        let (_client, client_io) = agent_client_protocol::ClientSideConnection::new(
            Rc::new(Host {
                state: state.clone(),
                root: root.to_owned(),
            }),
            cw.compat_write(),
            cr.compat(),
            |future| {
                tokio::task::spawn_local(future);
            },
        );
        bridge.connect(Rc::new(connection));
        let io = vec![
            tokio::task::spawn_local(async move {
                let _ = agent_io.await;
            }),
            tokio::task::spawn_local(async move {
                let _ = client_io.await;
            }),
        ];
        let model = Arc::new(Model {
            calls: AtomicUsize::new(0),
            state: state.clone(),
            results: Mutex::new(vec![]),
            injected,
        });
        let agent =
            Agent::from_config(crate::agent::test_launch(Some(root.join("sessions"))).config)
                .await
                .unwrap();
        let local = a3s_code_core::workspace::WorkspaceServices::local(root);
        let services = a3s_code_core::workspace::WorkspaceServicesBuilder::new(
            local.workspace_ref().clone(),
            local.fs(),
        )
        .capabilities(a3s_code_core::workspace::WorkspaceCapabilities {
            read,
            write: false,
            exec: false,
            search: false,
            git: false,
            code_intelligence: false,
        })
        .build();
        let options = SessionOptions::new()
            .with_session_id("document-session")
            .with_llm_client(model.clone())
            .with_workspace_backend(services)
            .with_planning_mode(a3s_code_core::PlanningMode::Disabled)
            .with_permission_policy(
                a3s_code_core::permissions::PermissionPolicy::new().allow("studio_document_read"),
            )
            .with_harness(
                HarnessComposeOptions::compose(
                    ["system", "tools", "budget", "compact", "infer"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                    None,
                    None,
                    vec![],
                )
                .unwrap(),
            );
        let session = Arc::new(
            agent
                .session_async(root.to_string_lossy().into_owned(), Some(options))
                .await
                .unwrap(),
        );
        session
            .register_dynamic_tool(Arc::new(Observe {
                tool: bridge.tool(root, "document-session").unwrap(),
                state: state.clone(),
            }))
            .unwrap();
        Self {
            session,
            model,
            bridge,
            state,
            io,
        }
    }
    async fn stop(self) {
        self.session.close().await;
        self.bridge.stop();
        for task in self.io {
            task.abort();
        }
    }
}
fn workspace() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(directory.path()).unwrap();
    std::fs::write(root.join("note.txt"), "clean text").unwrap();
    (directory, root)
}

#[tokio::test]
async fn native_release_acknowledgement_gates_completion_and_keeps_one_actual_run_across_turns() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_directory, root) = workspace();
            let f = Fixture::new(&root, true, false, true, false, None).await;
            let send = {
                let session = f.session.clone();
                tokio::spawn(async move { session.send("Read twice", None).await })
            };
            tokio::time::timeout(Duration::from_secs(3), f.state.closing.acquire())
                .await
                .unwrap()
                .unwrap()
                .forget();
            assert!(!send.is_finished());
            assert!(!f.session.runs().await[0].status.is_terminal());
            {
                let calls = f.state.calls.lock().unwrap();
                assert_eq!(calls.len(), 3);
                assert_eq!(calls[0].1["runId"], calls[1].1["runId"]);
                assert_eq!(calls[1].1["runId"], calls[2].1["runId"]);
                assert_eq!(calls[2].0, "a3s/document/release_run");
            }
            {
                let roots = f.state.roots.lock().unwrap();
                assert_eq!(roots.len(), 2);
                assert_eq!(roots[0], roots[1]);
            }
            f.state.release.add_permits(1);
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(3), send)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
                    .text,
                "done"
            );
            assert!(f.bridge.runs.lock().unwrap().is_empty());
            assert!(f.model.results.lock().unwrap().iter().all(
                |text| serde_json::from_str::<Value>(text).unwrap()["retainedForRun"] == true
            ));
            f.stop().await;
        })
        .await;
}

#[tokio::test]
async fn refused_native_release_cannot_complete_a_pinned_document_run() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_directory, root) = workspace();
            let f = Fixture::new(&root, false, true, true, false, None).await;
            let error = f.session.send("Read twice", None).await.unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Capability Run close was incomplete"),
                "{error}"
            );
            assert_eq!(
                f.session.runs().await[0].status,
                a3s_code_core::run::RunStatus::Failed
            );
            assert_eq!(f.bridge.runs.lock().unwrap().len(), 1);
            f.stop().await;
        })
        .await;
}

#[tokio::test]
async fn cancellation_during_native_release_waits_for_cleanup_and_refuses_prior_success() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_directory, root) = workspace();
            let f = Fixture::new(&root, true, false, true, false, None).await;
            let send = {
                let session = f.session.clone();
                tokio::spawn(async move { session.send("Read twice", None).await })
            };
            tokio::time::timeout(Duration::from_secs(3), f.state.closing.acquire())
                .await
                .unwrap()
                .unwrap()
                .forget();
            assert!(f.session.cancel().await);
            f.state.release.add_permits(1);
            assert!(tokio::time::timeout(Duration::from_secs(3), send)
                .await
                .unwrap()
                .unwrap()
                .is_err());
            assert!(f.bridge.runs.lock().unwrap().is_empty());
            f.stop().await;
        })
        .await;
}

#[tokio::test]
async fn denied_read_and_forged_owner_input_cannot_pin_a_native_resource() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for (read, injected) in [(false, false), (true, true)] {
                let (_directory, root) = workspace();
                let f = Fixture::new(&root, true, true, read, injected, None).await;
                f.session.send("Read twice", None).await.unwrap();
                assert!(f.state.calls.lock().unwrap().is_empty());
                assert!(f.bridge.runs.lock().unwrap().is_empty());
                assert!(f
                    .model
                    .results
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|text| serde_json::from_str::<Value>(text).is_err()));
                f.stop().await;
            }
        })
        .await;
}

#[tokio::test]
async fn plain_context_cannot_promote_a_document_pin_to_a_fabricated_run() {
    let (_directory, root) = workspace();
    let bridge = DocumentBridge::new();
    let ctx = ToolContext::new(root);
    assert!(ctx.capability_run_instance_id().is_err());
    assert!(bridge.retain_run(&ctx, "document-session").is_err());
    assert!(bridge.runs.lock().unwrap().is_empty());
}

struct Probe {
    child: std::process::Child,
    input: std::process::ChildStdin,
    output: std::io::BufReader<std::process::ChildStdout>,
    id: u64,
    editor: Option<String>,
}
impl Probe {
    fn new(binary: &str, root: &Path) -> (Self, Value) {
        let mut child = std::process::Command::new(binary)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut probe = Self {
            input: child.stdin.take().unwrap(),
            output: std::io::BufReader::new(child.stdout.take().unwrap()),
            child,
            id: 0,
            editor: None,
        };
        probe
            .call(json!({"harness":{"operation":"bind","root":root,"session":"document-session"}}));
        let lease = probe
            .call(json!({"request":{"operation":"open","root":root,"path":root.join("note.txt")}}));
        probe.editor = Some(lease["lease"].as_str().unwrap().to_owned());
        (probe, lease["snapshot"]["documentId"].clone())
    }
    fn call(&mut self, mut command: Value) -> Value {
        use std::io::{BufRead, Write};
        self.id += 1;
        command["id"] = json!(self.id);
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["status"], 200, "{value}");
        assert_eq!(value["id"], self.id);
        value["value"].clone()
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
#[ignore = "requires separately built Studio workspace_document_stdio probe"]
async fn actual_harness_keeps_clean_native_model_across_turns_and_releases_it_before_success() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_directory, root) = workspace();
            let binary = std::env::var("A3S_STUDIO_DOCUMENT_PROBE").unwrap();
            let (probe, identity) = Probe::new(&binary, &root);
            let f = Fixture::new(&root, true, true, true, false, Some(probe)).await;
            f.session.send("Read twice", None).await.unwrap();
            {
                let values = f.model.results.lock().unwrap();
                assert_eq!(values.len(), 2);
                for value in values.iter() {
                    assert_eq!(
                        serde_json::from_str::<Value>(value).unwrap()["documentId"],
                        identity
                    );
                }
            }
            {
                let mut probe = f.state.probe.lock().unwrap();
                let next = probe.as_mut().unwrap().call(json!({
                    "harness": {"operation":"read", "params": {
                        "sessionId":"document-session", "path":root.join("note.txt")
                    }}
                }));
                assert_ne!(next["snapshot"]["documentId"], identity);
            }
            assert!(f.bridge.runs.lock().unwrap().is_empty());
            f.stop().await;
        })
        .await;
}

#[tokio::test]
async fn fresh_actual_runs_do_not_reuse_a_previous_pin_even_when_scope_addresses_repeat() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_directory, root) = workspace();
            let f = Fixture::new(&root, true, true, true, false, None).await;
            f.session.send("Read twice", None).await.unwrap();
            f.state.closing.try_acquire().unwrap().forget();
            f.state.release.add_permits(1);
            f.model.calls.store(0, Ordering::SeqCst);
            f.session.send("Read twice again", None).await.unwrap();
            {
                let roots = f.state.roots.lock().unwrap();
                assert_eq!(roots.len(), 4);
                assert_eq!(roots[0], roots[1]);
                assert_eq!(roots[2], roots[3]);
                assert_ne!(roots[0], roots[2]);
            }
            {
                let calls = f.state.calls.lock().unwrap();
                assert_eq!(calls.len(), 6);
                assert_ne!(calls[0].1["runId"], calls[3].1["runId"]);
            }
            assert!(f.bridge.runs.lock().unwrap().is_empty());
            f.stop().await;
        })
        .await;
}

#[tokio::test]
async fn a_native_release_that_never_acknowledges_refuses_success_at_the_run_deadline() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (_directory, root) = workspace();
            let f = Fixture::new(&root, true, false, true, false, None).await;
            let error =
                tokio::time::timeout(Duration::from_secs(8), f.session.send("Read twice", None))
                    .await
                    .unwrap()
                    .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("Capability Run close was incomplete"),
                "{error}"
            );
            assert_eq!(
                f.session.runs().await[0].status,
                a3s_code_core::run::RunStatus::Failed
            );
            assert_eq!(f.bridge.runs.lock().unwrap().len(), 1);
            f.stop().await;
        })
        .await;
}
