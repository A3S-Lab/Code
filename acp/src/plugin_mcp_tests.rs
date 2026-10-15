#![cfg(test)]
#![cfg(unix)]

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use a3s_code_core::llm::{
    ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
};
use a3s_code_core::{Agent, AgentSession, PlanningMode, SessionOptions};
use a3s_use_reconcile::{
    files_sha256, PackageFile, PackageSpec, Payload, ReconcileStore, SurfaceKind, SurfaceSpec,
};
use anyhow::{Context, Result};
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::PluginSelectionHost;

const ID: &str = "studio-installed-plugins";

#[tokio::test]
async fn external_stdio_mcp_does_not_block_the_rest_of_the_selection() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let server = vec![PackageFile {
        path: "server.json".into(),
        bytes: br#"{"type":"stdio","command":"npx","args":["-y","@example/server"]}"#.to_vec(),
    }];
    let skill = vec![PackageFile {
        path: "SKILL.md".into(),
        bytes: b"---\nname: notes\ndescription: keep notes\n---\n# notes\n".to_vec(),
    }];
    let publication = store
        .apply_sync(&PackageSpec {
            package_id: "acme/external".into(),
            surfaces: vec![
                SurfaceSpec {
                    kind: SurfaceKind::Skill,
                    id: "notes".into(),
                    sha256: files_sha256(&skill).unwrap(),
                    entry: None,
                    payload: Payload::Files(skill),
                },
                SurfaceSpec {
                    kind: SurfaceKind::Mcp,
                    id: "github".into(),
                    sha256: files_sha256(&server).unwrap(),
                    entry: None,
                    payload: Payload::Files(server),
                },
            ],
        })
        .unwrap();
    assert!(publication.surfaces.iter().any(|surface| {
        surface.kind == SurfaceKind::Mcp && surface.id == "github" && surface.entry.is_none()
    }));
    store
        .publish_selection_sync(ID, None, &[publication.cursor()])
        .unwrap();
    let host = PluginSelectionHost::current(store, ID, root.path())
        .await
        .unwrap();
    assert!(host.contains_mcp_server("github"));
    let _batch = host
        .batch(a3s_code_core::capability::CodeCatalogGeneration::new(1))
        .await
        .expect("an unadmitted external MCP must not fail the session");
}

#[tokio::test]
async fn missing_skill_file_does_not_block_a_sibling_package() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let nested = skill_files(
        "skills/review/SKILL.md",
        b"---\nname: review\ndescription: review notes\n---\n# review\n",
    );
    let notes = skill_files(
        "SKILL.md",
        b"---\nname: notes\ndescription: keep notes\n---\n# notes\n",
    );
    let nested = store
        .apply_sync(&skill_package("acme/review", "review", nested))
        .unwrap();
    let notes = store
        .apply_sync(&skill_package("acme/notes", "notes", notes))
        .unwrap();
    store
        .publish_selection_sync(ID, None, &[nested.cursor(), notes.cursor()])
        .unwrap();
    let host = PluginSelectionHost::current(store, ID, root.path())
        .await
        .expect("a skill file outside the surface root must not refuse the selection");
    let session = session(root.path(), Arc::new(Model::new())).await;
    host.apply(&session)
        .await
        .expect("a missing skill file must not close the session");
    let names = session.projected_capability_names();
    assert!(names.iter().any(|name| name == "notes"), "{names:?}");
    assert!(!names.iter().any(|name| name == "review"), "{names:?}");
    session
        .send("Use the remaining skill.", None)
        .await
        .unwrap();
    session.close().await;
}

#[tokio::test]
async fn unreachable_http_mcp_does_not_block_a_sibling_package() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let notes = skill_files(
        "SKILL.md",
        b"---\nname: notes\ndescription: keep notes\n---\n# notes\n",
    );
    let remote = vec![PackageFile {
        path: "server.json".into(),
        bytes: br#"{"type":"http","url":"http://127.0.0.1:1/mcp"}"#.to_vec(),
    }];
    let notes = store
        .apply_sync(&skill_package("acme/notes", "notes", notes))
        .unwrap();
    let remote = store
        .apply_sync(&PackageSpec {
            package_id: "acme/remote".into(),
            surfaces: vec![SurfaceSpec {
                kind: SurfaceKind::Mcp,
                id: "remote".into(),
                sha256: files_sha256(&remote).unwrap(),
                entry: None,
                payload: Payload::Files(remote),
            }],
        })
        .unwrap();
    store
        .publish_selection_sync(ID, None, &[notes.cursor(), remote.cursor()])
        .unwrap();
    let session = session(root.path(), Arc::new(Model::new())).await;
    PluginSelectionHost::current(store, ID, root.path())
        .await
        .unwrap()
        .apply(&session)
        .await
        .expect("an unreachable MCP must not close the session");
    let names = session.projected_capability_names();
    assert!(names.iter().any(|name| name == "notes"), "{names:?}");
    assert!(!names.iter().any(|name| name == "remote"), "{names:?}");
    assert_eq!(session.capability_catalog_stamp().generation().get(), 1);
    session
        .send("Use the remaining skill.", None)
        .await
        .unwrap();
    session.close().await;
}

fn skill_files(path: &str, bytes: &[u8]) -> Vec<PackageFile> {
    vec![PackageFile {
        path: path.into(),
        bytes: bytes.to_vec(),
    }]
}

fn skill_package(package_id: &str, id: &str, files: Vec<PackageFile>) -> PackageSpec {
    PackageSpec {
        package_id: package_id.into(),
        surfaces: vec![SurfaceSpec {
            kind: SurfaceKind::Skill,
            id: id.into(),
            sha256: files_sha256(&files).unwrap(),
            entry: None,
            payload: Payload::Files(files),
        }],
    }
}

fn server_binary(root: &Path, marker: &str) -> Vec<u8> {
    let source = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/mcp_fake_server.rs"),
    )
    .unwrap();
    let path = root.join(format!("{marker}.rs"));
    let binary = root.join(marker);
    let source = source.replace("fixture", marker);
    let source = source.replace("let result = match method.as_str()", r#"
        if method == "initialize" {
            if let Some(log) = &log_path {
                std::fs::write(log.with_extension("cwd"), std::env::current_dir()?.to_string_lossy().as_bytes())?;
            }
        }
        let result = match method.as_str()"#);
    let source = if marker == "native-zbroken" {
        source.replace(
            "\"tools/list\" => concat!(",
            "\"tools/list\" => \"null\", \"native-unused\" => concat!(",
        )
    } else if matches!(marker, "native-one-hung" | "native-one-timeout") {
        source.replace(
            "let result = match method.as_str()",
            r#"
        if method == "tools/call" {
            if let Some(log) = &log_path {
                let _child = std::process::Command::new("/bin/sh")
                    .args(["-c", "sleep 1; printf leaked > \"$1\"", "fixture"])
                    .arg(log.with_extension("leaked")).spawn()?;
                append_log(log, "{\"event\":\"call_started\"}")?;
            }
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
        let result = match method.as_str()"#,
        )
    } else {
        source
    };
    let source = if marker == "native-one-slow-init" {
        source.replace(
            "let result = match method.as_str()",
            r#"
        if method == "initialize" {
            if let Some(log) = &log_path {
                std::fs::write(log.with_extension("initializing"), b"started")?;
                while !log.with_extension("allow").exists() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
        let result = match method.as_str()"#,
        )
    } else if marker == "native-one-timeout" {
        source.replace("sleep 1;", "sleep 3;")
    } else {
        source
    };
    std::fs::write(&path, source).unwrap();
    let result = std::process::Command::new("rustc")
        .args(["--edition=2021", "--crate-name", "native_mcp_fixture"])
        .arg(path)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    std::fs::read(binary).unwrap()
}

#[tokio::test]
async fn failed_later_mcp_preparation_keeps_earlier_process_bytes_until_observed_rollback() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let mut package = spec(root.path(), "native-one");
    let mut broken = spec(root.path(), "native-zbroken")
        .surfaces
        .into_iter()
        .find(|surface| surface.kind == SurfaceKind::Mcp)
        .unwrap();
    broken.id = "zbroken".into();
    package.surfaces.push(broken);
    let publication = store.apply_sync(&package).unwrap();
    store
        .publish_selection_sync(ID, None, &[publication.cursor()])
        .unwrap();
    let entry = publication
        .surfaces
        .iter()
        .find(|surface| surface.id == "catalog")
        .unwrap()
        .entry
        .as_ref()
        .unwrap();
    let session = session(root.path(), Arc::new(Model::new())).await;
    let before = session.capability_catalog_stamp();
    PluginSelectionHost::current(store.clone(), ID, root.path())
        .await
        .unwrap()
        .apply(&session)
        .await
        .expect("a refused package must not close the session");
    assert_eq!(
        session.capability_catalog_stamp(),
        before,
        "failed MCP candidate published a partial Skill/MCP projection"
    );
    assert!(
        alive(pid(root.path(), "native-one")),
        "first acquired connection was not transferred to rollback ownership"
    );
    assert!(!alive(pid(root.path(), "native-zbroken")));
    store.withdraw_sync("acme/cognition").unwrap();
    store.collect_retired_sync("acme/cognition").unwrap();
    assert!(
        entry.exists(),
        "unobserved rollback released a live process's package"
    );
    let report = session.drain_capability_cleanup().await;
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.rollback_batches, 1);
    assert_eq!(report.effects_closed, 1);
    store.collect_retired_sync("acme/cognition").unwrap();
    assert!(!entry.exists());
    assert!(!alive(pid(root.path(), "native-one")));
    session.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_an_actual_native_mcp_call_retires_its_process_and_descendant_writes() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let publication = store
        .apply_sync(&spec(root.path(), "native-one-hung"))
        .unwrap();
    store
        .publish_selection_sync(ID, None, &[publication.cursor()])
        .unwrap();
    let entry = publication
        .surfaces
        .iter()
        .find(|surface| surface.kind == SurfaceKind::Mcp)
        .unwrap()
        .entry
        .clone()
        .unwrap();
    let model = Arc::new(Model::new());
    model.release.add_permits(1);
    let session = session(root.path(), model.clone()).await;
    PluginSelectionHost::current(store.clone(), ID, root.path())
        .await
        .unwrap()
        .apply(&session)
        .await
        .unwrap();
    let task = tokio::spawn({
        let session = session.clone();
        async move { session.send("Call the native MCP.", None).await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if std::fs::read_to_string(root.path().join("native-one-hung.log"))
                .unwrap_or_default()
                .contains("call_started")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("actual native MCP tools/call did not start");
    store.withdraw_sync("acme/cognition").unwrap();
    store.collect_retired_sync("acme/cognition").unwrap();
    assert!(entry.exists());
    assert!(session.cancel().await);
    let _outcome = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while alive(pid(root.path(), "native-one-hung")) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    store.collect_retired_sync("acme/cognition").unwrap();
    assert!(!entry.exists());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        !root.path().join("native-one-hung.leaked").exists(),
        "cancelled MCP descendant still wrote after Run settlement"
    );
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    session.close().await;
}

#[tokio::test]
async fn native_session_refuses_client_mcp_override_before_any_process_effect() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let publication = store.apply_sync(&spec(root.path(), "native-one")).unwrap();
    store
        .publish_selection_sync(ID, None, &[publication.cursor()])
        .unwrap();
    let host = PluginSelectionHost::current(store, ID, root.path())
        .await
        .unwrap();
    let cursor = host.cursor().clone();
    let mut launch = crate::agent::test_launch(Some(root.path().join("sessions")));
    launch.workspace = root.path().to_owned();
    let agent = crate::A3sCodeAgent::new(launch).with_plugin_selection(host);
    let request = serde_json::json!({"cwd":root.path(),"mcpServers":[{"name":"catalog","command":"/bin/sh","args":["-c","exit 0"],"env":[]}],"_meta":{"a3sPluginSelection":cursor}});
    assert!(agent_client_protocol::Agent::new_session(
        &agent,
        serde_json::from_value(request).unwrap()
    )
    .await
    .is_err());
    assert!(!root.path().join("native-one.log").exists());
    assert!(!root.path().join(".a3s").exists());
}

fn spec(root: &Path, marker: &str) -> PackageSpec {
    let mcp = vec![
        PackageFile { path: "server".into(), bytes: server_binary(root, marker) },
        PackageFile { path: "server.json".into(), bytes: serde_json::to_vec(&serde_json::json!({"type":"stdio", "command":"server", "tool_timeout_secs":if marker == "native-one-timeout" {1} else {60}, "env":{"A3S_TEST_MCP_LOG":root.join(format!("{marker}.log"))}})).unwrap() },
    ];
    let skill = vec![PackageFile { path: "SKILL.md".into(), bytes: format!("---\nname: native-reason\ndescription: {marker} package reasoning\nallowed-tools: search_skills(*)\n---\n# {marker}\n").into_bytes() }];
    PackageSpec {
        package_id: "acme/cognition".into(),
        surfaces: vec![
            SurfaceSpec {
                kind: SurfaceKind::Skill,
                id: "native-reason".into(),
                sha256: files_sha256(&skill).unwrap(),
                entry: None,
                payload: Payload::Files(skill),
            },
            SurfaceSpec {
                kind: SurfaceKind::Mcp,
                id: "catalog".into(),
                sha256: files_sha256(&mcp).unwrap(),
                entry: None,
                payload: Payload::Files(mcp),
            },
        ],
    }
}

struct Model {
    calls: AtomicUsize,
    admitted: Semaphore,
    release: Semaphore,
    results: Mutex<Vec<String>>,
    expect_mcp_timeout: bool,
}

impl Model {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            admitted: Semaphore::new(0),
            release: Semaphore::new(0),
            results: Mutex::new(Vec::new()),
            expect_mcp_timeout: false,
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

#[async_trait::async_trait]
impl LlmClient for Model {
    async fn complete(
        &self,
        messages: &[Message],
        _system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        if !tools.iter().any(|tool| tool.name == "mcp__catalog__lookup") {
            return Ok(Self::response(
                vec![ContentBlock::Text {
                    text: "auxiliary completion".into(),
                }],
                "end_turn",
            ));
        }
        let call = self.calls.load(Ordering::SeqCst);
        if call.is_multiple_of(2) {
            let marker = if call == 0 {
                "native-one"
            } else {
                "native-two"
            };
            anyhow::ensure!(
                tools
                    .iter()
                    .any(|tool| tool.name == "mcp__catalog__lookup"
                        && tool.description.contains(marker)),
                "actual MCP definition changed inside the Run"
            );
            self.calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                self.admitted.add_permits(1);
                self.release.acquire().await?.forget();
            }
            Ok(Self::response(
                vec![
                    ContentBlock::ToolUse {
                        id: format!("search-{call}"),
                        name: "search_skills".into(),
                        input: serde_json::json!({"query":"native-reason", "limit":3}),
                    },
                    ContentBlock::ToolUse {
                        id: format!("lookup-{call}"),
                        name: "mcp__catalog__lookup".into(),
                        input: serde_json::json!({}),
                    },
                ],
                "tool_use",
            ))
        } else {
            for prefix in ["search", "lookup"] {
                let id = format!("{prefix}-{}", call - 1);
                let result = messages
                    .iter()
                    .rev()
                    .flat_map(|message| &message.content)
                    .find_map(|block| match block {
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } if tool_use_id == &id => Some(content.as_text()),
                        _ => None,
                    })
                    .with_context(|| format!("actual {prefix} result missing"))?;
                let marker = if call == 1 {
                    "native-one"
                } else {
                    "native-two"
                };
                if self.expect_mcp_timeout && prefix == "lookup" {
                    anyhow::ensure!(
                        result.contains("timed out"),
                        "MCP timeout was not reported: {result}"
                    );
                } else {
                    anyhow::ensure!(
                        result.contains(marker),
                        "{prefix} resolved another package: {result}"
                    );
                }
                self.results.lock().unwrap().push(result);
            }
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
        anyhow::bail!("this contract uses actual non-streaming Harness Runs")
    }
}

async fn session(root: &Path, model: Arc<Model>) -> Arc<AgentSession> {
    let config = crate::agent::test_launch(Some(root.join("sessions"))).config;
    let agent = Agent::from_config(config).await.unwrap();
    let options = SessionOptions::new()
        .with_session_id(uuid::Uuid::new_v4().to_string())
        .with_llm_client(model)
        .with_planning_mode(PlanningMode::Disabled)
        .with_permission_policy(
            a3s_code_core::permissions::PermissionPolicy::new()
                .allow("search_skills")
                .allow("mcp__catalog__lookup"),
        );
    Arc::new(
        agent
            .session_async(root.to_string_lossy().into_owned(), Some(options))
            .await
            .unwrap(),
    )
}

fn pid(root: &Path, marker: &str) -> u32 {
    std::fs::read_to_string(root.join(format!("{marker}.log")))
        .unwrap()
        .lines()
        .find_map(|line| {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            (event["event"] == "process_started").then(|| event["pid"].as_u64().unwrap() as u32)
        })
        .unwrap()
}

fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_skill_and_real_stdio_mcp_stay_frozen_and_ready_processes_retain_their_package() {
    let root = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let old = store.apply_sync(&spec(root.path(), "native-one")).unwrap();
    let first = store
        .publish_selection_sync(ID, None, &[old.cursor()])
        .unwrap();
    let old_entry = old
        .surfaces
        .iter()
        .find(|surface| surface.kind == SurfaceKind::Mcp)
        .unwrap()
        .entry
        .clone()
        .unwrap();
    let model = Arc::new(Model::new());
    let session = session(root.path(), model.clone()).await;
    PluginSelectionHost::current(store.clone(), ID, root.path())
        .await
        .unwrap()
        .apply(&session)
        .await
        .unwrap();
    let old_pid = pid(root.path(), "native-one");
    assert!(alive(old_pid));
    assert_eq!(
        std::fs::read_to_string(root.path().join("native-one.cwd")).unwrap(),
        std::fs::canonicalize(root.path())
            .unwrap()
            .to_string_lossy()
    );
    let mut task = tokio::spawn({
        let session = session.clone();
        async move { session.send("Query the package skill and MCP.", None).await }
    });
    tokio::select! {
        result = &mut task => panic!("Run ended before actual native model admission: {result:?}"),
        admitted = tokio::time::timeout(Duration::from_secs(5), model.admitted.acquire()) => admitted.unwrap().unwrap().forget(),
    }
    let next = store.apply_sync(&spec(root.path(), "native-two")).unwrap();
    let second = store
        .publish_selection_sync(ID, Some(&first.cursor()), &[next.cursor()])
        .unwrap();
    PluginSelectionHost::current(store.clone(), ID, root.path())
        .await
        .unwrap()
        .apply(&session)
        .await
        .unwrap();
    store.collect_retired_sync("acme/cognition").unwrap();
    assert!(old_entry.exists());
    assert!(alive(old_pid));
    model.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let report = session.drain_capability_cleanup().await;
    assert!(report.is_clean(), "{report:?}");
    assert!(report.effects_closed > 0);
    store.collect_retired_sync("acme/cognition").unwrap();
    assert!(!old_entry.exists());
    assert!(!alive(old_pid));
    session.send("Query the new package.", None).await.unwrap();
    assert_eq!(model.results.lock().unwrap().len(), 4);
    store.withdraw_selection_sync(&second.cursor()).unwrap();
    store.withdraw_sync("acme/cognition").unwrap();
    assert!(session
        .send("This selection was withdrawn.", None)
        .await
        .is_err());
    assert_eq!(model.calls.load(Ordering::SeqCst), 4);
    let next_entry = next
        .surfaces
        .iter()
        .find(|surface| surface.kind == SurfaceKind::Mcp)
        .unwrap()
        .entry
        .as_ref()
        .unwrap();
    store.collect_retired_sync("acme/cognition").unwrap();
    assert!(
        next_entry.exists(),
        "a Ready MCP process lost its real package lease before close"
    );
    assert!(alive(pid(root.path(), "native-two")));
    session.close().await;
    store.collect_retired_sync("acme/cognition").unwrap();
    assert!(!next_entry.exists());
    assert!(!alive(pid(root.path(), "native-two")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn timed_out_native_mcp_call_revokes_readiness_and_reaps_before_retirement() {
    let root = tempfile::tempdir().unwrap();
    // Host-owned process diagnostics are not candidate workspace edits.
    let workspace = tempfile::tempdir().unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let publication = store
        .apply_sync(&spec(root.path(), "native-one-timeout"))
        .unwrap();
    store
        .publish_selection_sync(ID, None, &[publication.cursor()])
        .unwrap();
    let entry = publication
        .surfaces
        .iter()
        .find(|surface| surface.kind == SurfaceKind::Mcp)
        .unwrap()
        .entry
        .clone()
        .unwrap();
    let mut model = Model::new();
    model.expect_mcp_timeout = true;
    model.release.add_permits(1);
    let model = Arc::new(model);
    let session = session(workspace.path(), model.clone()).await;
    PluginSelectionHost::current(store.clone(), ID, workspace.path())
        .await
        .unwrap()
        .apply(&session)
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        session.send("Call the native MCP.", None),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(model.calls.load(Ordering::SeqCst), 2);
    assert!(
        !alive(pid(root.path(), "native-one-timeout")),
        "timeout settled before the server exited"
    );
    assert!(session
        .send("Reuse the failed connection.", None)
        .await
        .is_err());
    assert_eq!(
        model.calls.load(Ordering::SeqCst),
        2,
        "new Run inferred through an unready binding"
    );
    store.withdraw_sync("acme/cognition").unwrap();
    store.collect_retired_sync("acme/cognition").unwrap();
    assert!(
        !entry.exists(),
        "reaped server retained retired package bytes without a Run"
    );
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert!(
        !root.path().join("native-one-timeout.leaked").exists(),
        "timed-out MCP descendant wrote after settlement"
    );
    session.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_replacement_during_real_mcp_preparation_refuses_commit_and_keeps_rollback_ownership(
) {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let store = ReconcileStore::open(root.path().join("generations")).unwrap();
    let publication = store
        .apply_sync(&spec(root.path(), "native-one-slow-init"))
        .unwrap();
    store
        .publish_selection_sync(ID, None, &[publication.cursor()])
        .unwrap();
    let entry = publication
        .surfaces
        .iter()
        .find(|surface| surface.kind == SurfaceKind::Mcp)
        .unwrap()
        .entry
        .clone()
        .unwrap();
    let host = PluginSelectionHost::current(store.clone(), ID, &workspace)
        .await
        .unwrap();
    let model = Arc::new(Model::new());
    let session = session(&workspace, model.clone()).await;
    let before = session.capability_catalog_stamp();
    let task = tokio::spawn({
        let session = session.clone();
        async move { host.apply(&session).await }
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !root
            .path()
            .join("native-one-slow-init.initializing")
            .exists()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    std::fs::rename(&workspace, root.path().join("retired-workspace")).unwrap();
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(root.path().join("native-one-slow-init.allow"), b"proceed").unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(
        error.to_string().contains("workspace binding refused"),
        "{error}"
    );
    assert_eq!(session.capability_catalog_stamp(), before);
    assert_eq!(model.calls.load(Ordering::SeqCst), 0);
    assert!(alive(pid(root.path(), "native-one-slow-init")));
    store.withdraw_sync("acme/cognition").unwrap();
    store.collect_retired_sync("acme/cognition").unwrap();
    assert!(
        entry.exists(),
        "workspace refusal released a still-owned MCP process's artifacts"
    );
    let report = session.drain_capability_cleanup().await;
    assert!(report.is_clean(), "{report:?}");
    assert_eq!(report.rollback_batches, 1);
    assert_eq!(report.effects_closed, 1);
    assert!(!alive(pid(root.path(), "native-one-slow-init")));
    store.collect_retired_sync("acme/cognition").unwrap();
    assert!(!entry.exists());
    session.close().await;
}
