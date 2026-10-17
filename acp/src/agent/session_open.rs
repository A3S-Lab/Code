//! Opening and replacing the core session behind an ACP session.

use super::*;

impl A3sCodeAgent {
    pub(super) async fn ensure_core_agent(&self) -> acp::Result<()> {
        let mut guard = self.core_agent.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let agent = Agent::from_config(self.launch.config.clone())
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        // SEC-SAMPLE1 wiring: arm per-server sampling behind all three
        // preconditions (consent bridge, per-server ceiling,
        // generation-enforced cap). Runs once, with the agent.
        if let Err(error) = self.arm_sampling_consent(&agent).await {
            tracing::warn!(error = %error, "sampling stays refused: arming failed");
        }
        *guard = Some(agent);
        Ok(())
    }

    pub(super) fn core_session_options(
        session_id: &str,
        model_id: &str,
        effort: Option<EffortLimits>,
    ) -> SessionOptions {
        let mut options = SessionOptions::new()
            .with_assistant_final_response(true)
            .with_session_id(session_id.to_string())
            .with_model(model_id.to_string())
            .with_auto_compact(true)
            .with_auto_compact_threshold(a3s_code_core::store::DEFAULT_AUTO_COMPACT_THRESHOLD);
        let mut guideline = String::new();
        if let Some(limits) = effort {
            options = options
                .with_reasoning_effort(limits.token)
                .with_thinking_budget(limits.thinking_budget)
                .with_max_tool_rounds(limits.max_tool_rounds)
                .with_max_parallel_tasks(limits.max_parallel_tasks)
                .with_max_continuation_turns(limits.max_continuation_turns);
            if limits.token == "ultracode" {
                options = options
                    .with_planning_mode(a3s_code_core::PlanningMode::Auto)
                    .with_auto_delegation_enabled(true)
                    .with_auto_parallel_delegation(true)
                    .with_manual_delegation_enabled(true);
            }
            // The numbers are a safety ceiling. Native-effort models still
            // receive them because the provider parameter does not carry a count.
            guideline = format!(
                "These limits are a maximum, not a quota. thinking_budget={} tool_rounds={}. Stop when the user's outcome is done.",
                limits.thinking_budget, limits.max_tool_rounds
            );
            if !a3s_code_core::llm::model_sends_native_effort(model_id) {
                if let Some(extra) = effort_guideline(limits.token) {
                    guideline.push('\n');
                    guideline.push_str(extra);
                }
            }
        }
        if !guideline.is_empty() {
            guideline.push('\n');
        }
        guideline.push_str(SESSION_OUTCOME_BOUNDARY);
        options = options.with_prompt_slots(
            a3s_code_core::SystemPromptSlots::default().with_guidelines(guideline),
        );
        options
    }

    /// Snapshot directory for the interactive core session.
    ///
    /// ACL `sessions_dir` wins. Otherwise the store sits beside the pager tree
    /// at `<product home>/session-store`, not inside `<product home>/sessions`.
    /// The pager scans every child of `sessions` as a workspace bucket, and the
    /// file store's `v1` directory would be one of those buckets.
    pub(super) fn interactive_session_store_dir(configured: Option<&std::path::Path>) -> PathBuf {
        interactive_session_store_dir_from(
            configured,
            std::env::var_os("A3S_HOME").as_deref(),
            std::env::var_os("HOME").as_deref(),
        )
    }

    pub(super) fn host_session_options(
        session_id: &str,
        setup: &SessionSetup<'_>,
        config: &a3s_code_core::CodeConfig,
        session_store_dir: &std::path::Path,
    ) -> acp::Result<SessionOptions> {
        let options = Self::core_session_options(session_id, setup.model_id, setup.effort);
        let mut slots = crate::host_context::merge_studio_host(
            options.prompt_slots.clone().unwrap_or_default(),
            &setup.host,
        );
        let home = a3s_code_core::hooks::command_hook_home();
        let trust = match a3s_code_core::hooks::command_hook_trust_path(&setup.workspace) {
            Ok(path) => Some(path),
            Err(error) => {
                tracing::error!(%error, "hook trust path is unavailable");
                None
            }
        };
        Self::host_session_options_with_hook_roots(
            options,
            &mut slots,
            setup,
            config,
            session_store_dir,
            home.as_deref(),
            trust.as_deref(),
            session_embedding_provider(),
        )
    }

    pub(super) fn session_options_for(
        &self,
        session_id: &str,
        setup: &SessionSetup<'_>,
        session_store_dir: &std::path::Path,
    ) -> acp::Result<SessionOptions> {
        let options = match &self.unattended {
            Some(unattended) => unattended
                .session_options(
                    session_id,
                    setup.model_id,
                    interactive_skill_dirs(&setup.workspace, &self.launch.config.skill_dirs),
                )
                .map_err(|error| acp::Error::internal_error().data(error)),
            None => Self::host_session_options(
                session_id,
                setup,
                &self.launch.config,
                session_store_dir,
            ),
        }?;
        let options = self.with_client_reads(options, session_id, &setup.workspace);
        if setup.host.source_review {
            // A fixed checker keeps the capability ceiling empty even after a mode change.
            Ok(options.with_permission_policy(
                a3s_code_core::permissions::PermissionPolicy::new().deny("*"),
            ))
        } else {
            Ok(options)
        }
    }

    pub(super) async fn open_core_session(
        &self,
        session_id: &str,
        setup: SessionSetup<'_>,
        restore: bool,
    ) -> acp::Result<LiveSession> {
        self.ensure_core_agent().await?;
        let agent_guard = self.core_agent.lock().await;
        let agent = agent_guard
            .as_ref()
            .ok_or_else(|| acp::Error::internal_error().data("core agent missing"))?;
        let store_dir =
            Self::interactive_session_store_dir(self.launch.config.sessions_dir.as_deref());
        let options = self.session_options_for(session_id, &setup, &store_dir)?;
        let options = match &self.plugin_selection {
            Some(selection) => selection.session_options(options),
            None => options,
        };
        let options = crate::harness_assemble::apply(options, setup.components)
            .map_err(|error| acp::Error::invalid_params().data(error))?;
        let engineering = options.work_policy.is_some();
        let stored = if restore {
            Self::resume_snapshot_store(&store_dir, session_id, &setup.workspace).await?
        } else {
            None
        };
        let session = match stored {
            Some(store) => {
                agent
                    .resume_session_async(session_id, options.with_session_store(store))
                    .await
            }
            None => {
                agent
                    .session_async(setup.workspace.to_string_lossy().to_string(), Some(options))
                    .await
            }
        }
        .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        if engineering {
            crate::engineering::register_scenarios(&session)
                .map_err(|error| acp::Error::internal_error().data(error))?;
        }
        let asset_denies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        if Self::attach_local_assets(&session, &setup.host, &asset_denies, self.document_bridge())?
        {
            session
                .set_permission_policy(posture_policy("default", &[]))
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        }
        Ok(LiveSession {
            turn_started: std::sync::Mutex::new(None),
            session: Arc::new(session),
            workspace: setup.workspace,
            model_id: setup.model_id.to_string(),
            posture: "default".to_string(),
            denies: Vec::new(),
            asset_denies,
            host: setup.host,
            plugin_tools: Vec::new(),
            components: setup.components.to_vec(),
            turn_queue: Arc::new(TurnQueue::new()),
            mcp_servers: setup.mcp_servers,
            active_prompt_id: std::sync::Mutex::new(None),
        })
    }

    /// Register each installed plugin executable on the live session.
    pub(super) fn admit_plugin_tools(
        session: &AgentSession,
        entries: &[serde_json::Value],
    ) -> acp::Result<()> {
        for tool in crate::plugin_tools::from_entries(entries) {
            session
                .register_dynamic_tool(tool)
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        }
        Ok(())
    }

    pub(super) async fn finish_plugin_tools(
        &self,
        mut live: LiveSession,
        entries: Vec<serde_json::Value>,
    ) -> acp::Result<LiveSession> {
        let result = if let Some(host) = &self.plugin_selection {
            host.apply(&live.session)
                .await
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))
        } else {
            Self::admit_plugin_tools(&live.session, &entries)
        };
        if let Err(error) = result {
            live.session.close().await;
            return Err(error);
        }
        if let Some(documents) = self.document_bridge() {
            let result = documents
                .tool(&live.workspace, live.session.session_id())
                .and_then(|tool| live.session.register_dynamic_tool(tool).map_err(Into::into));
            if let Err(error) = result {
                live.session.close().await;
                return Err(acp::Error::internal_error().data(error.to_string()));
            }
        }
        live.plugin_tools = entries;
        Ok(live)
    }

    /// Rebuild a session that is already registered. `session_async` rejects a
    /// live id (`session is already live or being built`); model and effort
    /// changes have to replace that session in place.
    pub(super) async fn replace_core_session(
        &self,
        current: &AgentSession,
        setup: SessionSetup<'_>,
    ) -> acp::Result<LiveSession> {
        self.ensure_core_agent().await?;
        let agent_guard = self.core_agent.lock().await;
        let agent = agent_guard
            .as_ref()
            .ok_or_else(|| acp::Error::internal_error().data("core agent missing"))?;
        let store_dir =
            Self::interactive_session_store_dir(self.launch.config.sessions_dir.as_deref());
        let mut options = self.session_options_for(current.session_id(), &setup, &store_dir)?;
        if let Some(selection) = &self.plugin_selection {
            options = selection.session_options(options);
        }
        options = crate::harness_assemble::apply(options, setup.components)
            .map_err(|error| acp::Error::invalid_params().data(error))?;
        let session = if let Some(store) = current.session_store() {
            options = options.with_session_store(store);
            agent
                .replace_session_async(current, options)
                .await
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?
        } else {
            // Nothing was persisted, so closing frees the id for a fresh session.
            current.close().await;
            agent
                .session_async(setup.workspace.to_string_lossy().to_string(), Some(options))
                .await
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?
        };
        let asset_denies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        if Self::attach_local_assets(&session, &setup.host, &asset_denies, self.document_bridge())?
        {
            session
                .set_permission_policy(posture_policy("default", &[]))
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        }
        Ok(LiveSession {
            turn_started: std::sync::Mutex::new(None),
            session: Arc::new(session),
            workspace: setup.workspace,
            model_id: setup.model_id.to_string(),
            posture: "default".to_string(),
            denies: Vec::new(),
            asset_denies,
            host: setup.host,
            plugin_tools: Vec::new(),
            components: setup.components.to_vec(),
            turn_queue: Arc::new(TurnQueue::new()),
            mcp_servers: setup.mcp_servers,
            active_prompt_id: std::sync::Mutex::new(None),
        })
    }

    /// Register `open_local_asset` when the tenant catalog is non-empty.
    pub(super) fn attach_local_assets(
        session: &AgentSession,
        host: &crate::host_context::StudioHost,
        denies: &std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        bridge: Option<std::sync::Arc<crate::document_read::DocumentBridge>>,
    ) -> acp::Result<bool> {
        let Some(roots) = host.assets.clone() else {
            if bridge.is_none() {
                return Ok(false);
            }
            session
                .register_dynamic_tool(std::sync::Arc::new(
                    crate::wiki_search::SearchWikiTool::new(
                        None,
                        bridge,
                        std::sync::Arc::clone(denies),
                    ),
                ))
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
            return Ok(true);
        };
        let catalog_empty = crate::local_assets::load_catalog(&roots).is_empty();
        let wiki = roots.wiki.as_ref().and_then(|path| {
            match a3s_code_core::workspace::WikiIndex::start(path.clone()) {
                Ok(index) => Some(index),
                Err(error) => {
                    tracing::warn!(%error, "wiki index did not start");
                    None
                }
            }
        });
        if catalog_empty && wiki.is_none() && bridge.is_none() {
            return Ok(false);
        }
        if !catalog_empty {
            session
                .register_dynamic_tool(Arc::new(crate::local_assets::OpenLocalAssetTool::new(
                    roots,
                    std::sync::Arc::clone(denies),
                )))
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        }
        if wiki.is_some() || bridge.is_some() {
            session
                .register_dynamic_tool(Arc::new(crate::wiki_search::SearchWikiTool::new(
                    wiki.map(std::sync::Arc::new),
                    bridge,
                    std::sync::Arc::clone(denies),
                )))
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        }
        Ok(true)
    }

    /// Sync the agent-global MCP manager to the UNION of the config baseline
    /// and every live session's server set. Previously the last session's
    /// desired set won alone, so opening a second session with different
    /// `mcp_servers` silently deleted the first session's servers.
    pub(super) async fn sync_session_mcp_servers(
        &self,
        for_session: Option<&str>,
        servers: &[acp::McpServer],
    ) -> acp::Result<()> {
        self.ensure_core_agent().await?;
        let guard = self.core_agent.lock().await;
        let agent = guard
            .as_ref()
            .ok_or_else(|| acp::Error::internal_error().data("core agent missing"))?;
        let baseline = {
            let mut cached = self.config_mcp.lock().await;
            if cached.is_none() {
                *cached = Some(agent.global_mcp_configs().await);
            }
            cached.clone().unwrap_or_default()
        };
        let live: Vec<(String, Vec<a3s_code_core::mcp::McpServerConfig>)> = {
            let sessions = self.sessions.lock().await;
            sessions
                .iter()
                .map(|(key, live)| (key.clone(), live.mcp_servers.clone()))
                .collect()
        };
        let desired = union_mcp_desired(baseline, &live, for_session, servers)
            .map_err(|conflict| acp::Error::invalid_params().data(conflict))?;
        agent
            .sync_global_mcp_servers(desired)
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))
    }
}

/// The desired global set: baseline ∪ every live session's servers,
/// skipping the session being re-modeled (its new set merges in via
/// `add` instead). With no skip and no additions this is exactly the
/// session-close resync: baseline ∪ the remaining sessions.
///
/// Same-name merges follow the ORIGIN: within one origin (the baseline
/// or one session) later-wins is the update rule, and two origins
/// holding the SAME definition dedupe to one entry — but a name held by
/// one origin under a DIFFERENT definition than another origin's is a
/// cross-session takeover of that name and is REFUSED, naming both
/// origins. The `mcp__<server>__<tool>` prefix routes by name, so
/// silently re-pointing it would redirect another live session's tool
/// source.
pub(super) fn union_mcp_desired(
    baseline: Vec<a3s_code_core::mcp::McpServerConfig>,
    live: &[(String, Vec<a3s_code_core::mcp::McpServerConfig>)],
    skip: Option<&str>,
    add: &[acp::McpServer],
) -> Result<Vec<a3s_code_core::mcp::McpServerConfig>, String> {
    let mut desired: Vec<(a3s_code_core::mcp::McpServerConfig, &str)> = baseline
        .into_iter()
        .map(|server| (server, "the configuration baseline"))
        .collect();
    for (key, servers) in live {
        if skip == Some(key.as_str()) {
            continue;
        }
        for server in servers {
            merge_with_origin(&mut desired, server.clone(), key)?;
        }
    }
    let add_origin = skip.unwrap_or("the new session");
    for server in session_mcp_configs(add) {
        merge_with_origin(&mut desired, server, add_origin)?;
    }
    Ok(desired.into_iter().map(|(server, _)| server).collect())
}

/// Merge one server into the desired set by name, tracking which origin
/// holds the name: a same-origin collision is that origin updating
/// itself (later wins), a same-definition collision across origins
/// dedupes, and a different-definition collision across origins refuses.
pub(super) fn merge_with_origin<'a>(
    desired: &mut Vec<(a3s_code_core::mcp::McpServerConfig, &'a str)>,
    server: a3s_code_core::mcp::McpServerConfig,
    origin: &'a str,
) -> Result<(), String> {
    match desired
        .iter()
        .position(|(entry, _)| entry.name == server.name)
    {
        Some(index) => {
            let (existing, held_by) = &desired[index];
            if held_by == &origin || same_server_definition(existing, &server) {
                desired[index] = (server, origin);
                Ok(())
            } else {
                Err(format!(
                    "mcp server name `{}` is already held by {held_by:?} with a different \
                     definition; {origin:?} must use another name (the mcp__{0}__ tool \
                     prefix routes by name)",
                    server.name,
                ))
            }
        }
        None => {
            desired.push((server, origin));
            Ok(())
        }
    }
}

/// Canonical-definition equality: `McpServerConfig` carries a custom
/// `Deserialize` and no `PartialEq`, so definitions compare through
/// their serialization (serde_json maps are ordered — deterministic for
/// this plain-data type).
pub(super) fn same_server_definition(
    left: &a3s_code_core::mcp::McpServerConfig,
    right: &a3s_code_core::mcp::McpServerConfig,
) -> bool {
    match (serde_json::to_value(left), serde_json::to_value(right)) {
        (Ok(left), Ok(right)) => left == right,
        // Serialize cannot fail for this type; treat a failure as
        // inequality rather than silently merging unknowns.
        _ => false,
    }
}

pub(super) fn session_mcp_configs(
    servers: &[acp::McpServer],
) -> Vec<a3s_code_core::mcp::McpServerConfig> {
    servers.iter().filter_map(acp_mcp_to_config).collect()
}

pub(super) fn acp_mcp_to_config(
    server: &acp::McpServer,
) -> Option<a3s_code_core::mcp::McpServerConfig> {
    use a3s_code_core::mcp::{McpServerConfig, McpTransportConfig};

    let hosted = |name: &str,
                  url: &str,
                  headers: &[acp::HttpHeader],
                  sse: bool|
     -> Option<a3s_code_core::mcp::McpServerConfig> {
        if name.is_empty() || url.is_empty() {
            return None;
        }
        let headers = headers
            .iter()
            .filter(|header| !header.name.is_empty())
            .map(|header| (header.name.clone(), header.value.clone()))
            .collect();
        let transport = if sse {
            McpTransportConfig::Http {
                url: url.to_string(),
                headers,
            }
        } else {
            McpTransportConfig::StreamableHttp {
                url: url.to_string(),
                headers,
            }
        };
        Some(McpServerConfig {
            name: name.to_string(),
            transport,
            enabled: true,
            env: HashMap::new(),
            oauth: None,
            tool_timeout_secs: 60,
        })
    };

    match server {
        acp::McpServer::Stdio(stdio) => {
            let command = stdio.command.to_string_lossy();
            if stdio.name.is_empty() || command.is_empty() {
                return None;
            }
            let env = stdio
                .env
                .iter()
                .filter(|variable| !variable.name.is_empty())
                .map(|variable| (variable.name.clone(), variable.value.clone()))
                .collect();
            Some(McpServerConfig {
                name: stdio.name.clone(),
                transport: McpTransportConfig::Stdio {
                    command: command.into_owned(),
                    args: stdio.args.clone(),
                },
                enabled: true,
                env,
                oauth: None,
                tool_timeout_secs: 60,
            })
        }
        acp::McpServer::Http(http) => hosted(&http.name, &http.url, &http.headers, false),
        acp::McpServer::Sse(sse) => hosted(&sse.name, &sse.url, &sse.headers, true),
        _ => None,
    }
}
