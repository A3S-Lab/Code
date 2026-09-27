use super::*;
use crate::session::acp_session::{
    sampler_turn::call_with_auth_retry,
    support::{create_test_actor, prepare_call, test_agent_with_tools},
    tool_dispatch::dispatch_tool,
};
use a3s_code_tools::{
    computer::{
        local::MockFs,
        types::{AsyncFileSystem, ComputerError},
    },
    implementations::use_tool::UseTool,
    types::resources::{
        ManagedGatewayToolCallResponse, ManagedGatewayToolCaller, ManagedGatewayToolCatalog,
        ManagedGatewayToolClient, ManagedGatewayToolSource,
    },
};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

tokio::task_local! {
    static FILE_EVENTS: std::cell::RefCell<Vec<Value>>;
}

pub(super) fn record_event<T: a3s_code_telemetry::TelemetryEvent>(event: &T) -> bool {
    FILE_EVENTS
        .try_with(|events| {
            events
                .borrow_mut()
                .push(json!({"name": T::NAME, "payload": event}));
        })
        .is_ok()
}

async fn capture_events<F: std::future::Future>(future: F) -> (F::Output, Vec<Value>) {
    FILE_EVENTS
        .scope(std::cell::RefCell::new(Vec::new()), async {
            let output = future.await;
            (output, FILE_EVENTS.with(|events| events.take()))
        })
        .await
}

struct CountingFs {
    files: MockFs,
    reads: AtomicUsize,
    blocked: std::sync::atomic::AtomicBool,
}
#[async_trait::async_trait]
impl AsyncFileSystem for CountingFs {
    async fn read_file(&self, _: &std::path::Path) -> Result<Vec<u8>, ComputerError> {
        panic!("transport must never use unbounded reads")
    }
    fn supports_bounded_read(&self) -> bool {
        true
    }
    async fn read_file_bounded(
        &self,
        path: &std::path::Path,
        limit: usize,
    ) -> Result<Vec<u8>, ComputerError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if self.blocked.load(Ordering::Relaxed) {
            return std::future::pending().await;
        }
        self.files.read_file_bounded(path, limit).await
    }
    async fn write_file(&self, path: &std::path::Path, data: &[u8]) -> Result<(), ComputerError> {
        self.files.write_file(path, data).await
    }
    async fn delete_file(&self, path: &std::path::Path) -> Result<(), ComputerError> {
        self.files.delete_file(path).await
    }
}

struct Receiver {
    calls: parking_lot::Mutex<Vec<Value>>,
    mutate: Option<Arc<CountingFs>>,
}
#[async_trait::async_trait]
impl ManagedGatewayToolCaller for Receiver {
    async fn call_tool(
        &self,
        call_id: &str,
        arguments: Value,
        _: &str,
    ) -> Result<ManagedGatewayToolCallResponse, a3s_tool_runtime::ToolError> {
        assert_eq!("fixture.update", call_id);
        let first = {
            let mut calls = self.calls.lock();
            calls.push(arguments);
            calls.len() == 1
        };
        if first && let Some(fs) = &self.mutate {
            fs.files
                .set_file("/tmp/mcp-source.json", b"invalid replacement")
                .await;
            return Err(a3s_tool_runtime::ToolError::custom(
                "http_failure",
                "HTTP 401 Unauthorized",
            )
            .with_details(json!({"status":401})));
        }
        Ok(ManagedGatewayToolCallResponse {
            result: json!({"content":[{"type":"text","text":"RECEIPT"}]}),
            connectors_needing_reauth: vec![],
        })
    }
}

async fn fixture(
    retry: bool,
) -> (
    SessionActor,
    Arc<CountingFs>,
    Arc<Receiver>,
    tokio::sync::mpsc::UnboundedReceiver<a3s_acp_lib::AcpClientMessage>,
) {
    let (gateway, receiver) = tokio::sync::mpsc::unbounded_channel();
    let (persistence, _persist_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut actor = create_test_actor(0, 256_000, 85, gateway, persistence).await;
    actor.hook_resolved_workspace_root = "/tmp".to_owned();
    *actor.agent.borrow_mut() = test_agent_with_tools(vec![
        a3s_code_tools::registry::types::ToolConfig::for_tool::<UseTool>(),
    ])
    .await;
    let fs = Arc::new(CountingFs {
        files: MockFs::new(),
        reads: AtomicUsize::new(0),
        blocked: std::sync::atomic::AtomicBool::new(false),
    });
    let remote = Arc::new(Receiver {
        calls: parking_lot::Mutex::new(vec![]),
        mutate: retry.then(|| fs.clone()),
    });
    let resources = actor.tool_bridge_handle().shared_resources().await;
    {
        let mut resources = resources.lock().await;
        resources.insert(FileSystem(fs.clone()));
        resources.insert(ManagedGatewayToolCatalog(std::collections::HashMap::from(
            [(
                "fixture__update".to_owned(),
                ManagedGatewayToolSource {
                    connector_id: "fixture".to_owned(),
                    connector_name: "fixture".to_owned(),
                    tool_id: "update".to_owned(),
                    tool_name: "update".to_owned(),
                    call_id: "fixture.update".to_owned(),
                },
            )],
        )));
        resources.insert(ManagedGatewayToolClient(remote.clone()));
    }
    actor
        .workspace_ops
        .bind_local_session(
            &actor.session_id_string(),
            actor.tool_context.cwd.as_path().to_path_buf(),
            actor.tool_context.hunk_tracker_handle.clone(),
            actor.tool_bridge_handle().toolset(),
            None,
        )
        .unwrap();
    (actor, fs, remote, receiver)
}

fn install_client_hooks(actor: &SessionActor, events: &[a3s_code_hooks::event::HookEventName]) {
    for event in events {
        actor.client_hooks.borrow_mut().insert(
            *event,
            vec![crate::extensions::hooks::ClientHookGroup {
                matcher: Some(
                    a3s_code_hooks::matcher::HookMatcher::new("fixture__update").unwrap(),
                ),
                callback_ids: vec!["fixture-hook".to_owned()],
                timeout: None,
            }],
        );
    }
}

fn record_client_hooks(
    mut gateway: tokio::sync::mpsc::UnboundedReceiver<a3s_acp_lib::AcpClientMessage>,
    seen: Arc<parking_lot::Mutex<Vec<Value>>>,
    reply: Value,
) -> tokio_util::task::AbortOnDropHandle<()> {
    tokio_util::task::AbortOnDropHandle::new(tokio::task::spawn_local(async move {
        while let Some(message) = gateway.recv().await {
            match message {
                a3s_acp_lib::AcpClientMessage::ExtMethod(args) => {
                    seen.lock()
                        .push(serde_json::from_str::<Value>(args.request.params.get()).unwrap());
                    let body = serde_json::value::to_raw_value(&reply).unwrap();
                    args.response_tx
                        .send(Ok(acp::ExtResponse::new(Arc::from(body))))
                        .unwrap();
                }
                a3s_acp_lib::AcpClientMessage::SessionNotification(args) => {
                    args.response_tx.send(Ok(())).unwrap();
                }
                _ => {}
            }
        }
    }))
}

fn call(arguments: Value) -> crate::sampling::types::ToolCallResponse {
    crate::sampling::types::ToolCallResponse {
        id: "fixture-call".to_owned(),
        kind: "function".to_owned(),
        function: crate::sampling::types::ToolCallFunction::new("use_tool", arguments.to_string()),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn concatenated_inline_recovers_first_object_without_rewriting_authored_arguments() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, fs, receiver, _gateway) = fixture(false).await;
            for (arguments, tail) in [
                (
                    json!({"input":"{\"body\":\"nested }{ text\"}"}),
                    r#"{"tool_name":"fixture__update","tool_input":{"later":true}}"#,
                ),
                (Value::Null, r#"{"unrelated":true}"#),
            ] {
                let first = json!({"tool_name":"fixture__update","tool_input":arguments});
                let mut request = call(first.clone());
                request.function.arguments = format!("{first} {tail}");
                let authored = request.function.arguments.clone();
                let prepared = prepare_call(&actor, request).await.unwrap();
                assert_eq!(2, prepared.concatenated_json_count);
                assert_eq!(authored, prepared.raw_arguments);
                assert_eq!(&first, prepared.execution_arguments());
                assert!(prepared.mcp_file.is_none());
                dispatch_tool(&actor.workspace_ops, &prepared, &actor.session_id_string())
                    .await
                    .unwrap();
            }
            assert_eq!(
                vec![json!({"input":"{\"body\":\"nested }{ text\"}"}), json!({})],
                *receiver.calls.lock()
            );
            assert_eq!(0, fs.reads.load(Ordering::Relaxed));
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn inline_recovery_keeps_raw_duplicate_checks_and_file_forms_strict() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, fs, receiver, gateway) = fixture(false).await;
            install_client_hooks(&actor, &[a3s_code_hooks::event::HookEventName::PreToolUse]);
            let hooks = Arc::new(parking_lot::Mutex::new(Vec::new()));
            let responder = record_client_hooks(gateway, hooks.clone(), json!({}));
            let valid = r#"{"tool_name":"fixture__update","tool_input":{}}"#;
            for first in [
                r#"{"tool_name":"fixture__update","tool_name":"fixture__other","tool_input":{}}"#,
                r#"{"tool_name":"fixture__update","tool_input":{},"tool_\u0069nput":null}"#,
                r#"{"tool_name":"fixture__update","tool_input":{},"file":null}"#,
            ] {
                for suffix in ["", valid] {
                    let mut request = call(Value::Null);
                    request.function.arguments = format!("{first}{suffix}");
                    assert!(matches!(
                        prepare_call(&actor, request).await,
                        Err(ToolLoop::ToolParsingError)
                    ));
                }
            }
            for source in [
                json!({"file":"/tmp/mcp-source.json"}),
                json!({"tool_name":"fixture__update","tool_input_file":"/tmp/mcp-source.json"}),
            ] {
                let mut request = call(source.clone());
                request.function.arguments = format!("{source}{valid}");
                assert!(matches!(
                    prepare_call(&actor, request).await,
                    Err(ToolLoop::ToolParsingError)
                ));
            }
            let mut request = call(Value::Null);
            request.function.arguments = format!("{valid} trailing");
            assert!(matches!(
                prepare_call(&actor, request).await,
                Err(ToolLoop::ToolParsingError)
            ));
            assert_eq!(0, fs.reads.load(Ordering::Relaxed));
            assert!(hooks.lock().is_empty());
            assert!(receiver.calls.lock().is_empty());
            drop(responder);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn file_advertisement_requires_host_preparation_and_bounded_filesystem() {
    use a3s_code_agent::{AgentBuilder, AgentDefinition};
    use a3s_code_tools::implementations::use_tool::UseToolParams;
    use a3s_code_tools::registry::types::{ToolConfig, ToolServerConfig};
    use a3s_code_tools::types::resources::Params;

    tokio::task::LocalSet::new()
        .run_until(async {
            for case in ["default", "forged", "delegated", "supported", "renamed"] {
                let (actor, _, _, _gateway) = fixture(false).await;
                let dir = tempfile::tempdir().unwrap();
                let mut spec = crate::session::agent_rebuild::test_rebuild_spec_default();
                let fields = Arc::get_mut(&mut spec).unwrap();
                fields.working_directory = dir.path().to_path_buf();
                fields.bridge_state_path = dir.path().join("tool_state.json");
                if case == "delegated" {
                    fields.fs_backend =
                        Arc::new(a3s_code_workspace::file_system::AcpFsAdapter::new(
                            actor.notifications.gateway.clone(),
                            actor.session_info.id.clone(),
                        ));
                }
                let mut config = ToolConfig::for_tool::<UseTool>();
                if case != "default" {
                    config = config.with_param("file_input_supported", true);
                }
                if case == "renamed" {
                    config = config
                        .with_name("invoke_remote")
                        .with_param_rename("tool_name", "target")
                        .with_param_rename("tool_input", "args")
                        .with_param_rename("tool_input_file", "args_path")
                        .with_param_rename("file", "source");
                }
                let mut definition = AgentDefinition::default_grok_build();
                definition.inject_default_tools = false;
                definition.discover_skills = false;
                definition.agents_md = false;
                definition.tool_config = ToolServerConfig {
                    tools: vec![config],
                    behavior_preset: None,
                };
                let agent = if matches!(case, "default" | "forged") {
                    AgentBuilder::new(
                        dir.path().to_path_buf(),
                        spec.terminal_backend.clone(),
                        spec.tools_notification_handle.clone(),
                    )
                    .from_definition(definition)
                    .with_fs(spec.fs_backend.clone())
                    .build()
                    .await
                } else {
                    spec.build_agent(definition, a3s_code_agent::DEFAULT_SYSTEM_PROMPT_LABEL)
                        .await
                }
                .unwrap();
                *actor.agent.borrow_mut() = agent;
                let supported = matches!(case, "supported" | "renamed");
                let definitions = actor.tool_bridge_handle().toolset().tool_definitions();
                let tool = &definitions
                    .iter()
                    .find(|definition| {
                        definition.function.name
                            == if case == "renamed" {
                                "invoke_remote"
                            } else {
                                "use_tool"
                            }
                    })
                    .unwrap()
                    .function;
                let properties = tool
                    .parameters
                    .get("properties")
                    .unwrap()
                    .as_object()
                    .unwrap();
                let expected = if case == "renamed" {
                    vec!["args", "args_path", "source", "target"]
                } else if supported {
                    vec!["file", "tool_input", "tool_input_file", "tool_name"]
                } else {
                    vec!["tool_input", "tool_name"]
                };
                let mut actual = properties.keys().map(String::as_str).collect::<Vec<_>>();
                actual.sort_unstable();
                assert_eq!(expected, actual, "{case}");
                assert_eq!(supported, tool.parameters.get("oneOf").is_some(), "{case}");
                let params = actor
                    .tool_bridge_handle()
                    .read_resource::<Params<UseToolParams>>()
                    .await
                    .unwrap();
                assert_eq!(supported, params.0.supports_file_input(), "{case}");
                let description = tool.description.as_deref().unwrap();
                let hint = actor.rendered_mcp_hint().await.unwrap();
                if supported {
                    let file_key = if case == "renamed" { "source" } else { "file" };
                    let args_key = if case == "renamed" {
                        "args_path"
                    } else {
                        "tool_input_file"
                    };
                    for text in [description, hint.as_str()] {
                        assert!(
                            text.contains(file_key) && text.contains(args_key),
                            "{case}: {text}"
                        );
                        assert!(!text.contains("${{"), "{case}: {text}");
                    }
                } else {
                    assert!(!description.to_lowercase().contains("file"), "{case}");
                    assert!(!hint.to_lowercase().contains("file"), "{case}");
                    assert!(!tool.parameters.to_string().contains("file"), "{case}");
                }
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn messages_backend_advertises_inline_only_use_tool() {
    use a3s_code_agent::AgentDefinition;
    use a3s_code_sampling_types::ApiBackend;
    use a3s_code_tools::registry::types::{ToolConfig, ToolServerConfig};

    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, _, _, _gateway) = fixture(false).await;
            let dir = tempfile::tempdir().unwrap();
            let mut spec = crate::session::agent_rebuild::test_rebuild_spec_default();
            let fields = Arc::get_mut(&mut spec).unwrap();
            fields.working_directory = dir.path().to_path_buf();
            fields.bridge_state_path = dir.path().join("tool_state.json");
            let mut definition = AgentDefinition::default_grok_build();
            definition.inject_default_tools = false;
            definition.discover_skills = false;
            definition.agents_md = false;
            definition.tool_config = ToolServerConfig {
                tools: vec![ToolConfig::for_tool::<UseTool>()],
                behavior_preset: None,
            };
            *actor.agent.borrow_mut() = spec
                .build_agent(definition, a3s_code_agent::DEFAULT_SYSTEM_PROMPT_LABEL)
                .await
                .unwrap();

            for backend in [
                ApiBackend::Responses,
                ApiBackend::Messages,
                ApiBackend::ChatCompletions,
            ] {
                let mut config = actor.chat_state_handle.get_sampling_config().await.unwrap();
                config.api_backend = backend.clone();
                actor.chat_state_handle.update_sampling_config(config);
                let hidden = backend == ApiBackend::Messages;

                let definitions = actor.prepare_tool_definitions_inner().await;
                let tool = &definitions
                    .iter()
                    .find(|definition| definition.function.name == "use_tool")
                    .unwrap()
                    .function;
                let mut keys = tool
                    .parameters
                    .get("properties")
                    .and_then(serde_json::Value::as_object)
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                keys.sort_unstable();
                let expected = if hidden {
                    vec!["tool_input", "tool_name"]
                } else {
                    vec!["file", "tool_input", "tool_input_file", "tool_name"]
                };
                assert_eq!(expected, keys, "{backend:?}");
                assert_eq!(
                    !hidden,
                    tool.parameters.get("oneOf").is_some(),
                    "{backend:?}"
                );
                let description = tool.description.as_deref().unwrap();
                let hint = actor.rendered_mcp_hint().await.unwrap();
                for text in [description, hint.as_str(), &tool.parameters.to_string()] {
                    assert_eq!(
                        !hidden,
                        text.to_lowercase().contains("file"),
                        "{backend:?}: {text}"
                    );
                }
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn finalized_randomized_wrapper_preserves_canonical_file_keys() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, _fs, _receiver, _gateway) = fixture(false).await;
            let mut config = a3s_code_tools::registry::types::ToolConfig::for_tool::<UseTool>();
            config.name_override = Some("invoke_remote".to_owned());
            config.params_name_overrides = Some(std::collections::HashMap::from([
                ("tool_name".to_owned(), "target".to_owned()),
                ("tool_input".to_owned(), "args".to_owned()),
                ("tool_input_file".to_owned(), "args_path".to_owned()),
                ("file".to_owned(), "source".to_owned()),
                ("unused".to_owned(), "extra".to_owned()),
            ]));
            *actor.agent.borrow_mut() = test_agent_with_tools(vec![config]).await;
            let toolset = actor.tool_bridge_handle().toolset();
            let input = UseToolInput::Inline(InlineMcpInvocation {
                tool_name: "fixture__update".to_owned(),
                tool_input: json!({"file":"remote", "tool_input":"remote"}),
            });
            let model = toolset
                .model_mcp_arguments("invoke_remote", &input)
                .unwrap();
            assert_eq!(
                json!({"target":"fixture__update","args":{"file":"remote", "tool_input":"remote"}}),
                model
            );
            assert_eq!(
                input,
                toolset
                    .parse_mcp_wrapper_json("invoke_remote", &model.to_string())
                    .unwrap()
            );
            let recovered =
                recover_concatenated_inline(&toolset, "invoke_remote", &format!("{model}{{}}"))
                    .unwrap();
            assert_eq!(input, recovered);
            let collision =
                r#"{"target":"fixture__update","tool_name":"fixture__other","args":{}}{}"#;
            assert!(
                recover_concatenated_inline(&toolset, "invoke_remote", collision)
                    .unwrap_err()
                    .detail
                    .contains("duplicate or ambiguous")
            );
            assert!(matches!(
                toolset.try_parse("invoke_remote", &model).await.unwrap(),
                ToolInput::UseTool(UseToolInput::Inline(_))
            ));
            let guarded = toolset
                .call(
                    "invoke_remote",
                    json!({"source":"source.json"}),
                    "unresolved-file",
                    None,
                )
                .await
                .unwrap_err();
            assert!(guarded.detail.contains("supported shell preparation"));
            assert!(
                toolset
                    .parse_mcp_wrapper_json("invoke_remote", r#"{"source":"a","file":"b"}"#)
                    .unwrap_err()
                    .to_string()
                    .contains("ambiguous")
            );
            let definitions = serde_json::to_value(toolset.tool_definitions()).unwrap();
            let schema = definitions.pointer("/0/function/parameters").unwrap();
            assert_eq!(
                Some(&json!(true)),
                schema.pointer("/oneOf/0/properties/args/additionalProperties")
            );
            assert_eq!(
                Some(&json!(["source"])),
                schema.pointer("/oneOf/2/required")
            );
            assert_eq!(
                Some(&json!(["args_path"])),
                schema.pointer("/oneOf/0/not/anyOf/0/required")
            );
            for branch in schema.get("oneOf").unwrap().as_array().unwrap() {
                assert_eq!(Some(&json!("object")), branch.get("type"));
                for required in branch.get("required").unwrap().as_array().unwrap() {
                    assert!(
                        branch
                            .get("properties")
                            .unwrap()
                            .get(required.as_str().unwrap())
                            .is_some()
                    );
                }
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_sources_never_dispatch_or_echo_source_details() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, fs, receiver, _gateway) = fixture(false).await;
            let invocation = json!({"file":"/tmp/mcp-source.json"});
            let arguments =
                json!({"tool_name":"fixture__update","tool_input_file":"/tmp/mcp-source.json"});
            for (bytes, wrapper, diagnostic) in [
                (vec![0xff], invocation.clone(), None),
                (vec![b' '; MAX_SOURCE_BYTES + 1], invocation.clone(), None),
                (
                    b"\n\"DO_NOT_ECHO_SOURCE\"".to_vec(),
                    invocation,
                    Some("Invalid canonical MCP invocation JSON at line 2, column 20"),
                ),
                (
                    b"\n{\"DO_NOT_ECHO_SOURCE\":".to_vec(),
                    arguments,
                    Some("Invalid MCP arguments JSON at line 2, column 22"),
                ),
            ] {
                fs.files.set_file("/tmp/mcp-source.json", &bytes).await;
                assert!(matches!(
                    prepare_call(&actor, call(wrapper)).await,
                    Err(ToolLoop::Continue)
                ));
                if let Some(diagnostic) = diagnostic {
                    let history =
                        serde_json::to_value(actor.chat_state_handle.get_conversation().await)
                            .unwrap();
                    let result = history
                        .as_array()
                        .unwrap()
                        .last()
                        .unwrap()
                        .get("content")
                        .unwrap()
                        .as_str()
                        .unwrap();
                    assert_eq!(diagnostic, result);
                    assert!(!result.contains("DO_NOT_ECHO_SOURCE"));
                }
            }
            assert!(receiver.calls.lock().is_empty());
            assert_eq!(4, fs.reads.load(Ordering::Relaxed));
        })
        .await;
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn ignored_logical_symlink_source_is_denied_before_loading() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use a3s_code_tools::types::resources::{GitignoreFilter, RespectGitignore};
            let dir = tempfile::tempdir().unwrap();
            let root = dunce::canonicalize(dir.path()).unwrap();
            let source = root.join("source.json");
            let link = root.join("ignored.json");
            std::fs::write(
                &source,
                br#"{"tool_name":"fixture__update","tool_input":{}}"#,
            )
            .unwrap();
            std::os::unix::fs::symlink(&source, &link).unwrap();
            let (actor, fs, remote, _gateway) = fixture(false).await;
            let mut ignore = ignore::gitignore::GitignoreBuilder::new(&root);
            ignore.add_line(None, "ignored.json").unwrap();
            let resources = actor.tool_bridge_handle().shared_resources().await;
            resources
                .lock()
                .await
                .insert(GitignoreFilter::new(ignore.build().unwrap(), root));
            resources.lock().await.insert(RespectGitignore(true));
            assert!(matches!(
                prepare_call(&actor, call(json!({"file":link}))).await,
                Err(ToolLoop::Continue)
            ));
            assert_eq!(0, fs.reads.load(Ordering::Relaxed));
            assert_eq!(0, remote.calls.lock().len());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn transport_memory_policy_validates_and_denies_without_model_bookkeeping() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use a3s_code_tools::types::memory_v2::{
                MemoryV2Access, MemoryV2AccessResource, MemoryV2Write,
            };
            #[derive(Debug)]
            struct MemoryPolicy {
                validated: parking_lot::Mutex<Vec<PathBuf>>,
                model_reads: AtomicUsize,
                denies_read: bool,
            }
            impl MemoryV2Access for MemoryPolicy {
                fn validate_read(&self, path: &std::path::Path) -> Result<bool, String> {
                    self.validated.lock().push(path.to_path_buf());
                    if self.denies_read {
                        Err("fixture memory read denied".to_owned())
                    } else {
                        Ok(true)
                    }
                }
                fn record_read(&self, _: &std::path::Path, _: &[u8]) -> Result<(), String> {
                    self.model_reads.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
                fn preflight_write(&self, _: &std::path::Path, _: &[u8]) -> Result<bool, String> {
                    Ok(false)
                }
                fn write_file(
                    &self,
                    _: &std::path::Path,
                    _: &[u8],
                ) -> Result<MemoryV2Write, String> {
                    Ok(MemoryV2Write::Outside)
                }
                fn scope_roots(&self) -> [PathBuf; 2] {
                    [PathBuf::from("/tmp"), PathBuf::from("/tmp")]
                }
            }
            for denies_read in [false, true] {
                let dir = tempfile::tempdir().unwrap();
                let physical = dunce::canonicalize(dir.path())
                    .unwrap()
                    .join("physical.json");
                std::fs::write(&physical, b"{}").unwrap();
                #[cfg(unix)]
                let logical = {
                    let link = physical.with_file_name("logical.json");
                    std::os::unix::fs::symlink(&physical, &link).unwrap();
                    link
                };
                #[cfg(not(unix))]
                let logical = physical.clone();
                let (actor, fs, remote, gateway) = fixture(false).await;
                let policy = Arc::new(MemoryPolicy {
                    validated: parking_lot::Mutex::new(Vec::new()),
                    model_reads: AtomicUsize::new(0),
                    denies_read,
                });
                actor
                    .tool_bridge_handle()
                    .shared_resources()
                    .await
                    .lock()
                    .await
                    .insert(MemoryV2AccessResource(policy.clone()));
                fs.files
                    .set_file(
                        &physical,
                        br#"{"tool_name":"fixture__update","tool_input":{}}"#,
                    )
                    .await;
                install_client_hooks(&actor, &[a3s_code_hooks::event::HookEventName::PreToolUse]);
                let hooks = Arc::new(parking_lot::Mutex::new(Vec::new()));
                let responder = record_client_hooks(gateway, hooks.clone(), json!({}));
                actor
                    .execute_tool_calls(vec![call(json!({"file":logical}))], None)
                    .await
                    .unwrap();
                let mut expected_paths = vec![logical.clone()];
                if !denies_read && logical != physical {
                    expected_paths.push(physical);
                }
                assert_eq!(expected_paths, *policy.validated.lock());
                let expected_calls = usize::from(!denies_read);
                assert_eq!(expected_calls, fs.reads.load(Ordering::Relaxed));
                assert_eq!(expected_calls, hooks.lock().len());
                assert_eq!(expected_calls, remote.calls.lock().len());
                assert_eq!(0, policy.model_reads.load(Ordering::Relaxed));
                let history =
                    serde_json::to_value(actor.chat_state_handle.get_conversation().await).unwrap();
                let result = history
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row.get("tool_call_id") == Some(&json!("fixture-call")))
                    .unwrap();
                assert!(result.get("content").unwrap().as_str().unwrap().contains(
                    if denies_read {
                        "fixture memory read denied"
                    } else {
                        "RECEIPT"
                    }
                ));
                drop(responder);
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn acp_filesystem_is_unsupported_without_read_text_file_fallback() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, _fs, remote, _gateway) = fixture(false).await;
            let (gateway, mut calls) = tokio::sync::mpsc::unbounded_channel();
            let backend = a3s_code_workspace::file_system::AcpFsAdapter::new(
                a3s_acp_lib::AcpAgentGatewaySender::new(gateway),
                actor.session_info.id.clone(),
            );
            actor
                .tool_bridge_handle()
                .shared_resources()
                .await
                .lock()
                .await
                .insert(FileSystem(Arc::new(backend)));
            assert!(matches!(
                prepare_call(&actor, call(json!({"file":"/tmp/mcp-source.json"}))).await,
                Err(ToolLoop::Continue)
            ));
            assert_eq!(0, remote.calls.lock().len());
            assert!(calls.try_recv().is_err());
        })
        .await;
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn deadline_or_cancelled_preparation_never_dispatches() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, fs, remote, _gateway) = fixture(false).await;
            fs.blocked.store(true, Ordering::Relaxed);
            let mut deferred = vec![];
            let result = actor
                .prepare_tool_call(
                    call(json!({"file":"/tmp/mcp-source.json"})),
                    &mut deferred,
                    None,
                )
                .await
                .unwrap();
            assert!(matches!(result, Err(ToolLoop::Continue)));
            let mut deferred = vec![];
            let cancelled = tokio::time::timeout(
                Duration::from_millis(1),
                actor.prepare_tool_call(
                    call(json!({"file":"/tmp/mcp-source.json"})),
                    &mut deferred,
                    None,
                ),
            )
            .await;
            assert!(cancelled.is_err());
            assert_eq!(0, remote.calls.lock().len());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn approval_preview_preserves_arguments_and_obeys_remaining_budget() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, _fs, remote, _gateway) = fixture(false).await;
            let arguments = json!({"body":"α".repeat(a3s_code_hooks::event::MAX_PAYLOAD_SIZE)});
            for remaining in [FILE_OPERATION_TIMEOUT, Duration::ZERO] {
                let mut source = McpFileSource::start(
                    PathBuf::from("/tmp/mcp-source.json"),
                    a3s_code_telemetry::events::McpFileInputKind::Invocation,
                    "grok-4.6".to_owned(),
                );
                source.operation_remaining = remaining;
                let preparation = McpFilePreparation::Resolved {
                    source,
                    authored: json!({"file":"/tmp/mcp-source.json"}),
                    authored_json: r#"{"file":"/tmp/mcp-source.json"}"#.to_owned(),
                };
                let mut input = ToolInput::UseTool(UseToolInput::Inline(InlineMcpInvocation {
                    tool_name: "fixture__update".to_owned(),
                    tool_input: arguments.clone(),
                }));
                let result = preparation
                    .approval(
                        &actor,
                        &acp::ToolCallId::new("preview"),
                        "use_tool",
                        &mut input,
                    )
                    .await;
                if remaining.is_zero() {
                    assert!(
                        result
                            .unwrap_err()
                            .to_string()
                            .contains("MCP approval preview timed out")
                    );
                } else {
                    let (_, _, preview) = result.unwrap();
                    let text = preview.get("tool_input").unwrap().as_str().unwrap();
                    assert!(text.ends_with(" [truncated]"));
                    assert!(
                        text.len()
                            <= a3s_code_hooks::event::MAX_PAYLOAD_SIZE + " [truncated]".len()
                    );
                }
                let ToolInput::UseTool(UseToolInput::Inline(input)) = input else {
                    panic!("inline input")
                };
                assert_eq!(arguments, input.tool_input);
            }
            assert!(remote.calls.lock().is_empty());
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn snapshot_overflow_emits_once_on_the_session_task_with_measured_bytes() {
    use a3s_code_telemetry::session_ctx::{TelemetryCtx, with_session_ctx};
    let context = TelemetryCtx::new(
        "mcp-overflow-session".to_owned(),
        Arc::new(tokio::sync::Mutex::new(7)),
    );
    let (error, events) = capture_events(with_session_ctx(context, async {
        let mut source = McpFileSource::start(
            PathBuf::from("source"),
            a3s_code_telemetry::events::McpFileInputKind::Arguments,
            "grok-4.6".to_owned(),
        );
        source.bytes = 42;
        let arguments = json!({"tool_name":"fixture__update","tool_input":{"body":"x".repeat(MAX_BATCH_SNAPSHOT_BYTES)}});
        PreparedMcpFile::freeze(source, arguments).await.unwrap_err()
    })).await;
    assert_eq!(
        "MCP effective invocation exceeds the 32 MiB snapshot limit",
        error
    );
    let names: Vec<_> = events
        .iter()
        .map(|event| event.get("name").unwrap().as_str().unwrap())
        .collect();
    assert_eq!(
        vec![
            "mcp_file_input_used",
            "mcp_file_input_limit_hit",
            "mcp_file_input_completed"
        ],
        names
    );
    for event in &events {
        assert_eq!(
            Some(&json!("grok-4.6")),
            event.pointer("/payload/model_id"),
            "{event}"
        );
    }
    let completion = events.last().unwrap();
    assert_eq!(
        Some(&json!("failed")),
        completion.pointer("/payload/outcome")
    );
    assert_eq!(
        Some(&json!(42)),
        completion.pointer("/payload/source_bytes")
    );
    assert_eq!(
        events.get(1).unwrap().pointer("/payload/observed_bytes"),
        completion.pointer("/payload/snapshot_bytes")
    );
    assert!(
        completion
            .pointer("/payload/snapshot_bytes")
            .unwrap()
            .as_u64()
            .unwrap()
            > MAX_BATCH_SNAPSHOT_BYTES as u64
    );
}

#[tokio::test(flavor = "current_thread")]
async fn failed_preparations_preserve_order_and_known_byte_counts() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for stage in [
                "read",
                "parse",
                "hook",
                #[cfg(unix)]
                "invocation-rewrite",
                #[cfg(unix)]
                "arguments-rewrite",
                "source-deny",
                "permission",
                "source-budget",
                "snapshot-budget",
            ] {
                let (mut actor, fs, remote, gateway) = fixture(false).await;
                if matches!(stage, "source-deny" | "permission") {
                    use a3s_code_workspace::permission::types::{
                        PatternMode, PermissionConfig, PermissionRule, RuleAction, ToolFilter,
                    };
                    let (permissions, _events) =
                        a3s_code_workspace::permission::spawn_permission_manager(
                            actor.session_info.id.clone(),
                            actor.notifications.gateway.clone(),
                            a3s_code_paths::AbsPathBuf::new(PathBuf::from("/tmp")).unwrap(),
                            a3s_code_workspace::permission::ClientType::Generic,
                            Some(PermissionConfig::new(vec![PermissionRule {
                                action: RuleAction::Deny,
                                tool: if stage == "source-deny" {
                                    ToolFilter::Read
                                } else {
                                    ToolFilter::Mcp
                                },
                                pattern: None,
                                pattern_mode: PatternMode::Glob,
                            }])),
                            vec![],
                            vec![],
                            true,
                            None,
                        );
                    actor.permissions = permissions;
                }
                let document = if stage == "parse" {
                    "not JSON".to_owned()
                } else {
                    json!({"tool_name":"fixture__update","tool_input":{"body":"loaded"}})
                        .to_string()
                };
                if stage != "read" {
                    fs.files
                        .set_file("/tmp/mcp-source.json", document.as_bytes())
                        .await;
                }
                if stage.ends_with("rewrite") {
                    let rewrite = if stage == "invocation-rewrite" {
                        json!({"file":"/tmp/another.json"})
                    } else {
                        json!({"tool_name":"fixture__update","tool_input_file":"/tmp/another.json"})
                    };
                    let script = format!(
                        "printf '%s' '{}'",
                        json!({"hookSpecificOutput":{"updatedInput":rewrite}})
                    );
                    *actor.hook_registry.borrow_mut() = Some(Arc::new(
                        crate::session::acp_session::client_hooks_tests::file_registry_with_spec(
                            a3s_code_hooks::event::HookEventName::PreToolUse,
                            &script,
                        ),
                    ));
                }
                install_client_hooks(&actor, &[a3s_code_hooks::event::HookEventName::PreToolUse]);
                let hooks = Arc::new(parking_lot::Mutex::new(Vec::new()));
                let responder = record_client_hooks(
                    gateway,
                    hooks.clone(),
                    if stage == "hook" {
                        json!({"decision":"deny"})
                    } else {
                        json!({})
                    },
                );
                let (_, events) = capture_events(async {
                    let result =
                        prepare_call(&actor, call(json!({"file":"/tmp/mcp-source.json"}))).await;
                    if stage.ends_with("budget") {
                        let prepared = result.unwrap();
                        let mut budget = McpFileBatchBudget {
                            source_bytes: if stage == "source-budget" {
                                MAX_BATCH_SOURCE_BYTES
                            } else {
                                0
                            },
                            snapshot_bytes: if stage == "snapshot-budget" {
                                MAX_BATCH_SNAPSHOT_BYTES
                            } else {
                                0
                            },
                        };
                        assert_eq!(
                            "MCP file input batch budget exceeded",
                            budget.admit(&prepared).unwrap_err()
                        );
                    } else {
                        let expected = result.unwrap_err();
                        assert!(matches!(
                            (stage, expected),
                            (
                                "hook" | "invocation-rewrite" | "arguments-rewrite",
                                ToolLoop::HookDenied { .. }
                            ) | (
                                "read" | "parse" | "source-deny" | "permission",
                                ToolLoop::Continue
                            )
                        ));
                    }
                })
                .await;
                let completed: Vec<_> = events
                    .iter()
                    .filter(|event| event.get("name") == Some(&json!("mcp_file_input_completed")))
                    .collect();
                assert_eq!(1, completed.len(), "{stage}");
                let completion = completed.first().unwrap();
                assert_eq!(
                    Some(&json!("failed")),
                    completion.pointer("/payload/outcome")
                );
                assert_eq!(
                    Some(&json!(if matches!(stage, "read" | "source-deny") {
                        0
                    } else {
                        document.len()
                    })),
                    completion.pointer("/payload/source_bytes")
                );
                assert_eq!(
                    Some(&json!(if stage.ends_with("budget") {
                        document.len()
                    } else {
                        0
                    })),
                    completion.pointer("/payload/snapshot_bytes")
                );
                assert!(remote.calls.lock().is_empty());
                assert_eq!(
                    usize::from(stage != "source-deny"),
                    fs.reads.load(Ordering::Relaxed),
                    "{stage}"
                );
                let hooks = hooks.lock();
                assert_eq!(
                    usize::from(
                        !stage.ends_with("rewrite")
                            && !matches!(stage, "read" | "parse" | "source-deny"),
                    ),
                    hooks.len()
                );
                for hook in hooks.iter() {
                    assert_eq!(
                        Some(
                            &json!({"tool_name":"fixture__update","tool_input":{"body":"loaded"}})
                        ),
                        hook.get("toolInput")
                    );
                    assert_eq!(Some(&json!("fixture__update")), hook.get("toolName"));
                }
                drop(responder);
            }
        })
        .await;
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn real_auth_retry_uses_one_read_and_post_hook_snapshot() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (actor, fs, receiver, gateway) = fixture(true).await;
            let arguments = json!({"input":"{\"body\":\"rewritten\"}"});
            fs.files
                .set_file(
                    "/tmp/mcp-source.json",
                    br#"{"tool_name":"fixture__update","tool_input":{"input":"original"}}"#,
                )
                .await;
            let rewritten = json!({"tool_name":"fixture__update","tool_input":arguments});
            let script = format!(
                "printf '%s' '{}'",
                json!({"hookSpecificOutput":{"updatedInput":rewritten}})
            );
            *actor.hook_registry.borrow_mut() = Some(Arc::new(
                crate::session::acp_session::client_hooks_tests::file_registry_with_spec(
                    a3s_code_hooks::event::HookEventName::PreToolUse,
                    &script,
                ),
            ));
            install_client_hooks(
                &actor,
                &[
                    a3s_code_hooks::event::HookEventName::PreToolUse,
                    a3s_code_hooks::event::HookEventName::PostToolUse,
                ],
            );
            let hook_calls = Arc::new(parking_lot::Mutex::new(Vec::new()));
            let responder = record_client_hooks(gateway, hook_calls.clone(), json!({}));
            let prepared = prepare_call(&actor, call(json!({"file":"/tmp/mcp-source.json"})))
                .await
                .unwrap();
            let auth_dir = tempfile::tempdir().unwrap();
            let auth = Arc::new(a3s_code_login::AuthManager::new(
                auth_dir.path(),
                a3s_code_login::GrokComConfig::default(),
            ));
            auth.hot_swap(a3s_code_login::GrokAuth {
                key: "fixture".into(),
                auth_mode: a3s_code_login::AuthMode::Oidc,
                refresh_token: Some("fixture".into()),
                expires_at: Some(chrono::DateTime::from_timestamp(0, 0).unwrap()),
                ..a3s_code_login::GrokAuth::test_default()
            });
            struct Refresh;
            #[async_trait::async_trait]
            impl a3s_code_login::refresh::TokenRefresher for Refresh {
                async fn refresh(
                    &self,
                    _: a3s_code_login::refresh::RefreshReason,
                ) -> a3s_code_login::refresh::RefreshOutcome {
                    a3s_code_login::refresh::RefreshOutcome::Success(Box::new(
                        a3s_code_login::GrokAuth {
                            key: "fixture-refreshed".into(),
                            expires_at: Some(
                                chrono::DateTime::from_timestamp(4_102_444_800, 0).unwrap(),
                            ),
                            ..a3s_code_login::GrokAuth::test_default()
                        },
                    ))
                }
            }
            auth.set_refresher(Arc::new(Refresh));
            assert_eq!(
                &json!({"file":"/tmp/mcp-source.json"}),
                prepared.authored_arguments()
            );
            assert_eq!(&rewritten, prepared.execution_arguments());
            assert_eq!(&rewritten, prepared.hook_arguments().as_ref());
            assert!(!format!("{:?}", prepared.mcp_file.as_ref().unwrap()).contains("rewritten"));
            let session = actor.session_id_string();
            let result = call_with_auth_retry(Some(&auth), None, &prepared.tool_name, || {
                dispatch_tool(&actor.workspace_ops, &prepared, &session)
            })
            .await
            .unwrap();
            actor
                .dispatch_post_tool_use_hook(&prepared, &result.output, Some(0))
                .await;
            assert_eq!(1, fs.reads.load(Ordering::Relaxed));
            assert_eq!(vec![arguments.clone(), arguments], *receiver.calls.lock());
            assert_eq!(
                Some(b"invalid replacement".to_vec()),
                fs.files.get_file("/tmp/mcp-source.json").await
            );
            let hook_calls = hook_calls.lock();
            assert_eq!(2, hook_calls.len());
            for hook in hook_calls.iter() {
                assert_eq!(Some(&rewritten), hook.get("toolInput"));
                assert_eq!(Some(&json!("fixture__update")), hook.get("toolName"));
            }
            assert_eq!(
                json!({"file":"/tmp/mcp-source.json"}),
                serde_json::from_str::<Value>(&prepared.raw_arguments).unwrap()
            );
            drop(responder);
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn source_and_resolved_approval_are_distinct_and_reject_prevents_send() {
    tokio::task::LocalSet::new()
        .run_until(async {
            use a3s_code_workspace::permission::types::{
                PatternMode, PermissionConfig, PermissionRule, RuleAction, ToolFilter,
            };
            let (mut actor, fs, remote, _gateway) = fixture(false).await;
            fs.files
                .set_file(
                    "/tmp/mcp-source.json",
                    br#"{"tool_name":"fixture__update","tool_input":{"body":"loaded"}}"#,
                )
                .await;
            let (gateway, mut requests) = tokio::sync::mpsc::unbounded_channel();
            let (permission, _events) = a3s_code_workspace::permission::spawn_permission_manager(
                actor.session_info.id.clone(),
                a3s_acp_lib::AcpAgentGatewaySender::new(gateway),
                a3s_code_paths::AbsPathBuf::new(PathBuf::from("/tmp")).unwrap(),
                a3s_code_workspace::permission::ClientType::Desktop,
                Some(PermissionConfig::new(vec![PermissionRule {
                    action: RuleAction::Ask,
                    tool: ToolFilter::Any,
                    pattern: None,
                    pattern_mode: PatternMode::Glob,
                }])),
                vec![],
                vec![],
                false,
                None,
            );
            actor.permissions = permission;
            let prompts = Arc::new(parking_lot::Mutex::new(vec![]));
            let captured = prompts.clone();
            let responder =
                tokio_util::task::AbortOnDropHandle::new(tokio::task::spawn_local(async move {
                    while let Some(message) = requests.recv().await {
                        if let a3s_acp_lib::AcpClientMessage::RequestPermission(args) = message {
                            let first = captured.lock().is_empty();
                            captured.lock().push(args.request.tool_call.clone());
                            let outcome = acp::RequestPermissionOutcome::Selected(
                                acp::SelectedPermissionOutcome::new(acp::PermissionOptionId::new(
                                    if first { "allow-once" } else { "reject-once" },
                                )),
                            );
                            args.response_tx
                                .send(Ok(acp::RequestPermissionResponse::new(outcome)))
                                .unwrap();
                        }
                    }
                }));
            assert!(matches!(
                prepare_call(&actor, call(json!({"file":"/tmp/mcp-source.json"}))).await,
                Err(ToolLoop::PermissionReject { .. })
            ));
            let prompts = prompts.lock();
            assert_eq!(2, prompts.len());
            let source = prompts.first().unwrap();
            let target = prompts.last().unwrap();
            assert_eq!(Some(acp::ToolKind::Read), source.fields.kind);
            assert!(
                source
                    .fields
                    .title
                    .as_deref()
                    .unwrap()
                    .contains("Read MCP source")
            );
            let title = target.fields.title.as_deref().unwrap();
            assert!(
                title.contains("fixture__update")
                    && title.contains("source file: /tmp/mcp-source.json")
            );
            assert!(
                target
                    .fields
                    .raw_input
                    .as_ref()
                    .unwrap()
                    .to_string()
                    .contains("loaded")
            );
            assert_eq!(1, fs.reads.load(Ordering::Relaxed));
            assert_eq!(0, remote.calls.lock().len());
            drop(responder);
        })
        .await;
}
