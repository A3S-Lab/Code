use super::*;

#[tokio::test(flavor = "current_thread")]
async fn async_session_builder_initializes_all_async_resources_on_current_thread_runtime() {
    use base64::Engine as _;

    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let memory_dir = root.path().join("memory");
    let sessions_dir = root.path().join("sessions");
    let mcp = Arc::new(crate::mcp::manager::McpManager::new());
    let options = SessionOptions::new()
        .with_session_id("async-current-thread")
        .with_queue_config(SessionQueueConfig::default())
        .with_file_memory(&memory_dir)
        .with_file_session_store(&sessions_dir)
        .with_mcp(mcp);

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_builder(workspace.display().to_string())
        .options(options)
        .build()
        .await
        .unwrap();

    assert!(session.has_queue());
    assert!(session.memory().is_some());
    session.save().await.unwrap();
    assert!(memory_dir.is_dir());
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("async-current-thread");
    assert!(sessions_dir
        .join("v1")
        .join("sessions")
        .join(format!("id_{key}.json"))
        .is_file());
}

#[tokio::test(flavor = "current_thread")]
async fn async_session_builder_returns_typed_memory_initialization_error() {
    let root = tempfile::tempdir().unwrap();
    let blocked = root.path().join("not-a-directory");
    std::fs::write(&blocked, "file blocks directory creation").unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let error = agent
        .session_builder(root.path().display().to_string())
        .options(SessionOptions::new().with_file_memory(blocked.join("memory")))
        .build()
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::SessionInitialization {
            resource: crate::error::SessionBuildResource::MemoryStore,
            ..
        }
    ));
    assert!(error
        .to_string()
        .replace('\\', "/")
        .contains("not-a-directory/memory"));
}

#[tokio::test(flavor = "current_thread")]
async fn async_session_builder_returns_typed_trajectory_initialization_error() {
    let root = tempfile::tempdir().unwrap();
    let blocked = root.path().join("not-a-directory");
    std::fs::write(&blocked, "file blocks trajectory parent creation").unwrap();

    let agent = Agent::from_config(test_config()).await.unwrap();
    let error = agent
        .session_builder(root.path().display().to_string())
        .options(SessionOptions::new().with_rl_trajectory(
            crate::rl_trajectory::RlTrajectoryConfig::new(blocked.join("trajectory.jsonl")),
        ))
        .build()
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::SessionInitialization {
            resource: crate::error::SessionBuildResource::RlTrajectory,
            ..
        }
    ));
    assert!(error.to_string().contains("not-a-directory"));
}

#[tokio::test(flavor = "current_thread")]
async fn sync_session_compatibility_rejects_async_resource_specs_without_panicking() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();

    let error = agent
        .session(root.path().display().to_string(), None)
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::AsyncSessionBuildRequired {
            resource: crate::error::SessionBuildResource::MemoryStore,
        }
    ));

    let memory_dir = root.path().join("memory");
    let error = agent
        .session(
            root.path().display().to_string(),
            Some(SessionOptions::new().with_file_memory(memory_dir.clone())),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        crate::error::CodeError::AsyncSessionBuildRequired {
            resource: crate::error::SessionBuildResource::MemoryStore,
        }
    ));
    assert!(!memory_dir.exists());

    let trajectory_path = root.path().join("trajectory.jsonl");
    let error = agent
        .session(
            root.path().display().to_string(),
            Some(SessionOptions::new().with_rl_trajectory(
                crate::rl_trajectory::RlTrajectoryConfig::new(trajectory_path.clone()),
            )),
        )
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::CodeError::AsyncSessionBuildRequired {
            resource: crate::error::SessionBuildResource::RlTrajectory,
        }
    ));
    assert!(!trajectory_path.exists());
}

#[tokio::test(flavor = "current_thread")]
async fn sync_session_compatibility_accepts_preinitialized_resources() {
    let root = tempfile::tempdir().unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let memory = Arc::new(a3s_memory::InMemoryStore::new());

    let session = agent
        .session(
            root.path().display().to_string(),
            Some(SessionOptions::new().with_memory(memory)),
        )
        .unwrap();

    assert!(session.memory().is_some());
    assert!(!session.has_queue());
}

#[tokio::test(flavor = "current_thread")]
async fn async_session_keeps_inherited_mcp_sources_separate_from_live_extensions() {
    use crate::mcp::{McpManager, McpServerConfig, McpTransportConfig};

    fn server(name: &str) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: McpTransportConfig::Stdio {
                command: "unused".to_string(),
                args: Vec::new(),
            },
            enabled: false,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 60,
        }
    }

    let global = Arc::new(McpManager::new());
    global.register_server(server("global-source")).await;
    let configured = Arc::new(McpManager::new());
    configured
        .register_server(server("configured-source"))
        .await;

    let mut agent = Agent::from_config(test_config()).await.unwrap();
    agent.global_mcp = Some(Arc::clone(&global));
    let session = agent
        .session_async(
            "/tmp/test-mcp-source-isolation",
            Some(SessionOptions::new().with_mcp(Arc::clone(&configured))),
        )
        .await
        .unwrap();

    assert!(!Arc::ptr_eq(&session.mcp_manager, &global));
    assert!(!Arc::ptr_eq(&session.mcp_manager, &configured));
    assert_eq!(session.inherited_mcp_managers.len(), 2);
    assert!(Arc::ptr_eq(&session.inherited_mcp_managers[0], &global));
    assert!(Arc::ptr_eq(&session.inherited_mcp_managers[1], &configured));
    assert_eq!(session.mcp_managers.len(), 3);
    assert!(Arc::ptr_eq(
        session.mcp_managers.last().unwrap(),
        &session.mcp_manager
    ));

    let status = session.mcp_status().await;
    assert!(status.contains_key("global-source"));
    assert!(status.contains_key("configured-source"));
    assert!(!global.get_status().await.contains_key("configured-source"));
    assert!(!configured.get_status().await.contains_key("global-source"));
    assert!(session.mcp_manager.get_status().await.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn async_sessions_never_share_their_live_mcp_manager() {
    let inherited = Arc::new(crate::mcp::McpManager::new());
    let agent = Agent::from_config(test_config()).await.unwrap();
    let first = agent
        .session_async(
            "/tmp/test-private-mcp-a",
            Some(SessionOptions::new().with_mcp(Arc::clone(&inherited))),
        )
        .await
        .unwrap();
    let second = agent
        .session_async(
            "/tmp/test-private-mcp-b",
            Some(SessionOptions::new().with_mcp(Arc::clone(&inherited))),
        )
        .await
        .unwrap();

    assert!(!Arc::ptr_eq(&first.mcp_manager, &second.mcp_manager));
    assert!(!Arc::ptr_eq(&first.mcp_manager, &inherited));
    assert!(!Arc::ptr_eq(&second.mcp_manager, &inherited));
}

#[tokio::test(flavor = "current_thread")]
async fn sync_session_uses_cached_global_mcp_without_blocking_runtime() {
    let global = Arc::new(crate::mcp::McpManager::new());
    let mut agent = Agent::from_config(test_config()).await.unwrap();
    agent.global_mcp = Some(Arc::clone(&global));

    let session = agent
        .session(
            "/tmp/test-sync-global-mcp",
            Some(SessionOptions::new().with_memory(Arc::new(a3s_memory::InMemoryStore::new()))),
        )
        .unwrap();

    assert_eq!(session.inherited_mcp_managers.len(), 1);
    assert!(Arc::ptr_eq(&session.inherited_mcp_managers[0], &global));
    assert!(!Arc::ptr_eq(&session.mcp_manager, &global));
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_global_mcp_source_is_deduplicated() {
    let global = Arc::new(crate::mcp::McpManager::new());
    let mut agent = Agent::from_config(test_config()).await.unwrap();
    agent.global_mcp = Some(Arc::clone(&global));

    let session = agent
        .session_async(
            "/tmp/test-deduplicated-global-mcp",
            Some(SessionOptions::new().with_mcp(Arc::clone(&global))),
        )
        .await
        .unwrap();

    assert_eq!(session.inherited_mcp_managers.len(), 1);
    assert_eq!(session.mcp_managers.len(), 2);
    assert!(Arc::ptr_eq(&session.inherited_mcp_managers[0], &global));
}

#[tokio::test(flavor = "current_thread")]
async fn live_mcp_add_remove_is_session_local_and_restores_inherited_precedence() {
    use crate::mcp::{McpManager, McpServerConfig, McpTransportConfig};

    fn server(name: &str, enabled: bool) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport: McpTransportConfig::Stdio {
                command: "unused".to_string(),
                args: Vec::new(),
            },
            enabled,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 60,
        }
    }

    let global = Arc::new(McpManager::new());
    global.register_server(server("shared", false)).await;
    let configured = Arc::new(McpManager::new());
    configured.register_server(server("shared", true)).await;

    let mut agent = Agent::from_config(test_config()).await.unwrap();
    agent.global_mcp = Some(Arc::clone(&global));
    let session = agent
        .session_async(
            "/tmp/test-live-mcp-isolation",
            Some(SessionOptions::new().with_mcp(Arc::clone(&configured))),
        )
        .await
        .unwrap();

    let inherited_status = session.mcp_status().await;
    assert!(
        inherited_status["shared"].enabled,
        "the later configured source must override the global status"
    );

    let error = session
        .add_mcp_server(server("shared", false))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("disabled"));
    let rolled_back_status = session.mcp_status().await;
    assert!(
        rolled_back_status["shared"].enabled,
        "a failed session-local add must reveal the inherited source"
    );
    assert!(
        !session
            .mcp_manager
            .get_status()
            .await
            .contains_key("shared"),
        "connect failure must roll back local config and error state"
    );
    assert!(
        !global.get_status().await["shared"].enabled,
        "live add must not mutate the global manager"
    );
    assert!(
        configured.get_status().await["shared"].enabled,
        "live add must not mutate the configured inherited manager"
    );

    session.remove_mcp_server("shared").await.unwrap();
    let restored_status = session.mcp_status().await;
    assert!(
        restored_status["shared"].enabled,
        "removing the local shadow must reveal the configured source again"
    );
    assert!(!session
        .mcp_manager
        .get_status()
        .await
        .contains_key("shared"));
    assert!(global.get_status().await.contains_key("shared"));
    assert!(configured.get_status().await.contains_key("shared"));
}

#[tokio::test(flavor = "current_thread")]
async fn close_serializes_with_live_mcp_mutation_and_rejects_late_add_remove() {
    use crate::mcp::{McpServerConfig, McpTransportConfig};

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = Arc::new(
        agent
            .session_async("/tmp/test-live-mcp-close-race", None)
            .await
            .unwrap(),
    );
    let local_tool_name = "mcp__close-owned__local";
    let shadowed_tool_name = "mcp__close-owned__shared";
    let shadowed_tool: Arc<dyn crate::tools::Tool> =
        Arc::new(NamedSessionTool(shadowed_tool_name.to_string()));
    session
        .register_dynamic_tool(Arc::clone(&shadowed_tool))
        .unwrap();
    session
        .close_handle
        .mcp_tool_ownership
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .install(
            "close-owned",
            &session.tool_executor,
            vec![
                Arc::new(NamedSessionTool(local_tool_name.to_string())),
                Arc::new(NamedSessionTool(shadowed_tool_name.to_string())),
            ],
        );
    assert!(session
        .tool_names()
        .iter()
        .any(|name| name == local_tool_name));
    let mutation = session.close_handle.extension_mutation.lock().await;

    let closing_session = Arc::clone(&session);
    let close_task = tokio::spawn(async move {
        closing_session.close().await;
    });
    while !session.is_closed() {
        tokio::task::yield_now().await;
    }

    let adding_session = Arc::clone(&session);
    let add_task = tokio::spawn(async move {
        adding_session
            .add_mcp_server(McpServerConfig {
                name: "late".to_string(),
                transport: McpTransportConfig::Stdio {
                    command: "unused".to_string(),
                    args: Vec::new(),
                },
                enabled: false,
                env: HashMap::new(),
                oauth: None,
                tool_timeout_secs: 60,
            })
            .await
    });

    drop(mutation);
    tokio::time::timeout(std::time::Duration::from_secs(2), close_task)
        .await
        .expect("close must finish after the admitted mutation releases")
        .unwrap();
    let add_error = add_task.await.unwrap().unwrap_err();
    assert!(matches!(
        add_error,
        crate::error::CodeError::SessionClosed { .. }
    ));
    assert!(session.mcp_manager.get_status().await.is_empty());
    assert!(session.mcp_manager.list_connected().await.is_empty());
    assert!(
        !session
            .tool_names()
            .iter()
            .any(|name| name == local_tool_name),
        "close must unwind wrappers owned by session-local MCP servers"
    );
    let restored_shadow = session
        .tool_executor
        .registry()
        .get(shadowed_tool_name)
        .unwrap();
    assert!(Arc::ptr_eq(&restored_shadow, &shadowed_tool));

    let remove_error = session.remove_mcp_server("late").await.unwrap_err();
    assert!(matches!(
        remove_error,
        crate::error::CodeError::SessionClosed { .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn live_mcp_remove_cleanup_failure_still_commits_registry_removal() {
    use crate::mcp::{McpClient, McpServerConfig, McpTransportConfig};

    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async("/tmp/test-live-mcp-remove-cleanup", None)
        .await
        .unwrap();
    let server_name = "failing-cleanup";
    let local_tool_name = "mcp__failing-cleanup__local";
    session
        .mcp_manager
        .register_server(McpServerConfig {
            name: server_name.to_string(),
            transport: McpTransportConfig::Stdio {
                command: "unused".to_string(),
                args: Vec::new(),
            },
            enabled: true,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 60,
        })
        .await;
    session
        .mcp_manager
        .insert_client_for_test(
            server_name,
            Arc::new(McpClient::new(
                server_name.to_string(),
                Arc::new(FailingCloseSessionTransport),
            )),
        )
        .await;
    session
        .close_handle
        .mcp_tool_ownership
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .install(
            server_name,
            &session.tool_executor,
            vec![Arc::new(NamedSessionTool(local_tool_name.to_string()))],
        );

    let error = session.remove_mcp_server(server_name).await.unwrap_err();
    assert!(error.to_string().contains("transport cleanup failed"));
    assert!(!session.mcp_manager.contains_server(server_name).await);
    assert!(session.mcp_manager.get_client(server_name).await.is_none());
    assert!(!session
        .tool_names()
        .iter()
        .any(|name| name == local_tool_name));
    session.remove_mcp_server(server_name).await.unwrap();
}

#[tokio::test(flavor = "current_thread")]
async fn sync_global_mcp_servers_hot_applies_without_rebuilding_agent() {
    use crate::mcp::test_support::{mcp_tool, RecordingMcpTransport};
    use crate::mcp::{McpClient, McpServerConfig, McpTransportConfig};

    let agent = Agent::from_config(test_config()).await.unwrap();
    assert!(agent.global_mcp.is_some());
    let manager = Arc::clone(agent.global_mcp.as_ref().unwrap());

    let session = agent
        .session_async("/tmp/test-hot-mcp-sync", None)
        .await
        .unwrap();
    assert!(session.inherits_mcp_managers());
    assert!(Arc::ptr_eq(
        session.inherited_mcp_managers.first().unwrap(),
        &manager
    ));

    let config = McpServerConfig {
        name: "hot-probe".to_string(),
        transport: McpTransportConfig::Stdio {
            command: "unused".to_string(),
            args: Vec::new(),
        },
        enabled: true,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 60,
    };
    agent
        .sync_global_mcp_servers(vec![config.clone()])
        .await
        .unwrap();
    assert!(manager.contains_server("hot-probe").await);

    // Seed a connected client so republish can discover tools without a
    // real stdio process (sync's connect against "unused" is best-effort).
    let transport = RecordingMcpTransport::new("2024-11-05", vec![mcp_tool("ping", "ping")]);
    let client = Arc::new(McpClient::new(
        "hot-probe".to_string(),
        Arc::clone(&transport) as Arc<dyn crate::mcp::transport::McpTransport>,
    ));
    client.initialize().await.unwrap();
    client.list_tools().await.unwrap();
    manager.insert_client_for_test("hot-probe", client).await;
    agent.refresh_mcp_tools().await.unwrap();

    session.republish_inherited_mcp_tools().await.unwrap();
    assert!(session
        .tool_names()
        .iter()
        .any(|name| name == "mcp__hot-probe__ping"));

    agent.sync_global_mcp_servers(Vec::new()).await.unwrap();
    session.republish_inherited_mcp_tools().await.unwrap();
    assert!(!manager.contains_server("hot-probe").await);
    assert!(!session
        .tool_names()
        .iter()
        .any(|name| name == "mcp__hot-probe__ping"));
}
