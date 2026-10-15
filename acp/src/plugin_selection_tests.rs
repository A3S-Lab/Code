#![cfg(test)]
#![cfg(unix)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use a3s_code_core::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use a3s_code_core::{Agent, BuiltinHostHarnessRegistry, HarnessComposeOptions, SessionOptions};
use a3s_use_reconcile::{files_sha256, PackageFile, PackageSpec, Payload, SurfaceSpec};
use tokio::sync::{mpsc, Semaphore};

use super::*;

const ID: &str = "studio-installed-plugins";

fn spec(text: &str) -> PackageSpec {
    script_spec(&format!("cat >/dev/null\nprintf '{text}'"))
}

fn script_spec(script: &str) -> PackageSpec {
    let files = vec![PackageFile {
        path: "echo".into(),
        bytes: format!("#!/bin/sh\n{script}\n").into_bytes(),
    }];
    PackageSpec {
        package_id: "acme/echo".into(),
        surfaces: vec![SurfaceSpec {
            kind: SurfaceKind::Tool,
            id: "echo".into(),
            sha256: files_sha256(&files).unwrap(),
            entry: Some("echo".into()),
            payload: Payload::Files(files),
        }],
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_native_harness_run_stops_descendants_before_releasing_retired_bytes() {
    let root = tempfile::tempdir().unwrap();
    let started = root.path().join("started");
    let leaked = root.path().join("leaked");
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let publication = store
        .apply_sync(&script_spec(&format!(
            "cat >/dev/null\n(sleep 1; printf leaked > '{}') &\nprintf started > '{}'\nwait",
            leaked.display(),
            started.display(),
        )))
        .unwrap();
    store
        .publish_selection_sync(ID, None, &[publication.cursor()])
        .unwrap();
    let entry = publication.surfaces[0].entry.clone().unwrap();
    let model = Arc::new(Model::new());
    model.release.add_permits(1);
    let session = session(root.path(), Arc::clone(&model)).await;
    PluginSelectionHost::current(store.clone(), ID, root.path())
        .await
        .unwrap()
        .apply(&session)
        .await
        .unwrap();
    let task = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("Call echo once.", None).await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !started.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("native executable was not reached by the actual Harness Run");
    store.withdraw_sync("acme/echo").unwrap();
    store.collect_retired_sync("acme/echo").unwrap();
    assert!(
        entry.exists(),
        "active native Tool lost its retained package bytes"
    );
    assert!(session.cancel().await);
    let _result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    store.collect_retired_sync("acme/echo").unwrap();
    assert!(
        !entry.exists(),
        "settled cancelled Run retained withdrawn package bytes"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        !leaked.exists(),
        "cancelled native Tool descendant still mutated the workspace"
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    session.close().await;
}

fn installed() -> (
    tempfile::TempDir,
    ReconcileStore,
    a3s_use_reconcile::SelectionSnapshot,
) {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let publication = store.apply_sync(&spec("old")).unwrap();
    let snapshot = store
        .publish_selection_sync(ID, None, &[publication.cursor()])
        .unwrap();
    (root, store, snapshot)
}

struct DelegatingNativeModel {
    parent_calls: AtomicUsize,
    child_calls: AtomicUsize,
    pause_first: std::sync::atomic::AtomicBool,
    admitted: Semaphore,
    release: Semaphore,
    results: Mutex<Vec<String>>,
}

impl DelegatingNativeModel {
    fn new(pause: bool) -> Self {
        Self {
            parent_calls: AtomicUsize::new(0),
            child_calls: AtomicUsize::new(0),
            pause_first: std::sync::atomic::AtomicBool::new(pause),
            admitted: Semaphore::new(0),
            release: Semaphore::new(0),
            results: Mutex::new(Vec::new()),
        }
    }
    fn reset_calls(&self) {
        self.parent_calls.store(0, Ordering::SeqCst);
        self.child_calls.store(0, Ordering::SeqCst);
    }
    fn latest_result(messages: &[Message]) -> Result<String> {
        messages
            .iter()
            .rev()
            .flat_map(|message| message.content.iter().rev())
            .find_map(|block| {
                if let ContentBlock::ToolResult { content, .. } = block {
                    Some(content.as_text())
                } else {
                    None
                }
            })
            .context("delegated native result missing")
    }
}

#[async_trait]
impl LlmClient for DelegatingNativeModel {
    async fn complete(
        &self,
        messages: &[Message],
        _: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if tools.iter().any(|tool| tool.name == "task") {
            if self.parent_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(Model::response(
                    vec![ContentBlock::ToolUse {
                        id: "delegate-native".into(),
                        name: "task".into(),
                        input: serde_json::json!({"tasks": [{"agent": "general", "description": "Call managed plugin", "prompt": "Call echo once and return delegated-complete."}]}),
                    }],
                    "tool_use",
                ));
            }
            let result = Self::latest_result(messages)?;
            anyhow::ensure!(
                result.contains("delegated-complete"),
                "Task did not return a native plugin result: {result}"
            );
            return Ok(Model::response(
                vec![ContentBlock::Text {
                    text: "parent complete".into(),
                }],
                "end_turn",
            ));
        }
        if tools.iter().any(|tool| tool.name == "echo") {
            let call = self.child_calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                if self.pause_first.swap(false, Ordering::SeqCst) {
                    self.admitted.add_permits(1);
                    self.release.acquire().await?.forget();
                }
                return Ok(Model::response(
                    vec![ContentBlock::ToolUse {
                        id: "child-native".into(),
                        name: "echo".into(),
                        input: serde_json::json!({}),
                    }],
                    "tool_use",
                ));
            }
            if call == 1 {
                self.results
                    .lock()
                    .unwrap()
                    .push(Self::latest_result(messages)?);
            }
            return Ok(Model::response(
                vec![ContentBlock::Text {
                    text: "delegated-complete".into(),
                }],
                "end_turn",
            ));
        }
        Ok(Model::response(
            vec![ContentBlock::Text {
                text: "auxiliary completion".into(),
            }],
            "end_turn",
        ))
    }
    async fn complete_streaming(
        &self,
        _: &[Message],
        _: Option<&str>,
        _: &[ToolDefinition],
        _: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("this contract uses non-streaming model responses")
    }
}

async fn delegated_session(
    root: &std::path::Path,
    model: Arc<DelegatingNativeModel>,
) -> Arc<AgentSession> {
    let agent = Agent::from_config(crate::agent::test_launch(Some(root.join("sessions"))).config)
        .await
        .unwrap();
    Arc::new(
        agent
            .session_async(
                root.to_string_lossy().into_owned(),
                Some(
                    SessionOptions::new()
                        .with_session_id(uuid::Uuid::new_v4().to_string())
                        .with_llm_client(model)
                        .with_planning_mode(a3s_code_core::PlanningMode::Disabled)
                        .with_permission_policy(
                            a3s_code_core::permissions::PermissionPolicy::new().allow("*"),
                        )
                        .with_harness(
                            HarnessComposeOptions::compose(
                                ["system", "tools", "budget", "compact", "infer"]
                                    .into_iter()
                                    .map(str::to_owned)
                                    .collect(),
                                None,
                                None,
                                Vec::new(),
                            )
                            .unwrap(),
                        ),
                ),
            )
            .await
            .unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_plugin_tool_is_inherited_by_actual_task_and_stays_pinned_across_cutover() {
    let (root, store, snapshot) = installed();
    // Native package publication is host state, outside the editable workspace.
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let model = Arc::new(DelegatingNativeModel::new(true));
    let session = delegated_session(&workspace, model.clone()).await;
    PluginSelectionHost::current(store.clone(), ID, &workspace)
        .await
        .unwrap()
        .apply(&session)
        .await
        .unwrap();
    let old_entry = store.current_sync("acme/echo").unwrap().unwrap().surfaces[0]
        .entry
        .clone()
        .unwrap();
    let mut old_run = tokio::spawn({
        let session = session.clone();
        async move { session.send("Delegate the old plugin.", None).await }
    });
    tokio::select! {
        result = &mut old_run => panic!("Run ended before native child admission: {result:?}"),
        admitted = tokio::time::timeout(Duration::from_secs(5), model.admitted.acquire()) => admitted.unwrap().unwrap().forget(),
    }
    let next = store.apply_sync(&spec("new")).unwrap();
    store
        .publish_selection_sync(ID, Some(&snapshot.cursor()), &[next.cursor()])
        .unwrap();
    PluginSelectionHost::current(store.clone(), ID, &workspace)
        .await
        .unwrap()
        .apply(&session)
        .await
        .unwrap();
    store.collect_retired_sync("acme/echo").unwrap();
    assert!(
        old_entry.exists(),
        "admitted child lost its accepted executable"
    );
    model.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), old_run)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(&*model.results.lock().unwrap(), &["old"]);
    store.collect_retired_sync("acme/echo").unwrap();
    assert!(
        !old_entry.exists(),
        "completed child retained retired executable bytes"
    );
    model.reset_calls();
    tokio::time::timeout(
        Duration::from_secs(10),
        session.send("Delegate the new plugin.", None),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(&*model.results.lock().unwrap(), &["old", "new"]);
    session.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_actual_task_native_tool_settles_descendants_before_package_retirement() {
    let root = tempfile::tempdir().unwrap();
    let started = root.path().join("child-started");
    let leaked = root.path().join("child-leaked");
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let publication = store
        .apply_sync(&script_spec(&format!(
            "cat >/dev/null\n(sleep 1; printf leaked > '{}') &\nprintf started > '{}'\nwait",
            leaked.display(),
            started.display(),
        )))
        .unwrap();
    store
        .publish_selection_sync(ID, None, &[publication.cursor()])
        .unwrap();
    let entry = publication.surfaces[0].entry.clone().unwrap();
    let model = Arc::new(DelegatingNativeModel::new(false));
    let session = delegated_session(root.path(), model).await;
    PluginSelectionHost::current(store.clone(), ID, root.path())
        .await
        .unwrap()
        .apply(&session)
        .await
        .unwrap();
    let mut run = tokio::spawn({
        let session = session.clone();
        async move { session.send("Delegate the cancellable plugin.", None).await }
    });
    tokio::select! {
        result = &mut run => panic!("Run ended before native child execution: {result:?}"),
        started = tokio::time::timeout(Duration::from_secs(5), async {
            while !started.exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
        }) => started.unwrap(),
    }
    store.withdraw_sync("acme/echo").unwrap();
    store.collect_retired_sync("acme/echo").unwrap();
    assert!(
        entry.exists(),
        "executing child lost its accepted package bytes"
    );
    assert!(session.cancel().await);
    let _result = tokio::time::timeout(Duration::from_secs(3), run)
        .await
        .unwrap()
        .unwrap();
    store.collect_retired_sync("acme/echo").unwrap();
    assert!(
        !entry.exists(),
        "settled delegated cancellation retained retired bytes"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        !leaked.exists(),
        "cancelled delegated plugin descendant still wrote into the workspace"
    );
    session.close().await;
}

struct Model {
    calls: AtomicUsize,
    admitted: Semaphore,
    release: Semaphore,
    results: Mutex<Vec<String>>,
}

impl Model {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            admitted: Semaphore::new(0),
            release: Semaphore::new(0),
            results: Mutex::new(Vec::new()),
        }
    }

    fn response(content: Vec<ContentBlock>, stop: &str) -> LlmResponse {
        LlmResponse {
            message: Message {
                role: "assistant".into(),
                content,
                reasoning_content: None,
                transcript_text: None,
                transcript_visibility: Default::default(),
            },
            usage: TokenUsage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
                cache_read_tokens: None,
                cache_write_tokens: None,
            },
            stop_reason: Some(stop.into()),
            token_logprobs: Vec::new(),
            meta: None,
        }
    }
}

#[async_trait]
impl LlmClient for Model {
    async fn complete(
        &self,
        messages: &[Message],
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if !tools.iter().any(|tool| tool.name == "echo") {
            return Ok(Self::response(
                vec![ContentBlock::Text {
                    text: "auxiliary completion".into(),
                }],
                "end_turn",
            ));
        }
        let call = self.calls.load(Ordering::SeqCst);
        if call.is_multiple_of(2) {
            anyhow::ensure!(
                tools.iter().any(|t| t.name == "echo"),
                "native Tool definition missing"
            );
            let stamped = system.unwrap_or("").contains("intent_stamp")
                || messages.iter().flat_map(|message| &message.content).any(|block| matches!(block, ContentBlock::Text { text } if text.contains("intent_stamp")));
            anyhow::ensure!(
                stamped,
                "host Harness component was not observed on inference call {call}"
            );
            anyhow::ensure!(
                system.unwrap_or("").matches("recipe_marker").count() == 1,
                "Harness recipe system content was duplicated or lost"
            );
            self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                self.admitted.add_permits(1);
                self.release.acquire().await?.forget();
            }
            Ok(Self::response(
                vec![ContentBlock::ToolUse {
                    id: format!("native-{call}"),
                    name: "echo".into(),
                    input: serde_json::json!({}),
                }],
                "tool_use",
            ))
        } else {
            let content = messages
                .iter()
                .rev()
                .flat_map(|m| m.content.iter().rev())
                .find_map(|b| match b {
                    ContentBlock::ToolResult { content, .. } => Some(content.as_text()),
                    _ => None,
                })
                .context("actual Tool result missing")?;
            self.results.lock().unwrap().push(content);
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(Self::response(
                vec![ContentBlock::Text {
                    text: "done".into(),
                }],
                "end_turn",
            ))
        }
    }

    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("this contract uses non-streaming model responses")
    }
}

async fn session(root: &std::path::Path, client: Arc<Model>) -> Arc<AgentSession> {
    session_with_services(root, client, None).await
}

async fn session_with_services(
    root: &std::path::Path,
    client: Arc<Model>,
    services: Option<Arc<a3s_code_core::workspace::WorkspaceServices>>,
) -> Arc<AgentSession> {
    let config = crate::agent::test_launch(Some(root.join("sessions"))).config;
    let agent = Agent::from_config(config).await.unwrap();
    let components = [
        "system",
        "tools",
        "host:intent_stamp",
        "budget",
        "compact",
        "infer",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let mut options = SessionOptions::new()
        .with_session_id(uuid::Uuid::new_v4().to_string())
        .with_llm_client(client)
        .with_planning_mode(a3s_code_core::PlanningMode::Disabled)
        .with_permission_policy(a3s_code_core::permissions::PermissionPolicy::new().allow("echo"))
        .with_harness(
            HarnessComposeOptions::compose(components, None, None, vec!["recipe_marker".into()])
                .unwrap(),
        )
        .with_host_harness_registry(Arc::new(BuiltinHostHarnessRegistry));
    if let Some(services) = services {
        options = options.with_workspace_backend(services);
    }
    Arc::new(
        agent
            .session_async(root.to_string_lossy().into_owned(), Some(options))
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn native_harness_tool_cannot_spawn_when_backend_denies_execute() {
    use a3s_code_core::workspace::{
        WorkspaceCapabilities, WorkspaceServices, WorkspaceServicesBuilder,
    };
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let publication = store
        .apply_sync(&script_spec(
            "cat >/dev/null; touch started; printf forbidden",
        ))
        .unwrap();
    store
        .publish_selection_sync(ID, None, &[publication.cursor()])
        .unwrap();
    let marker = publication.surfaces[0]
        .entry
        .as_ref()
        .unwrap()
        .parent()
        .unwrap()
        .join("started");
    let base = WorkspaceServices::local(&workspace);
    let services = WorkspaceServicesBuilder::new(base.workspace_ref().clone(), base.fs())
        .capabilities(WorkspaceCapabilities::read_write())
        .build();
    let model = Arc::new(Model::new());
    model.release.add_permits(1);
    let session = session_with_services(&workspace, model.clone(), Some(services)).await;
    PluginSelectionHost::current(store, ID, &workspace)
        .await
        .unwrap()
        .apply(&session)
        .await
        .unwrap();
    let result = session.send("Call echo once.", None).await.unwrap();
    assert_eq!(result.text, "done");
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    assert!(
        !marker.exists(),
        "denied native process reached its entry point"
    );
    let results = model.results.lock().unwrap().clone();
    assert_eq!(results.len(), 1);
    assert!(
        results[0].contains("Tool not found in active workspace capability scope"),
        "{}",
        results[0]
    );
    session.close().await;
}

#[tokio::test]
async fn native_tool_refuses_unscoped_correct_workspace_before_process_launch() {
    let root = tempfile::tempdir().unwrap();
    let entry = root.path().join("entry");
    std::fs::write(&entry, "#!/bin/sh\ntouch started\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755)).unwrap();
    let tool = crate::plugin_tools::from_publication(
        "echo",
        entry,
        crate::plugin_workspace::WorkspaceBinding::new(root.path()).unwrap(),
    )
    .unwrap();
    let ctx = a3s_code_core::tools::ToolContext::new(root.path().to_path_buf());
    tool.validate_execution_available(&serde_json::json!({}), &ctx)
        .unwrap();
    let output = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
    assert!(!output.success);
    assert!(output
        .content
        .contains("Tool not found in active workspace capability scope"));
    assert!(!root.path().join("started").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_harness_run_retains_native_tool_across_cutover_and_rejects_withdrawal() {
    let (root, store, snapshot) = installed();
    let model = Arc::new(Model::new());
    let session = session(root.path(), Arc::clone(&model)).await;
    PluginSelectionHost::current(store.clone(), ID, root.path())
        .await
        .unwrap()
        .apply(&session)
        .await
        .unwrap();
    let old_entry = store.current_sync("acme/echo").unwrap().unwrap().surfaces[0]
        .entry
        .clone()
        .unwrap();
    let mut task = tokio::spawn({
        let session = Arc::clone(&session);
        async move { session.send("Call echo once.", None).await }
    });
    tokio::select! {
        result = &mut task => panic!("Run ended before native model admission: {result:?}"),
        admitted = tokio::time::timeout(Duration::from_secs(5), model.admitted.acquire()) => admitted.unwrap().unwrap().forget(),
    }
    let next = store.apply_sync(&spec("new")).unwrap();
    store
        .publish_selection_sync(ID, Some(&snapshot.cursor()), &[next.cursor()])
        .unwrap();
    PluginSelectionHost::current(store.clone(), ID, root.path())
        .await
        .unwrap()
        .apply(&session)
        .await
        .unwrap();
    store.collect_retired_sync("acme/echo").unwrap();
    assert!(
        old_entry.exists(),
        "live Harness Run did not retain its real native package"
    );
    model.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    store.collect_retired_sync("acme/echo").unwrap();
    assert!(
        !old_entry.exists(),
        "settled Harness Run retained retired bytes"
    );
    session.send("Call echo again.", None).await.unwrap();
    assert_eq!(*model.results.lock().unwrap(), ["old", "new"]);
    let current = store.selection_sync(ID).unwrap().unwrap();
    store.withdraw_selection_sync(&current.cursor()).unwrap();
    assert!(session
        .send("Call echo after withdrawal.", None)
        .await
        .is_err());
    assert_eq!(
        model.calls.load(Ordering::SeqCst),
        4,
        "withdrawn admission reached inference"
    );
    session.close().await;
}

#[tokio::test]
async fn native_acp_binding_rejects_cursor_forgery_and_unbound_executable_metadata() {
    let (root, store, _snapshot) = installed();
    let host = PluginSelectionHost::current(store, ID, root.path())
        .await
        .unwrap();
    let cursor = host.cursor().clone();
    let mut launch = crate::agent::test_launch(Some(root.path().join("sessions")));
    launch.workspace = root.path().to_owned();
    let agent = crate::A3sCodeAgent::new(launch).with_plugin_selection(host);
    let mut request = serde_json::json!({"cwd": root.path(), "mcpServers": [], "_meta": {"a3sPluginSelection": cursor}});
    request["_meta"]["a3sPluginSelection"]["generation"] = 99.into();
    assert!(agent_client_protocol::Agent::new_session(
        &agent,
        serde_json::from_value(request.clone()).unwrap()
    )
    .await
    .is_err());
    request["_meta"]["a3sPluginSelection"] = serde_json::json!(cursor);
    request["_meta"]["a3sTools"] = serde_json::json!([{"id":"forged", "entry":"/bin/echo"}]);
    assert!(agent_client_protocol::Agent::new_session(
        &agent,
        serde_json::from_value(request.clone()).unwrap()
    )
    .await
    .is_err());
    request["_meta"].as_object_mut().unwrap().remove("a3sTools");
    let opened =
        agent_client_protocol::Agent::new_session(&agent, serde_json::from_value(request).unwrap())
            .await
            .unwrap();
    assert!(!opened.session_id.0.is_empty());
}

#[tokio::test]
async fn stale_content_or_cancelled_acquisition_cannot_issue_a_real_run_lease() {
    let (root, store, _snapshot) = installed();
    let host = PluginSelectionHost::current(store.clone(), ID, root.path())
        .await
        .unwrap();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(host.acquire(cancelled).await.is_err());
    let entry = store.current_sync("acme/echo").unwrap().unwrap().surfaces[0]
        .entry
        .clone()
        .unwrap();
    std::fs::write(entry, b"tampered").unwrap();
    assert!(host.acquire(CancellationToken::new()).await.is_err());
    assert!(host.batch(CodeCatalogGeneration::new(1)).await.is_err());
}

#[tokio::test]
async fn native_selection_cannot_be_applied_to_another_workspace_before_any_plugin_effect() {
    let (root, store, _snapshot) = installed();
    let foreign = tempfile::tempdir().unwrap();
    let host = PluginSelectionHost::current(store, ID, root.path())
        .await
        .unwrap();
    let model = Arc::new(Model::new());
    let session = session(foreign.path(), model.clone()).await;
    let before = session.capability_catalog_stamp();
    assert!(
        host.apply(&session).await.is_err(),
        "native selection entered an unbound workspace"
    );
    let batch = host.batch(CodeCatalogGeneration::new(1)).await.unwrap();
    assert!(
        session
            .apply_capability_batch(batch, CancellationToken::new())
            .await
            .is_err(),
        "Core bypassed the native workspace binding through the raw batch API"
    );
    assert_eq!(session.capability_catalog_stamp(), before);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    session.close().await;
    drop(root);
}

#[tokio::test]
async fn native_acp_refuses_foreign_new_and_loaded_workspaces_before_filesystem_effects() {
    let (root, store, _snapshot) = installed();
    let foreign = tempfile::tempdir().unwrap();
    let host = PluginSelectionHost::current(store, ID, root.path())
        .await
        .unwrap();
    let cursor = host.cursor().clone();
    let mut launch = crate::agent::test_launch(Some(root.path().join("sessions")));
    launch.workspace = root.path().to_owned();
    let agent = crate::A3sCodeAgent::new(launch).with_plugin_selection(host);
    let request = serde_json::json!({"cwd":foreign.path(), "mcpServers":[], "_meta":{"a3sPluginSelection":cursor}});
    let new_error = agent_client_protocol::Agent::new_session(
        &agent,
        serde_json::from_value(request.clone()).unwrap(),
    )
    .await
    .unwrap_err();
    assert!(
        new_error
            .to_string()
            .contains("native plugin workspace is not admitted"),
        "{new_error}"
    );
    let mut load = request;
    load["sessionId"] = "foreign-session".into();
    let load_error =
        agent_client_protocol::Agent::load_session(&agent, serde_json::from_value(load).unwrap())
            .await
            .unwrap_err();
    assert_eq!(
        serde_json::to_value(load_error).unwrap(),
        serde_json::to_value(agent_client_protocol::Error::resource_not_found(None)).unwrap(),
        "foreign recovery must preserve the denied/missing not-found contract"
    );
    assert!(!foreign.path().join(".a3s").exists());
    assert!(!root.path().join("sessions").exists());
}

#[tokio::test]
async fn retargeted_workspace_cannot_prepare_or_admit_native_capabilities() {
    let (root, store, _snapshot) = installed();
    let first = root.path().join("first");
    let second = root.path().join("second");
    let alias = root.path().join("current");
    std::fs::create_dir(&first).unwrap();
    std::fs::create_dir(&second).unwrap();
    std::os::unix::fs::symlink(&first, &alias).unwrap();
    let host = PluginSelectionHost::current(store, ID, &alias)
        .await
        .unwrap();
    let model = Arc::new(Model::new());
    let session = session(&first, model.clone()).await;
    host.apply(&session).await.unwrap();
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&second, &alias).unwrap();
    assert!(host.batch(CodeCatalogGeneration::new(2)).await.is_err());
    assert!(host.acquire(CancellationToken::new()).await.is_err());
    assert!(session
        .send("Use the retargeted selection.", None)
        .await
        .is_err());
    assert_eq!(
        model.calls.load(Ordering::SeqCst),
        0,
        "retargeted workspace reached actual inference"
    );
    session.close().await;
}

#[tokio::test]
async fn replacing_a_directory_at_the_same_path_cannot_rebind_native_run_authority() {
    let (root, store, _snapshot) = installed();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let host = PluginSelectionHost::current(store, ID, &workspace)
        .await
        .unwrap();
    let model = Arc::new(Model::new());
    let session = session(&workspace, model.clone()).await;
    host.apply(&session).await.unwrap();
    std::fs::rename(&workspace, root.path().join("retired-workspace")).unwrap();
    std::fs::create_dir(&workspace).unwrap();
    assert!(session
        .send("Use the replaced workspace.", None)
        .await
        .is_err());
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert!(host.apply(&session).await.is_err());
    session.close().await;
}

#[tokio::test]
async fn native_tool_refuses_a_foreign_invocation_workspace_before_process_launch() {
    let (root, store, _snapshot) = installed();
    let foreign = tempfile::tempdir().unwrap();
    let marker = root.path().join("started");
    let publication = store
        .apply_sync(&script_spec(&format!(
            "printf started > '{}'",
            marker.display()
        )))
        .unwrap();
    let tool = crate::plugin_tools::from_publication(
        "echo",
        publication.surfaces[0].entry.clone().unwrap(),
        crate::plugin_workspace::WorkspaceBinding::new(root.path()).unwrap(),
    )
    .unwrap();
    let result = tool
        .execute(
            &serde_json::json!({}),
            &a3s_code_core::tools::ToolContext::new(foreign.path().to_owned()),
        )
        .await
        .unwrap();
    assert!(!result.success);
    assert!(result
        .content
        .contains("native plugin workspace is not admitted"));
    assert!(!marker.exists());
}

#[tokio::test]
async fn selected_skill_link_does_not_collide_with_its_projection_on_session_admission() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let files = vec![PackageFile { path: "SKILL.md".into(), bytes: b"---\nname: review\ndescription: Review documents\n---\nRead the document before reviewing.".to_vec() }];
    let publication = store
        .apply_sync(&PackageSpec {
            package_id: "a3s/review".into(),
            surfaces: vec![SurfaceSpec {
                kind: SurfaceKind::Skill,
                id: "review".into(),
                sha256: files_sha256(&files).unwrap(),
                entry: None,
                payload: Payload::Files(files),
            }],
        })
        .unwrap();
    store
        .publish_selection_sync(ID, None, &[publication.cursor()])
        .unwrap();
    let local = root.path().join("skills");
    std::fs::create_dir(&local).unwrap();
    std::os::unix::fs::symlink(&publication.surfaces[0].directory, local.join("review")).unwrap();
    let host = PluginSelectionHost::current(store, ID, root.path())
        .await
        .unwrap();
    let agent =
        Agent::from_config(crate::agent::test_launch(Some(root.path().join("sessions"))).config)
            .await
            .unwrap();
    let options = host.session_options(
        SessionOptions::new()
            .with_skill_dirs([local])
            .with_llm_client(Arc::new(Model::new())),
    );
    let session = agent
        .session_async(root.path().to_string_lossy().to_string(), Some(options))
        .await
        .unwrap();
    host.apply(&session)
        .await
        .expect("same managed skill must have one runtime owner");
    session.close().await;
}
