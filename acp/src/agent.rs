//! `acp::Agent` backed by a3s-code-core fact-log sessions.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use a3s_code_core::{Agent, AgentEvent, AgentSession, SessionOptions};
use agent_client_protocol as acp;
use agent_client_protocol::Client;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio::task::JoinHandle;

use crate::config::LaunchConfig;

mod client_fs;
mod commands;
mod permission_ask;
#[cfg(test)]
use permission_ask::tool_permission_request;
mod resume;
mod session_catalog;
mod unattended_turn;
mod workspace_scope;

/// Prompts that may wait behind the active turn before the newest one is
/// rejected. Half the run-control inbox cap (64): each queued entry is a
/// full user turn, and a client this far ahead is malfunctioning.
const MAX_QUEUED_PROMPTS: usize = 32;
const MODEL_CONFIG_ID: &str = "model";

/// A reserved spot in a [`TurnQueue`]'s waiting count. Dropping it (turn
/// granted, future cancelled, or task panicked) gives the slot back — a
/// dropped prompt future must not permanently shrink the queue.
struct WaitingSlot<'a> {
    waiting: &'a std::sync::atomic::AtomicUsize,
}

impl Drop for WaitingSlot<'_> {
    fn drop(&mut self) {
        self.waiting
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

/// Serializes turns: a prompt arriving mid-turn queues here instead of
/// failing with "already has an active operation" (the pager's send-now
/// and queue reissue both rely on the agent accepting and serializing).
/// The wait is bounded — without a cap, a client that piles prompts
/// without waiting grows queued turns (and their held prompt state)
/// without end.
#[derive(Default)]
struct TurnQueue {
    lock: Arc<tokio::sync::Mutex<()>>,
    waiting: std::sync::atomic::AtomicUsize,
}

impl TurnQueue {
    fn new() -> Self {
        Self::default()
    }

    /// Reserve a waiting slot, rejecting the prompt once MAX_QUEUED_PROMPTS
    /// are already queued behind the active turn.
    fn reserve(&self) -> Option<WaitingSlot<'_>> {
        use std::sync::atomic::Ordering;
        let now = self.waiting.fetch_add(1, Ordering::AcqRel) + 1;
        if now > MAX_QUEUED_PROMPTS {
            self.waiting.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(WaitingSlot {
            waiting: &self.waiting,
        })
    }

    /// Queue for the turn lock. The slot is held while waiting (so a
    /// cancelled future returns it) and released the moment the permit is
    /// granted — the running turn is not a waiter.
    async fn acquire_owned(self: &Arc<Self>) -> Option<OwnedMutexGuard<()>> {
        let slot = self.reserve()?;
        let permit = Arc::clone(&self.lock).lock_owned().await;
        drop(slot);
        Some(permit)
    }
}

struct LiveSession {
    session: Arc<AgentSession>,
    workspace: PathBuf,
    model_id: String,
    /// Named next-run posture. Deny rules on this session still win.
    posture: String,
    denies: Vec<String>,
    /// Same deny list `open_local_asset` reads while a turn is in flight.
    asset_denies: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// Standing Studio instructions admitted into the system prompt at build.
    host: crate::host_context::StudioHost,
    /// Plugin executables from `_meta.a3sTools`, reapplied when the session is replaced.
    plugin_tools: Vec<serde_json::Value>,
    /// Admitted assemble list for this process. A later list starts a new session.
    components: Vec<String>,
    /// When the in-flight turn started (WC-NOTIFY1: the completion bell
    /// fires only for runs past the configured floor).
    turn_started: std::sync::Mutex<Option<std::time::Instant>>,
    /// Serializes turns: a prompt arriving mid-turn queues here instead of
    /// failing with "already has an active operation" (the pager's send-now
    /// and queue reissue both rely on the agent accepting and serializing).
    turn_queue: Arc<TurnQueue>,
    /// Session-scoped MCP servers (the `mcp_servers` this session was opened
    /// with). The agent-global manager serves the UNION of all live
    /// sessions' sets, so one session's sync can never delete another's.
    mcp_servers: Vec<a3s_code_core::mcp::McpServerConfig>,
    /// The client's prompt id for the in-flight turn, echoed on every
    /// session notification per the ACP prompt-ack contract.
    active_prompt_id: std::sync::Mutex<Option<String>>,
}

/// Inputs shared by session creation and in-place replacement. Keeping one
/// typed value prevents model changes from dropping a workspace, host or MCP
/// contribution while rebuilding the session.
struct SessionSetup<'a> {
    workspace: PathBuf,
    model_id: &'a str,
    effort: Option<EffortLimits>,
    host: crate::host_context::StudioHost,
    components: &'a [String],
    mcp_servers: Vec<a3s_code_core::mcp::McpServerConfig>,
}

/// Skill roots the interactive session scans. Existing directories only.
/// Configured ACL paths join the workspace when they are relative. The user
/// directory `~/.a3s/skills` is included so `a3s code skills add` is visible
/// without editing `skill_dirs`.
fn interactive_skill_dirs(workspace: &std::path::Path, configured: &[PathBuf]) -> Vec<PathBuf> {
    let (dirs, missing) = interactive_skill_dirs_from(workspace, configured);
    for path in missing {
        tracing::error!(
            path = %path.display(),
            "configured skill directory is missing; skills from this path are not loaded"
        );
    }
    dirs
}

/// Directories a session should WATCH for later installs even though they do
/// not exist yet. The project skills root, `~/.a3s/skills`, and the Agent
/// Skills homes (`~/.agents/skills`, `~/.claude/skills`, `~/.codex/skills`)
/// are included. The first install creates one of these; the next catalog
/// refresh or Skill lookup then loads the package.
fn watched_skill_dirs(workspace: &std::path::Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        workspace.join("skills"),
        workspace.join(".a3s").join("skills"),
    ];
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = std::path::PathBuf::from(home);
        dirs.push(home.join(".a3s").join("skills"));
        for root in [".agents", ".claude", ".codex"] {
            dirs.push(home.join(root).join("skills"));
        }
    }
    dirs
}

fn interactive_skill_dirs_from(
    workspace: &std::path::Path,
    configured: &[PathBuf],
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut dirs = Vec::new();
    let mut missing = Vec::new();
    let project = workspace.join(".a3s").join("skills");
    if project.is_dir() {
        dirs.push(project);
    }
    for dir in configured {
        let path = if dir.is_absolute() {
            dir.clone()
        } else {
            workspace.join(dir)
        };
        if path.is_dir() {
            dirs.push(path);
        } else {
            missing.push(path);
        }
    }
    if let Some(home) = std::env::var_os("HOME").filter(|value| !value.is_empty()) {
        let home = PathBuf::from(home);
        let user = home.join(".a3s").join("skills");
        if user.is_dir() {
            dirs.push(user);
        }
        for root in [".agents", ".claude", ".codex"] {
            let personal = home.join(root).join("skills");
            if personal.is_dir() {
                dirs.push(personal);
            }
        }
    }
    dirs.sort();
    dirs.dedup();
    missing.sort();
    missing.dedup();
    (dirs, missing)
}

/// Snapshot directory for one interactive core session.
///
/// ACL `sessions_dir` wins. Otherwise the store sits beside the pager tree
/// at `<product home>/session-store`.
fn interactive_session_store_dir_from(
    configured: Option<&std::path::Path>,
    a3s_home: Option<&std::ffi::OsStr>,
    os_home: Option<&std::ffi::OsStr>,
) -> PathBuf {
    if let Some(dir) = configured {
        return dir.to_path_buf();
    }
    if let Some(home) = a3s_home.filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join("session-store");
    }
    if let Some(home) = os_home.filter(|value| !value.is_empty()) {
        return PathBuf::from(home).join(".a3s").join("session-store");
    }
    PathBuf::from(".a3s").join("session-store")
}

fn canonical_posture(mode: &str) -> Option<&'static str> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "default" => Some("default"),
        "plan" => Some("plan"),
        "auto" => Some("auto"),
        "yolo" => Some("yolo"),
        _ => None,
    }
}

/// WC-NOTIFY1: whether a completed run owes the configured completion
/// signal. Pure so the floor and the closed set stay pinnable without
/// a terminal.
fn completion_bell_due(
    notifications: &a3s_code_core::config::NotificationsConfig,
    elapsed: std::time::Duration,
) -> bool {
    matches!(
        notifications.run_completion,
        a3s_code_core::config::RunCompletionSignal::Bell
    ) && elapsed.as_secs() >= notifications.min_seconds
}

fn posture_policy(
    posture: &str,
    denies: &[String],
) -> a3s_code_core::permissions::PermissionPolicy {
    let mut policy = a3s_code_core::permissions::policy_for_posture(posture);
    for rule in denies {
        policy = policy.deny(rule);
    }
    // Published catalog reads only. Deny rules above still win. Other tools stay
    // on the posture; this does not switch the session to yolo.
    policy.allow("open_local_asset").allow("search_wiki")
}

fn remember_denies(live: &mut LiveSession) {
    let mut gate = live
        .asset_denies
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *gate = live.denies.clone();
}

/// Restores the session posture if a research turn returns early.
struct ResearchGate<'a> {
    session: &'a AgentSession,
    posture: &'a str,
    denies: &'a [String],
    armed: bool,
}

impl Drop for ResearchGate<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ = self
            .session
            .set_permission_policy(posture_policy(self.posture, self.denies));
    }
}

/// ACP agent that owns live [`AgentSession`] values keyed by ACP session id.
pub struct A3sCodeAgent {
    launch: LaunchConfig,
    /// Set once the stdio `AgentSideConnection` is built (chicken/egg with
    /// ACP ctor). `Rc`-shared so the sampling consent consumer (spawned on
    /// the same LocalSet) reads the live connection.
    client: Rc<RefCell<Option<Rc<acp::AgentSideConnection>>>>,
    core_agent: Mutex<Option<Agent>>,
    /// One bounded filesystem inventory at a time; the blocking worker owns
    /// the permit even if its ACP request future is dropped.
    catalog_queries: Arc<tokio::sync::Semaphore>,
    /// `Arc`-shared with the sampling consent consumer, which resolves a
    /// server's owning session before asking a question.
    sessions: Arc<Mutex<HashMap<String, LiveSession>>>,
    model_id: Mutex<String>,
    /// Last `/effort` selection. Applied on every core session open.
    effort: Mutex<Option<EffortLimits>>,
    /// Durable `/goal` objectives keyed by ACP session id.
    goals: Mutex<HashMap<String, crate::goal::DurableGoal>>,
    /// Monotonic turn generation per session. `cancel` records the generation
    /// that was in flight, so a late cancel cannot mark the next turn.
    turn_generation: Mutex<HashMap<String, u64>>,
    /// Generation `cancel` observed for each session.
    cancelled_generation: Mutex<HashMap<String, u64>>,
    /// Wakes `stream_turn` after `cancel` records the generation.
    /// `notify_waiters` does not store a permit, so the turn also reads the map.
    turn_cancel: std::sync::Arc<tokio::sync::Notify>,
    /// MCP servers loaded from the launch config, captured before a session
    /// list is applied. Session servers replace same-named entries; an empty
    /// session list leaves this baseline in place.
    config_mcp: Mutex<Option<Vec<a3s_code_core::mcp::McpServerConfig>>>,
    plugin_selection: Option<crate::PluginSelectionHost>,
    /// Present after a launcher asks in `session/new` `_meta.a3sStudioDocuments`,
    /// or after the legacy `--studio-documents` process flag.
    documents: RefCell<Option<Arc<crate::document_read::DocumentBridge>>>,
    /// The consent channel the sampling factory's bridge parks asks on;
    /// built at construction, consumed by the task spawned at
    /// `set_client` (a LocalSet context, per the document-read worker
    /// precedent). Without a client the receiver is never polled and
    /// every ask times out refused — fail closed.
    consent_tx:
        RefCell<Option<tokio::sync::mpsc::Sender<crate::sampling_consent::SamplingConsentAsk>>>,
    consent_rx:
        RefCell<Option<tokio::sync::mpsc::Receiver<crate::sampling_consent::SamplingConsentAsk>>>,
    /// `--unattended` only. `None` for every interactive host.
    unattended: Option<crate::unattended::Unattended>,
    /// What the untrusted workspace layer was refused or left pending.
    workspace_admission: a3s_code_core::config::WorkspaceLayerAdmission,
    client_reads_files: std::cell::Cell<bool>,
}

fn model_picker_label(provider_name: &str, display_name: &str) -> String {
    if provider_name == "cc-switch" || provider_name.starts_with("cc-switch-") {
        format!("cc-switch · {display_name}")
    } else if provider_name == "grok" {
        format!("grok · {display_name}")
    } else if provider_name == "cursor" {
        format!("cursor · {display_name}")
    } else {
        format!("{provider_name}/{display_name}")
    }
}

fn model_picker_description(provider_name: &str) -> String {
    if provider_name == "cc-switch" || provider_name.starts_with("cc-switch-") {
        "Imported from local CC Switch".to_string()
    } else if provider_name == "grok" {
        "Logged-in Grok account".to_string()
    } else if provider_name == "cursor" {
        "Logged-in Cursor account".to_string()
    } else {
        format!("From A3S ACL provider {provider_name}")
    }
}

impl A3sCodeAgent {
    pub fn new(launch: LaunchConfig) -> Self {
        let model_id = launch.model_id.clone();
        let (consent_tx, consent_rx) = tokio::sync::mpsc::channel(16);
        Self {
            launch,
            client: Rc::new(RefCell::new(None)),
            core_agent: Mutex::new(None),
            catalog_queries: Arc::new(tokio::sync::Semaphore::new(1)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            model_id: Mutex::new(model_id),
            // a3s-code TUI default is BudgetProfile index 2 (`high`).
            effort: Mutex::new(effort_limits("high")),
            goals: Mutex::new(HashMap::new()),
            turn_generation: Mutex::new(HashMap::new()),
            cancelled_generation: Mutex::new(HashMap::new()),
            turn_cancel: std::sync::Arc::new(tokio::sync::Notify::new()),
            config_mcp: Mutex::new(None),
            plugin_selection: None,
            documents: RefCell::new(None),
            consent_tx: RefCell::new(Some(consent_tx)),
            consent_rx: RefCell::new(Some(consent_rx)),
            unattended: None,
            workspace_admission: Default::default(),
            client_reads_files: std::cell::Cell::new(false),
        }
    }

    /// Report the launch's workspace-layer admission to the client.
    pub fn with_workspace_admission(
        mut self,
        admission: a3s_code_core::config::WorkspaceLayerAdmission,
    ) -> Self {
        self.workspace_admission = admission;
        self
    }

    fn workspace_admission_meta(&self) -> serde_json::Value {
        let admission = &self.workspace_admission;
        serde_json::json!({
            "workspaceMcp": {
                "pending": admission.pending_mcp().collect::<Vec<_>>(),
                "trusted": admission
                    .workspace_mcp
                    .iter()
                    .filter(|server| server.trusted)
                    .map(|server| server.name.as_str())
                    .collect::<Vec<_>>(),
                "trustCommand": "a3s-code-acp --trust-workspace-mcp <id|all>",
            },
            "withheldBlocks": admission.withheld_blocks,
            "droppedDirs": admission.dropped_dirs,
        })
    }

    /// Run every prompt unattended. Only the `--unattended` launch calls this.
    pub fn with_unattended(mut self, unattended: crate::unattended::Unattended) -> Self {
        self.unattended = Some(unattended);
        self
    }

    pub fn with_plugin_selection(mut self, selection: crate::PluginSelectionHost) -> Self {
        self.plugin_selection = Some(selection);
        self
    }

    /// Enable reads through the launcher-owned Studio document host.
    pub fn with_studio_documents(self) -> Self {
        *self.documents.borrow_mut() = Some(crate::document_read::DocumentBridge::new());
        self
    }

    /// `_meta.a3sStudioDocuments: true` opts this process into the document host.
    /// Any other value leaves the bridge unset. A later session on the same
    /// process keeps the bridge the first request created.
    fn admit_studio_documents(&self, meta: Option<&serde_json::Map<String, serde_json::Value>>) {
        let requested = meta
            .and_then(|meta| meta.get("a3sStudioDocuments"))
            .and_then(|value| value.as_bool())
            == Some(true);
        if !requested || self.documents.borrow().is_some() {
            return;
        }
        let bridge = crate::document_read::DocumentBridge::new();
        if let Some(client) = self.client.borrow().as_ref().cloned() {
            bridge.connect(client);
        }
        *self.documents.borrow_mut() = Some(bridge);
    }

    fn document_bridge(&self) -> Option<Arc<crate::document_read::DocumentBridge>> {
        self.documents.borrow().clone()
    }

    /// Stop host RPCs when the owned ACP transport terminates.
    pub fn stop_document_bridge(&self) {
        if let Some(documents) = self.document_bridge() {
            documents.stop();
        }
        self.client.borrow_mut().take();
    }

    fn validate_plugin_selection(
        &self,
        meta: Option<&serde_json::Map<String, serde_json::Value>>,
        servers: &[acp::McpServer],
    ) -> acp::Result<()> {
        let requested = meta.and_then(|meta| meta.get("a3sPluginSelection"));
        match (&self.plugin_selection, requested) {
            (None, None) => Ok(()),
            (Some(host), Some(value)) => {
                let cursor: a3s_use_reconcile::SelectionCursor =
                    serde_json::from_value(value.clone())
                        .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
                if &cursor != host.cursor()
                    || !crate::plugin_tools::entries_from_meta(meta).is_empty()
                    || servers
                        .iter()
                        .filter_map(acp_mcp_to_config)
                        .any(|config| host.contains_mcp_server(&config.name))
                {
                    return Err(acp::Error::invalid_params()
                        .data("plugin selection mismatch or unbound Tool/MCP entries"));
                }
                Ok(())
            }
            _ => Err(acp::Error::invalid_params()
                .data("plugin selection requires a matching native launcher binding")),
        }
    }

    /// Sync the agent-global MCP manager to the UNION of the config baseline
    /// and every live session's server set. Previously the last session's
    /// desired set won alone, so opening a second session with different
    /// `mcp_servers` silently deleted the first session's servers.
    async fn sync_session_mcp_servers(
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

    /// The in-flight prompt id for this notification's session.
    ///
    /// A try-lock miss leaves the update unstamped. Falling through to another
    /// session's id would make the pager drop the update.
    fn prompt_id_for_session(&self, session_id: &acp::SessionId) -> Option<String> {
        let key = session_id.0.as_ref();
        let sessions = self.sessions.try_lock().ok()?;
        let prompt_id = sessions.get(key).and_then(|live| {
            live.active_prompt_id
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
        });
        drop(sessions);
        prompt_id
    }

    pub fn set_client(&self, conn: Rc<acp::AgentSideConnection>) {
        if let Some(documents) = self.document_bridge() {
            documents.connect(conn.clone());
        }
        // The sampling consent consumer rides the same LocalSet this
        // connection setup runs on (the document-read worker
        // precedent). Once only; a second connection reuses it.
        if let Some(rx) = self.consent_rx.borrow_mut().take() {
            tokio::task::spawn_local(serve_sampling_consent(
                rx,
                Rc::clone(&self.client),
                Arc::clone(&self.sessions),
            ));
        }
        *self.client.borrow_mut() = Some(conn);
    }

    fn available_model_infos(&self) -> Vec<acp::ModelInfo> {
        let mut infos = Vec::new();
        for (provider, model) in self.launch.config.list_models() {
            let id = format!("{}/{}", provider.name, model.id);
            let display_name = if model.name.trim().is_empty() {
                model.id.clone()
            } else {
                model.name.clone()
            };
            let name = model_picker_label(&provider.name, &display_name);
            let description = Some(model_picker_description(&provider.name));
            let mut info = acp::ModelInfo::new(acp::ModelId::new(id.clone()), name);
            if let Some(description) = description {
                info = info.description(description);
            }
            info = info.meta(Some(reasoning_effort_catalog_meta(&id)));
            infos.push(info);
        }
        if infos.is_empty() {
            let id = self.launch.model_id.clone();
            let info = acp::ModelInfo::new(acp::ModelId::new(id.clone()), id.clone())
                .meta(Some(reasoning_effort_catalog_meta(&id)));
            infos.push(info);
        }
        infos
    }

    fn model_state_with(
        &self,
        current: &str,
        effort_token: Option<&str>,
    ) -> acp::SessionModelState {
        let mut available = self.available_model_infos();
        let current_id = if available
            .iter()
            .any(|info| info.model_id.0.as_ref() == current)
        {
            current.to_string()
        } else {
            available
                .first()
                .map(|info| info.model_id.0.to_string())
                .unwrap_or_else(|| current.to_string())
        };
        if let Some(token) = effort_token {
            if let Some(info) = available
                .iter_mut()
                .find(|info| info.model_id.0.as_ref() == current_id)
            {
                let mut meta = info.meta.clone().unwrap_or_default();
                meta.insert(
                    "reasoningEffort".into(),
                    serde_json::Value::String(token.to_string()),
                );
                info.meta = Some(meta);
            }
        }
        acp::SessionModelState::new(acp::ModelId::new(current_id), available)
    }

    /// The model picker as a session config option: clients on ACP schemas
    /// without `SessionModelState` (Harbor's Python SDK) select through it.
    fn model_config_options(models: &acp::SessionModelState) -> Vec<acp::SessionConfigOption> {
        let options: Vec<acp::SessionConfigSelectOption> = models
            .available_models
            .iter()
            .map(|info| {
                acp::SessionConfigSelectOption::new(info.model_id.0.to_string(), info.name.clone())
            })
            .collect();
        vec![acp::SessionConfigOption::select(
            MODEL_CONFIG_ID,
            "Model",
            models.current_model_id.0.to_string(),
            options,
        )
        .category(acp::SessionConfigOptionCategory::Model)]
    }

    fn known_model(&self, model_id: &str) -> bool {
        self.launch
            .config
            .list_models()
            .iter()
            .any(|(provider, model)| format!("{}/{}", provider.name, model.id) == model_id)
            || model_id == self.launch.model_id
    }

    async fn ensure_core_agent(&self) -> acp::Result<()> {
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

    /// Build the consent channel (once) and arm the manager's sampling
    /// factory with the composition `Bounded(ApprovalGated(Llm))` — the
    /// consent bridge INSIDE the per-server ceiling, per both types'
    /// docs. The sampling voice is the launch default model. Any arming
    /// failure leaves sampling refused (fail closed); this never fails
    /// the agent itself.
    async fn arm_sampling_consent(&self, agent: &Agent) -> acp::Result<()> {
        let Some(tx) = self.consent_tx.borrow().clone() else {
            // The channel is built at construction; unreachable, but a
            // missing surface refuses rather than arms.
            return Ok(());
        };
        let model_ref = self
            .launch
            .config
            .default_model
            .as_deref()
            .unwrap_or_default()
            .to_string();
        let Some((provider_id, model_id)) = model_ref.split_once('/') else {
            // No default model voice: sampling stays refused.
            return Ok(());
        };
        let Some(llm) = self.launch.config.llm_config(provider_id, model_id) else {
            // No usable client for the default voice: refused.
            return Ok(());
        };
        let llm = a3s_code_core::llm::create_client_with_config(llm);
        let factory: a3s_code_core::mcp::SamplingHandlerFactory =
            std::sync::Arc::new(move |server| sampling_handler_for(server, &llm, &model_ref, &tx));
        agent
            .set_sampling_handler_factory(factory)
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(())
    }

    fn core_session_options(
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
    fn interactive_session_store_dir(configured: Option<&std::path::Path>) -> PathBuf {
        interactive_session_store_dir_from(
            configured,
            std::env::var_os("A3S_HOME").as_deref(),
            std::env::var_os("HOME").as_deref(),
        )
    }

    fn host_session_options(
        session_id: &str,
        setup: &SessionSetup<'_>,
        config: &a3s_code_core::CodeConfig,
        session_store_dir: &std::path::Path,
    ) -> acp::Result<SessionOptions> {
        let contracts = config
            .verification_contracts_for_workspace(&setup.workspace)
            .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
        let guardrail = a3s_code_core::permissions::InteractiveToolGuardrail::default()
            .with_workspace(&setup.workspace);
        let options = Self::core_session_options(session_id, setup.model_id, setup.effort);
        let slots = crate::host_context::merge_studio_host(
            options.prompt_slots.clone().unwrap_or_default(),
            &setup.host,
        );
        let mut options = options
            .with_prompt_slots(slots)
            .with_verification_contracts(contracts)
            .with_permission_checker(Arc::new(guardrail))
            .with_confirmation_policy(a3s_code_core::hitl::ConfirmationPolicy::enabled())
            .with_sandbox_network_mode(config.sandbox_network)
            .with_sandbox_network_allow(config.sandbox_network_allow.clone())
            .with_code_intelligence();
        if let Some(policy) = &setup.host.work_policy {
            options = options.with_work_policy(policy.clone());
        }
        let skill_dirs = interactive_skill_dirs(&setup.workspace, &config.skill_dirs);
        if !skill_dirs.is_empty() {
            options = options.with_skill_dirs(skill_dirs);
        }
        options = options.with_watch_skill_dirs(watched_skill_dirs(&setup.workspace));
        Ok(options.with_file_session_store(session_store_dir))
    }

    fn session_options_for(
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
        Ok(self.with_client_reads(options, session_id, &setup.workspace))
    }

    async fn open_core_session(
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
    fn admit_plugin_tools(
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

    async fn finish_plugin_tools(
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
    async fn replace_core_session(
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
    fn attach_local_assets(
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

    async fn emit(&self, notification: acp::SessionNotification) {
        let client = self.client.borrow().clone();
        if let Some(client) = client {
            let mut notification = notification;
            // Echo the in-flight turn's client prompt id on every
            // notification (the ACP prompt-ack contract: the pager disarms
            // its ack watch on the first update naming the prompt).
            if notification.meta.is_none() {
                let prompt_id = self.prompt_id_for_session(&notification.session_id);
                if let Some(prompt_id) = prompt_id {
                    let mut meta = acp::Meta::new();
                    meta.insert("promptId".to_string(), serde_json::json!(prompt_id));
                    notification.meta = Some(meta);
                }
            }
            let _ = client.session_notification(notification).await;
        }
    }

    async fn emit_run_observation(&self, observation: Option<crate::run_observation::Observation>) {
        let Some(observation) = observation else {
            return;
        };
        let Some(client) = self.client.borrow().clone() else {
            return;
        };
        let params = match serde_json::value::to_raw_value(&observation) {
            Ok(params) => params,
            Err(error) => {
                tracing::warn!(%error, "Run observation serialization refused");
                return;
            }
        };
        if let Err(error) = client
            .ext_notification(acp::ExtNotification::new(
                crate::run_observation::METHOD,
                params.into(),
            ))
            .await
        {
            tracing::warn!(%error, "Run observation transport unavailable");
        }
    }

    async fn close_run_observation(&self, state: &mut TurnState) {
        let observation = match state.observer.as_mut() {
            Some(observer) => observer.closed().await,
            None => None,
        };
        self.emit_run_observation(observation).await;
    }

    async fn emit_text(&self, session_id: &acp::SessionId, text: &str) {
        if text.is_empty() {
            return;
        }
        tracing::info!(target: "perf_probe", len = text.len(), preview = %text.chars().take(60).collect::<String>(), "chunk_emit");
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new(text.to_string()),
            ))),
        ))
        .await;
    }

    async fn emit_tool_call(
        &self,
        session_id: &acp::SessionId,
        id: &str,
        name: &str,
        args: Option<&serde_json::Value>,
    ) {
        let mut call = acp::ToolCall::new(acp::ToolCallId::new(id.to_string()), tool_title(name))
            .kind(tool_kind_for(name))
            .status(acp::ToolCallStatus::InProgress);
        if let Some(raw_input) = args.and_then(|args| prepared_raw_input(name, args)) {
            call = call.raw_input(raw_input);
        }
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::ToolCall(call),
        ))
        .await;
    }

    /// Attach a3s tool arguments once execution starts so the pager can render
    /// the Execute / Read / Edit / Search / Fetch card instead of a generic row.
    async fn emit_tool_call_started(
        &self,
        session_id: &acp::SessionId,
        id: &str,
        name: &str,
        args: &serde_json::Value,
    ) {
        let mut fields = acp::ToolCallUpdateFields::new()
            .title(tool_title(name))
            .kind(tool_kind_for(name))
            .status(acp::ToolCallStatus::InProgress);
        if let Some(raw_input) = prepared_raw_input(name, args) {
            fields = fields.raw_input(raw_input);
        }
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                acp::ToolCallId::new(id.to_string()),
                fields,
            )),
        ))
        .await;
    }

    async fn emit_tool_call_completed(
        &self,
        session_id: &acp::SessionId,
        id: &str,
        name: &str,
        args: Option<&serde_json::Value>,
        output: &str,
        exit_code: i32,
    ) {
        let status = if exit_code == 0 {
            acp::ToolCallStatus::Completed
        } else {
            acp::ToolCallStatus::Failed
        };
        let mut fields = acp::ToolCallUpdateFields::new()
            .title(tool_title(name))
            .kind(tool_kind_for(name))
            .status(status)
            .content(tool_call_content(name, args, output));
        if let Some(raw_input) = args.and_then(|args| prepared_raw_input(name, args)) {
            fields = fields.raw_input(raw_input);
        }
        if let Some(raw_output) = tool_raw_output(name, args, output, exit_code) {
            fields = fields.raw_output(raw_output);
        }
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                acp::ToolCallId::new(id.to_string()),
                fields,
            )),
        ))
        .await;
    }

    async fn emit_tool_call_output_delta(
        &self,
        session_id: &acp::SessionId,
        id: &str,
        name: &str,
        delta: &str,
    ) {
        if delta.is_empty() {
            return;
        }
        let preview: String = delta.chars().take(4_000).collect();
        let mut fields = acp::ToolCallUpdateFields::new()
            .title(tool_title(name))
            .kind(tool_kind_for(name))
            .status(acp::ToolCallStatus::InProgress)
            .content(vec![acp::ToolCallContent::from(acp::ContentBlock::Text(
                acp::TextContent::new(preview.clone()),
            ))]);
        if matches!(name, "bash" | "shell" | "execute") {
            fields = fields.raw_output(streaming_bash_delta(&preview));
        }
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                acp::ToolCallId::new(id.to_string()),
                fields,
            )),
        ))
        .await;
    }

    async fn emit_thought(&self, session_id: &acp::SessionId, text: &str) {
        if text.is_empty() {
            return;
        }
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
                acp::TextContent::new(text.to_string()),
            ))),
        ))
        .await;
    }

    async fn emit_plan(
        &self,
        session_id: &acp::SessionId,
        tasks: &[a3s_code_core::planning::Task],
    ) {
        if tasks.is_empty() {
            return;
        }
        let entries = tasks.iter().map(task_to_plan_entry).collect::<Vec<_>>();
        self.emit(acp::SessionNotification::new(
            session_id.clone(),
            acp::SessionUpdate::Plan(acp::Plan::new(entries)),
        ))
        .await;
    }

    /// Forward xAI-shaped session updates the pager already understands
    /// (`SubagentSpawned` / `SubagentProgress` / `SubagentFinished`).
    async fn emit_xai_session_update(
        &self,
        session_id: &acp::SessionId,
        update: serde_json::Value,
    ) {
        let payload = serde_json::json!({
            "sessionId": session_id.0.as_ref(),
            "update": update,
        });
        let Ok(raw) = serde_json::value::to_raw_value(&payload) else {
            return;
        };
        let client = self.client.borrow().clone();
        let Some(client) = client else {
            return;
        };
        let _ = client
            .ext_notification(acp::ExtNotification::new(
                "x.ai/session_notification",
                raw.into(),
            ))
            .await;
    }

    /// Notify the client how the run closed at the completion gate, on a
    /// dedicated method so it needs no ACP enum variant. Emitted before the
    /// prompt response resolves (same FIFO wire ⇒ it precedes the marker).
    async fn emit_turn_outcome(&self, session_id: &acp::SessionId, outcome: serde_json::Value) {
        let payload = serde_json::json!({
            "sessionId": session_id.0.as_ref(),
            "outcome": outcome,
        });
        let Ok(raw) = serde_json::value::to_raw_value(&payload) else {
            return;
        };
        let client = self.client.borrow().clone();
        let Some(client) = client else {
            return;
        };
        let _ = client
            .ext_notification(acp::ExtNotification::new("x.ai/turn/outcome", raw.into()))
            .await;
    }

    async fn emit_subagent_start(
        &self,
        session_id: &acp::SessionId,
        task_id: &str,
        child_session_id: &str,
        parent_session_id: &str,
        agent: &str,
        description: &str,
    ) {
        self.emit_xai_session_update(
            session_id,
            serde_json::json!({
                "sessionUpdate": "subagent_spawned",
                "subagent_id": task_id,
                "parent_session_id": parent_session_id,
                "child_session_id": child_session_id,
                "subagent_type": agent,
                "description": description,
            }),
        )
        .await;
    }

    #[allow(clippy::too_many_arguments)] // event surface arity is the ACP wire shape
    async fn emit_subagent_progress(
        &self,
        session_id: &acp::SessionId,
        task_id: &str,
        child_session_id: &str,
        status: &str,
        window_tokens: u64,
        metadata: &serde_json::Value,
        started_ms: u64,
        progress: &ChildProgress,
    ) {
        let tokens_used = progress_tokens_used(metadata);
        let (context_window_tokens, context_usage_pct) = context_usage(window_tokens, tokens_used);
        self.emit_xai_session_update(
            session_id,
            serde_json::json!({
                "sessionUpdate": "subagent_progress",
                "subagent_id": task_id,
                "parent_session_id": session_id.0.as_ref(),
                "child_session_id": child_session_id,
                "duration_ms": progress_duration_ms(started_ms, unix_now_ms()),
                "turn_count": progress.turn_count,
                "tool_call_count": progress.tool_call_count,
                "tokens_used": tokens_used,
                "context_window_tokens": context_window_tokens,
                "context_usage_pct": context_usage_pct,
                "tools_used": progress.tools_used,
                "error_count": progress.error_count,
            }),
        )
        .await;
        // Surface the progress text as thought so the TUI still shows movement
        // when the typed SubagentProgress payload has no message field.
        if !status.is_empty() {
            self.emit_thought(session_id, &format!("[{task_id}] {status}\n"))
                .await;
        }
    }

    #[allow(clippy::too_many_arguments)] // event surface arity is the ACP wire shape
    async fn emit_subagent_end(
        &self,
        session_id: &acp::SessionId,
        task_id: &str,
        child_session_id: &str,
        success: bool,
        output: &str,
        started_ms: u64,
        finished_ms: u64,
        progress: &ChildProgress,
    ) {
        let duration_ms = finished_ms.saturating_sub(started_ms);
        let update = subagent_finished_update(
            task_id,
            child_session_id,
            success,
            output,
            duration_ms,
            progress,
        );
        self.emit_xai_session_update(session_id, update).await;
    }

    /// Ask the ACP client (pager) whether a tool may run. Fail closed when no client is wired.
    /// A pager allow is a confirmation fact. Core still projects the admitted policy, and a deny is not executed.
    async fn request_tool_permission(
        &self,
        session_id: &acp::SessionId,
        tool_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
        workspace: &std::path::Path,
    ) -> bool {
        // The unattended launch is the operator's standing approval. It must
        // not learn a prefix into the graded workspace either.
        if self.unattended.is_some() {
            return true;
        }
        // A rule the user chose "don't ask again" for in this workspace
        // skips the round-trip; see approval_learning for what can match.
        if crate::approval_learning::is_previously_approved(workspace, tool_name, args) {
            tracing::debug!(tool = tool_name, "tool call approved by a learned rule");
            return true;
        }
        let request = permission_ask::tool_permission_ask(
            session_id,
            tool_id,
            tool_name,
            args,
            Some(workspace),
        );
        let client = self.client.borrow().clone();
        let Some(client) = client else {
            return false;
        };
        let ask = client.request_permission(request);
        let response = match tokio::time::timeout(ask_deadline(), ask).await {
            Ok(result) => result,
            Err(_elapsed) => {
                // Fail closed: an unanswered ask denies the call instead of
                // parking the turn forever. The bell rang; nobody came.
                tracing::warn!(
                    session = %session_id.0,
                    tool = tool_name,
                    "permission ask timed out unanswered; denying"
                );
                return false;
            }
        };
        match response {
            Ok(response) => {
                let approved = permission_response_approved(&response);
                if approved && permission_response_always(&response) && tool_name == "bash" {
                    let command = args
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default();
                    match crate::approval_learning::record_always_allowed_bash(workspace, command) {
                        Ok(Some(rule)) => tracing::info!(
                            rule = %rule,
                            "learned a bash rule for this workspace"
                        ),
                        Ok(None) => {}
                        Err(error) => tracing::warn!(
                            %error,
                            "could not persist the allow-always rule; it applies to this call only"
                        ),
                    }
                }
                approved
            }
            Err(_) => false,
        }
    }

    /// Ask the same ACP permission channel for a structured answer.
    ///
    /// Options use `RejectOnce` so a YOLO `AllowOnce` drain cannot answer.
    /// Free text with no options stays unanswered; the run remains parked.
    async fn request_question_answer(
        &self,
        session_id: &acp::SessionId,
        question_id: &str,
        question: &str,
        options: &[String],
        allow_free_text: bool,
    ) -> Option<String> {
        let request = question_permission_request(
            session_id,
            question_id,
            question,
            options,
            allow_free_text,
        )?;
        let client = self.client.borrow().clone();
        let client = client?;
        let ask = client.request_permission(request);
        let response = match tokio::time::timeout(ask_deadline(), ask).await {
            Ok(result) => result.ok()?,
            Err(_elapsed) => {
                // Unanswered: the run keeps its existing unanswered-question
                // behavior instead of parking on the wire forever.
                tracing::warn!(
                    session = %session_id.0,
                    question_id = %question_id,
                    "question ask timed out unanswered"
                );
                return None;
            }
        };
        // A typed free-text answer rides the followup meta and outranks the
        // choice-row mapping: the selected row only names how the dialog was
        // dismissed, not what the user answered.
        if let Some(text) = followup_message(&response) {
            return Some(text.to_string());
        }
        answer_from_permission_outcome(&response.outcome, options)
    }

    fn prompt_text(prompt: &[acp::ContentBlock]) -> String {
        prompt
            .iter()
            .filter_map(|block| match block {
                acp::ContentBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn handle_event(
        &self,
        session_id: &acp::SessionId,
        session: &AgentSession,
        state: &mut TurnState,
        event: AgentEvent,
    ) {
        let observation = state
            .observer
            .as_mut()
            .and_then(|observer| observer.event(&event));
        self.emit_run_observation(observation).await;
        match event {
            AgentEvent::TextDelta { text } => {
                if !text.is_empty() {
                    state.saw_text = true;
                }
                state.delta_text.push_str(&text);
                self.emit_text(session_id, &text).await;
            }
            AgentEvent::ReasoningDelta { text } => {
                self.emit_thought(session_id, &text).await;
            }
            AgentEvent::ToolStart { .. } => {
                // Streaming and the fact runner both announce a call before its
                // arguments exist. Painting that announcement opens a second
                // Run row for one execution, and it also paints a twin that
                // the same-response collapse never runs.
            }
            AgentEvent::ToolExecutionStart {
                id,
                name,
                args: tool_args,
            } => match note_tool_row(&mut state.painted_tools, &id) {
                ToolRowPaint::Open => {
                    self.emit_tool_call(session_id, &id, &name, Some(&tool_args))
                        .await;
                }
                ToolRowPaint::Update => {
                    self.emit_tool_call_started(session_id, &id, &name, &tool_args)
                        .await;
                }
            },
            AgentEvent::ToolOutputDelta { id, name, delta } => {
                self.emit_tool_call_output_delta(session_id, &id, &name, &delta)
                    .await;
            }
            AgentEvent::ToolEnd {
                id,
                name,
                args: tool_args,
                output,
                exit_code,
                metadata,
                ..
            } => {
                let same_response_duplicate = metadata
                    .as_ref()
                    .and_then(|value| value.get("guard"))
                    .and_then(|value| value.as_str())
                    == Some("same_response_duplicate");
                if state.painted_tools.contains(&id) || !same_response_duplicate {
                    if state.painted_tools.insert(id.clone()) {
                        self.emit_tool_call(session_id, &id, &name, tool_args.as_ref())
                            .await;
                    }
                    self.emit_tool_call_completed(
                        session_id,
                        &id,
                        &name,
                        tool_args.as_ref(),
                        &output,
                        exit_code,
                    )
                    .await;
                }
            }
            AgentEvent::ConfirmationRequired {
                tool_id,
                tool_name,
                args: tool_args,
                ..
            } => {
                // Yolo still parks bash, then auto-approves. That announcement
                // is the same id as ToolExecutionStart. Record it here or the
                // pager opens a second Run row for one fact.
                match note_tool_row(&mut state.painted_tools, &tool_id) {
                    ToolRowPaint::Open => {
                        self.emit_tool_call(session_id, &tool_id, &tool_name, Some(&tool_args))
                            .await;
                    }
                    ToolRowPaint::Update => {
                        self.emit_tool_call_started(session_id, &tool_id, &tool_name, &tool_args)
                            .await;
                    }
                }
                let approved = self
                    .request_tool_permission(
                        session_id,
                        &tool_id,
                        &tool_name,
                        &tool_args,
                        session.workspace(),
                    )
                    .await;
                let reason = if approved {
                    Some("approved via ACP request_permission".into())
                } else {
                    Some("rejected via ACP request_permission".into())
                };
                if let Err(error) = session.confirm_tool_use(&tool_id, approved, reason).await {
                    self.emit_text(
                        session_id,
                        &format!("confirm {tool_name} failed: {error}\n"),
                    )
                    .await;
                }
            }
            AgentEvent::TaskUpdated { tasks, .. } => {
                self.emit_plan(session_id, &tasks).await;
            }
            AgentEvent::PlanningEnd { plan, .. } => {
                self.emit_plan(session_id, &plan.steps).await;
            }
            AgentEvent::SubagentStart {
                task_id,
                session_id: child_session_id,
                parent_session_id,
                agent,
                description,
                started_ms,
            } => {
                state
                    .subagent_started_ms
                    .insert(task_id.clone(), started_ms);
                self.emit_subagent_start(
                    session_id,
                    &task_id,
                    &child_session_id,
                    &parent_session_id,
                    &agent,
                    &description,
                )
                .await;
            }
            AgentEvent::SubagentProgress {
                task_id,
                session_id: child_session_id,
                status,
                metadata,
            } => {
                let started_ms = state
                    .subagent_started_ms
                    .get(&task_id)
                    .copied()
                    .unwrap_or(0);
                let progress = state.child_progress.entry(task_id.clone()).or_default();
                note_child_progress(progress, &status, &metadata);
                self.emit_subagent_progress(
                    session_id,
                    &task_id,
                    &child_session_id,
                    &status,
                    session.context_window_tokens() as u64,
                    &metadata,
                    started_ms,
                    progress,
                )
                .await;
            }
            AgentEvent::SubagentEnd {
                task_id,
                session_id: child_session_id,
                output,
                success,
                finished_ms,
                ..
            } => {
                let started_ms = state
                    .subagent_started_ms
                    .remove(&task_id)
                    .unwrap_or(finished_ms);
                let progress = state.child_progress.remove(&task_id).unwrap_or_default();
                self.emit_subagent_end(
                    session_id,
                    &task_id,
                    &child_session_id,
                    success,
                    &output,
                    started_ms,
                    finished_ms,
                    &progress,
                )
                .await;
            }
            AgentEvent::End {
                text,
                verification_summary,
                completion_terminal,
                forced_stop,
                ..
            } => {
                state.end_text = Some(text.clone());
                state.end_outcome = Some(turn_outcome_payload(
                    &completion_terminal,
                    &verification_summary,
                    forced_stop.as_ref(),
                ));
                if should_emit_end_text(state.saw_text, &text) {
                    self.emit_text(session_id, &text).await;
                }
            }
            AgentEvent::Error { message } => {
                state.turn_failure = Some(message);
            }
            AgentEvent::ModelUsageBound { snapshot } => {
                let used = snapshot.reported_prompt_tokens as u64;
                let size = session.context_window_tokens() as u64;
                if used > 0 && size > 0 {
                    self.emit(acp::SessionNotification::new(
                        session_id.clone(),
                        acp::SessionUpdate::UsageUpdate(acp::UsageUpdate::new(used, size)),
                    ))
                    .await;
                }
            }
            AgentEvent::AutoCompact {
                phase,
                tokens_used,
                context_window,
                tokens_after,
            } => {
                let (_, percentage) = context_usage(context_window, tokens_used);
                let update = if phase == "started" {
                    serde_json::json!({
                        "sessionUpdate": "auto_compact_started",
                        "tokens_used": tokens_used,
                        "context_window": context_window,
                        "percentage": percentage,
                        "reason": "threshold",
                    })
                } else {
                    serde_json::json!({
                        "sessionUpdate": "auto_compact_completed",
                        "tokens_before": tokens_used,
                        "tokens_after": tokens_after,
                    })
                };
                self.emit_xai_session_update(session_id, update).await;
            }
            AgentEvent::PermissionDenied {
                tool_id,
                tool_name,
                args: tool_args,
                reason,
            } => {
                self.emit_tool_call(session_id, &tool_id, &tool_name, Some(&tool_args))
                    .await;
                self.emit_tool_call_completed(
                    session_id,
                    &tool_id,
                    &tool_name,
                    Some(&tool_args),
                    &reason,
                    1,
                )
                .await;
            }
            AgentEvent::UserQuestion {
                question_id,
                question,
                options,
                allow_free_text,
                form,
            } => {
                if !claim_question_prompt(&mut state.prompted_questions, &question_id) {
                    return;
                }
                if self.unattended.is_some() {
                    let answer = match form {
                        Some(_) => serde_json::Value::Null.to_string(),
                        None => crate::unattended::QUESTION_ANSWER.to_string(),
                    };
                    let _ = a3s_code_core::ask_user::answer(&question_id, &answer);
                    return;
                }
                if let Some(form) = form.as_deref() {
                    // Closed-catalog form: the client renders it and answers
                    // with the submitted values JSON.
                    let client = self.client.borrow().clone();
                    let values = match client {
                        Some(client) => {
                            crate::form_request::request_form(
                                &client,
                                &question_id,
                                &question,
                                form,
                            )
                            .await
                        }
                        None => None,
                    };
                    let answer = values.unwrap_or(serde_json::Value::Null);
                    let _ = a3s_code_core::ask_user::answer(&question_id, &answer.to_string());
                    return;
                }
                if !question_has_choices(&options) {
                    self.emit_text(
                        session_id,
                        &format!("\nquestion {question_id}: {question}\n"),
                    )
                    .await;
                }
                if let Some(text) = self
                    .request_question_answer(
                        session_id,
                        &question_id,
                        &question,
                        &options,
                        allow_free_text,
                    )
                    .await
                {
                    let _ = a3s_code_core::ask_user::answer(&question_id, &text);
                }
            }
            AgentEvent::GoalAchieved {
                goal,
                total_steps,
                duration_ms,
            } => {
                state.achieved_goal = Some((goal, total_steps, duration_ms));
            }
            _ => {}
        }
    }

    /// Restore planning after a session rebuild while a goal is still active.
    async fn reapply_active_goal(&self, session_key: &str) {
        let active = self
            .goals
            .lock()
            .await
            .get(session_key)
            .map(|goal| !goal.paused);
        let Some(active) = active else {
            return;
        };
        let session = self
            .sessions
            .lock()
            .await
            .get(session_key)
            .map(|live| Arc::clone(&live.session));
        let Some(session) = session else {
            return;
        };
        match session.goal_definition() {
            Ok(None) => {}
            source => {
                self.goals.lock().await.remove(session_key);
                if let Err(error) = source {
                    tracing::error!(%error, "Goal source unavailable; legacy projection disabled");
                }
                return;
            }
        }
        let mode = if active {
            a3s_code_core::PlanningMode::Enabled
        } else {
            a3s_code_core::PlanningMode::Disabled
        };
        let _ = session.set_planning_mode(mode);
    }

    async fn handle_goal(
        &self,
        session_id: &acp::SessionId,
        session: &AgentSession,
        op: crate::goal::GoalOp,
        generation: u64,
        turn_permit: OwnedMutexGuard<()>,
        observation: Option<&crate::run_observation::Binding>,
    ) -> acp::Result<acp::PromptResponse> {
        let key = session_id.0.to_string();
        let invalid = |error: anyhow::Error| acp::Error::invalid_params().data(error.to_string());
        if let Some(bound) = crate::goal::apply_bound_goal(session, &op)
            .await
            .map_err(invalid)?
        {
            self.goals.lock().await.remove(&key);
            self.emit_text(session_id, &bound.reply).await;
            let command = match crate::goal::bound_tick_command(session).map_err(invalid)? {
                Some(command) if bound.tick => command,
                _ => {
                    drop(turn_permit);
                    return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
                }
            };
            let (head, run_id, response) = match session
                .stream_goal_tick(&command)
                .await
                .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?
            {
                a3s_code_core::GoalTickStream::Decided(decision) => {
                    drop(turn_permit);
                    let head = format!("Durable goal tick: {}", decision.as_str());
                    (
                        Some(head),
                        None,
                        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)),
                    )
                }
                a3s_code_core::GoalTickStream::Replayed(snapshot) => {
                    drop(turn_permit);
                    let ok = Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
                    (None, Some(snapshot.id), ok)
                }
                a3s_code_core::GoalTickStream::Started(stream) => {
                    self.emit_text(session_id, "\n\n").await;
                    let run_id = stream.run.as_ref().map(|run| run.id().to_string());
                    let response = self
                        .drain_turn(
                            session_id,
                            session,
                            stream,
                            None,
                            generation,
                            turn_permit,
                            observation,
                        )
                        .await;
                    (None, run_id, response)
                }
            };
            let report = crate::goal::bound_slice_report(session, run_id.as_deref())
                .await
                .map_err(invalid)?;
            let text = head
                .into_iter()
                .chain([report])
                .collect::<Vec<_>>()
                .join("\n\n");
            self.emit_text(session_id, &format!("\n\n{text}")).await;
            return response;
        }
        let applied = crate::goal::apply_session_goal(session, op)
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        {
            let mut goals = self.goals.lock().await;
            if let Some(goal) = applied.next.clone() {
                goals.insert(key.clone(), goal);
            } else {
                goals.remove(&key);
            }
        }
        if let Some(reply) = applied.reply {
            self.emit_text(session_id, &reply).await;
        }
        if let Some(model_prompt) = applied.model_prompt {
            return self
                .stream_turn(
                    session_id,
                    session,
                    &model_prompt,
                    generation,
                    turn_permit,
                    observation,
                )
                .await;
        }
        drop(turn_permit);
        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
    }

    async fn stream_turn(
        &self,
        session_id: &acp::SessionId,
        session: &AgentSession,
        prompt: &str,
        generation: u64,
        turn_permit: OwnedMutexGuard<()>,
        observation: Option<&crate::run_observation::Binding>,
    ) -> acp::Result<acp::PromptResponse> {
        let governing_goal = self.goals.lock().await.get(session_id.0.as_ref()).cloned();
        let stream = match session.stream_with_run(prompt, None).await {
            Ok(parts) => parts,
            Err(error) => {
                drop(turn_permit);
                return Err(acp::Error::internal_error().data(error.to_string()));
            }
        };
        self.drain_turn(
            session_id,
            session,
            stream,
            governing_goal,
            generation,
            turn_permit,
            observation,
        )
        .await
    }

    /// Render one admitted Run's events, answer its confirmations and honour
    /// `cancel`, exactly as for a prompt turn.
    #[allow(clippy::too_many_arguments)]
    async fn drain_turn(
        &self,
        session_id: &acp::SessionId,
        session: &AgentSession,
        stream: a3s_code_core::AgentRunStream,
        governing_goal: Option<crate::goal::DurableGoal>,
        generation: u64,
        turn_permit: OwnedMutexGuard<()>,
        observation: Option<&crate::run_observation::Binding>,
    ) -> acp::Result<acp::PromptResponse> {
        let mut observer = stream.run.and_then(|run| {
            observation.map(|binding| {
                crate::run_observation::Observer::new(run, session_id.0.to_string(), binding)
            })
        });
        self.emit_run_observation(observer.as_mut().and_then(|observer| observer.opened()))
            .await;
        let mut rx = stream.events;
        let join = stream.worker;

        let mut state = TurnState {
            observer,
            turn_failure: None,
            saw_text: false,
            delta_text: String::new(),
            end_text: None,
            end_outcome: None,
            achieved_goal: None,
            subagent_started_ms: HashMap::new(),
            child_progress: HashMap::new(),
            prompted_questions: HashSet::new(),
            painted_tools: HashSet::new(),
        };
        let session_key = session_id.0.to_string();
        let mut turn_permit = Some(turn_permit);
        let drain_started = std::time::Instant::now();
        loop {
            // Register before the map read. `notify_waiters` stores no permit.
            let notified = self.turn_cancel.notified();
            if self.turn_was_cancelled(&session_key, generation).await {
                return self
                    .leave_cancelled_turn(
                        session_id,
                        session,
                        &mut state,
                        &mut rx,
                        join,
                        turn_permit.take().expect("turn permit"),
                    )
                    .await;
            }
            tokio::select! {
                biased;
                _ = notified => continue,
                event = rx.recv() => {
                    match event {
                        Some(event) => {
                            self.handle_event(session_id, session, &mut state, event)
                                .await;
                        }
                        None => break,
                    }
                }
            }
        }
        let events_drained = drain_started.elapsed();
        tracing::info!(
            target: "perf_probe",
            delta_chars = state.delta_text.chars().count(),
            delta_preview = %state.delta_text.chars().take(120).collect::<String>(),
            end_preview = %state.end_text.as_deref().unwrap_or("").chars().take(120).collect::<String>(),
            "turn_text_assembly"
        );
        // A worker that died without emitting a terminal event closed the
        // channel above; the join result is the only remaining evidence.
        // Swallowing it reported a panicked or aborted worker as a clean
        // EndTurn — indistinguishable from real completion.
        let join_failure = match join.await {
            Ok(()) => None,
            Err(join_error) => Some(worker_join_failure(join_error)),
        };
        if let (Some(message), None) = (&join_failure, &state.turn_failure) {
            tracing::error!(
                session = %session_id.0,
                detail = %message,
                "turn worker died without a terminal event"
            );
            state.turn_failure = Some(message.clone());
        }
        if state.turn_failure.is_none()
            && !self
                .turn_was_cancelled(session_id.0.as_ref(), generation)
                .await
        {
            if let Some((goal, total_steps, duration_ms)) = state.achieved_goal.take() {
                match crate::goal::retire_achieved_durable_goal(session, governing_goal.as_ref())
                    .await
                {
                    Ok(true) => {
                        self.goals.lock().await.remove(session_id.0.as_ref());
                        self.emit_text(session_id, &format!("\ngoal achieved and cleared ({total_steps} steps, {duration_ms} ms): {goal}\n")).await;
                    }
                    Ok(false) => {}
                    Err(error) => state.turn_failure = Some(error.to_string()),
                }
            }
        }
        // Emit the outcome BEFORE the response resolves: the client reads
        // the same FIFO wire, so the terminal marker it builds on the
        // response can carry this outcome.
        if let Some(outcome) = state.end_outcome.take() {
            self.emit_turn_outcome(session_id, outcome).await;
        }
        self.close_run_observation(&mut state).await;
        drop(turn_permit);
        tracing::info!(
            target: "perf_probe",
            events_drained_ms = events_drained.as_millis() as u64,
            join_ms = (drain_started.elapsed() - events_drained).as_millis() as u64,
            "stream_turn_tail"
        );
        if let Some(message) = state.turn_failure {
            return Err(agent_failure_error(&message));
        }
        Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
    }

    async fn turn_was_cancelled(&self, session_key: &str, generation: u64) -> bool {
        let recorded = self
            .cancelled_generation
            .lock()
            .await
            .get(session_key)
            .copied();
        crate::turn_cancel::generation_was_cancelled(recorded, generation)
    }

    /// Return `Cancelled` without awaiting a stuck public join.
    ///
    /// A short drain lets a tool that is already waiting append `cancelled`.
    /// `cancel_and_settle` then aborts the inner fact worker. The public join
    /// is only a waiter; the turn lock stays with it until that waiter ends.
    async fn leave_cancelled_turn(
        &self,
        session_id: &acp::SessionId,
        session: &AgentSession,
        state: &mut TurnState,
        rx: &mut tokio::sync::mpsc::Receiver<AgentEvent>,
        join: JoinHandle<()>,
        turn_permit: OwnedMutexGuard<()>,
    ) -> acp::Result<acp::PromptResponse> {
        let deadline = tokio::time::Instant::now() + crate::turn_cancel::CANCEL_EVENT_DRAIN;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            tokio::select! {
                biased;
                _ = tokio::time::sleep(remaining) => break,
                event = rx.recv() => {
                    match event {
                        Some(event) => {
                            self.handle_event(session_id, session, state, event).await;
                        }
                        None => {
                            self.settle_public_join(join, turn_permit).await;
                            self.close_run_observation(state).await;
                            tracing::info!(target: "perf_probe", "turn_cancel_return");
                            return Ok(acp::PromptResponse::new(acp::StopReason::Cancelled));
                        }
                    }
                }
            }
        }
        let _idle = session
            .cancel_and_settle(
                crate::turn_cancel::CANCEL_SETTLE_GRACE,
                crate::turn_cancel::CANCEL_ABORT_GRACE,
            )
            .await;
        self.settle_public_join(join, turn_permit).await;
        self.close_run_observation(state).await;
        tracing::info!(target: "perf_probe", "turn_cancel_return");
        Ok(acp::PromptResponse::new(acp::StopReason::Cancelled))
    }

    async fn settle_public_join(&self, mut join: JoinHandle<()>, turn_permit: OwnedMutexGuard<()>) {
        tokio::select! {
            biased;
            result = &mut join => {
                let _ = result;
                drop(turn_permit);
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                crate::turn_cancel::detach_turn_permit(turn_permit, join);
            }
        }
    }

    /// A cancel for this generation wins over both a normal end and a stream error.
    /// The word "cancel" inside an unrelated error is not a cancellation.
    async fn finish_prompt(
        &self,
        session_key: &str,
        generation: u64,
        response: acp::Result<acp::PromptResponse>,
    ) -> acp::Result<acp::PromptResponse> {
        {
            let sessions = self.sessions.lock().await;
            if let Some(live) = sessions.get(session_key) {
                *live
                    .active_prompt_id
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            }
        }
        let cancelled = self
            .cancelled_generation
            .lock()
            .await
            .get(session_key)
            .copied()
            == Some(generation);
        if cancelled {
            return Ok(acp::PromptResponse::new(acp::StopReason::Cancelled));
        }
        // WC-NOTIFY1: the completion signal the agent itself can emit —
        // a terminal bell on stderr when the configured floor elapsed.
        // Focus detection and OS-level notifications stay with the
        // terminal host; the payload never carries content.
        let elapsed = {
            let sessions = self.sessions.lock().await;
            sessions.get(session_key).and_then(|live| {
                live.turn_started
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .map(|started| started.elapsed())
            })
        };
        if let Some(elapsed) = elapsed {
            if let Some(notifications) = &self.launch.config.notifications {
                if completion_bell_due(notifications, elapsed) {
                    eprint!("\x07");
                    tracing::info!(seconds = elapsed.as_secs_f32(), "run completion bell");
                }
            }
        }
        response
    }

    async fn handle_compact_extension(
        &self,
        args: &acp::ExtRequest,
    ) -> acp::Result<acp::ExtResponse> {
        let request: serde_json::Value =
            serde_json::from_str(args.params.get()).map_err(|error| {
                acp::Error::invalid_params().data(format!("compact request: {error}"))
            })?;
        let session_id = request
            .get("sessionId")
            .or_else(|| request.get("session_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| acp::Error::invalid_params().data("compact request: sessionId"))?;
        let user_context = request
            .get("userContext")
            .or_else(|| request.get("user_context"))
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let session = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|live| Arc::clone(&live.session))
                .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?
        };
        session
            .compact_conversation(user_context.as_deref())
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        let body = serde_json::value::to_raw_value(&serde_json::json!({}))
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(body.into()))
    }

    async fn handle_interject(&self, args: &acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let request = parse_interject_params(args.params.get())
            .map_err(|error| acp::Error::invalid_params().data(error))?;
        let session = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(&request.session_id)
                .map(|live| Arc::clone(&live.session))
                .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?
        };
        session
            .steer(a3s_code_core::SteerRequest::new(request.text))
            .await
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        let body = serde_json::value::to_raw_value(&serde_json::json!({ "ok": true }))
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(body.into()))
    }

    /// `a3s/review/target` pins current target bytes; `a3s/review/admit` admits
    /// bound drafts; `a3s/review/settle` moves an admitted Finding under its
    /// admitted scope. All wait behind the session's turn queue like goal import.
    async fn handle_review(&self, args: &acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let invalid = |error: String| acp::Error::invalid_params().data(error);
        let request =
            crate::engineering::ReviewRequest::parse(args.method.as_ref(), args.params.get())
                .map_err(invalid)?;
        let (session, turn_queue) = self
            .sessions
            .lock()
            .await
            .get(request.session_id())
            .map(|live| (Arc::clone(&live.session), Arc::clone(&live.turn_queue)))
            .ok_or_else(|| acp::Error::resource_not_found(None))?;
        if session.is_closed() {
            return Err(acp::Error::resource_not_found(None));
        }
        let _turn_permit = turn_queue
            .acquire_owned()
            .await
            .ok_or_else(|| invalid("review queue is full".into()))?;
        let body = request.answer(&session).await.map_err(invalid)?;
        let raw = serde_json::value::to_raw_value(&body)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(raw.into()))
    }

    /// Typed goal commands are the Core DTOs; writes wait behind the turn queue.
    /// No method here ticks: a slice starts only from the user's `/goal resume`.
    async fn handle_goal_command(&self, args: &acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        use a3s_code_core::goal_control as goal;
        enum Command {
            Definition(goal::GoalDefinitionCommandV1),
            Plan(goal::GoalPlanCommandV1),
            Control(goal::GoalControlCommandV1),
            Wait(goal::GoalWaitCommandV1),
            State,
        }
        fn parse<T: serde::de::DeserializeOwned>(params: &str) -> acp::Result<T> {
            serde_json::from_str(params)
                .map_err(|error| acp::Error::invalid_params().data(error.to_string()))
        }
        let params = args.params.get();
        let (session_id, command) = match args.method.as_ref() {
            "a3s/goal/definition" => {
                let command: goal::GoalDefinitionCommandV1 = parse(params)?;
                (command.session_id.clone(), Command::Definition(command))
            }
            "a3s/goal/plan" => {
                let command: goal::GoalPlanCommandV1 = parse(params)?;
                (command.session_id.clone(), Command::Plan(command))
            }
            "a3s/goal/control" => {
                let command: goal::GoalControlCommandV1 = parse(params)?;
                (command.session_id.clone(), Command::Control(command))
            }
            "a3s/goal/wait" => {
                let command: goal::GoalWaitCommandV1 = parse(params)?;
                (command.session_id.clone(), Command::Wait(command))
            }
            _ => {
                let request: crate::goal::GoalStateRequest = parse(params)?;
                (request.session_id, Command::State)
            }
        };
        let (session, turn_queue) = self
            .sessions
            .lock()
            .await
            .get(&session_id)
            .map(|live| (Arc::clone(&live.session), Arc::clone(&live.turn_queue)))
            .ok_or_else(|| acp::Error::resource_not_found(None))?;
        if session.is_closed() {
            return Err(acp::Error::resource_not_found(None));
        }
        fn reply<T: serde::Serialize>(
            result: a3s_code_core::error::Result<T>,
        ) -> acp::Result<Box<serde_json::value::RawValue>> {
            let value =
                result.map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
            serde_json::value::to_raw_value(&value)
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))
        }
        let writes = !matches!(command, Command::State);
        let _turn_permit = if writes {
            Some(
                turn_queue
                    .acquire_owned()
                    .await
                    .ok_or_else(|| acp::Error::invalid_params().data("goal queue is full"))?,
            )
        } else {
            None
        };
        let raw = match command {
            Command::Definition(command) => reply(session.apply_goal_definition(command)),
            Command::Plan(command) => reply(session.apply_goal_plan(command)),
            Command::Control(command) => reply(session.apply_goal_control(command)),
            Command::Wait(command) => reply(session.apply_goal_wait(command)),
            // A policy session's create command needs the digest its Run admitted.
            Command::State => reply(session.goal_definition().and_then(|definition| {
                Ok(serde_json::json!({
                    "definition": definition,
                    "work_policy_digest": session.work_policy_digest()?,
                }))
            })),
        }?;
        if writes {
            self.goals.lock().await.remove(&session_id);
        }
        Ok(acp::ExtResponse::new(raw.into()))
    }

    /// `x.ai/session/info` is the pager's `/session-info` and `/context` source.
    /// The pager decodes `ExtMethodResult<SessionInfoResponse>` and treats any
    /// other body, including JSON null, as an invalid response.
    async fn handle_session_info(&self, args: &acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let request: serde_json::Value =
            serde_json::from_str(args.params.get()).map_err(|error| {
                acp::Error::invalid_params().data(format!("session info request: {error}"))
            })?;
        let session_id = request
            .get("sessionId")
            .or_else(|| request.get("session_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| acp::Error::invalid_params().data("session info request: sessionId"))?;
        let live = {
            let sessions = self.sessions.lock().await;
            sessions.get(session_id).map(|live| {
                // The fact scheduler increments its turn only on `user.message`.
                // Tool rounds and assistant tool lines are other fact kinds, and
                // the folded transcript stores those lines with role=user.
                let turns = product_prompt_turns(live.workspace.as_path(), session_id);
                let (prompt_tokens, completion_tokens, total_tokens) =
                    live.session.recorded_usage_tokens();
                let used = if total_tokens > 0 {
                    total_tokens
                } else {
                    prompt_tokens.saturating_add(completion_tokens)
                };
                let total = live.session.context_window_tokens() as u64;
                (
                    live.workspace.display().to_string(),
                    live.model_id.clone(),
                    live.posture.clone(),
                    turns,
                    used,
                    total,
                )
            })
        };
        let body = match live {
            Some((cwd, model_id, posture, turns, used, total)) => {
                let usage_pct = usage_percent(used, total);
                serde_json::json!({
                    "result": {
                        "sessionId": session_id,
                        "cwd": cwd,
                        "permissionPosture": posture,
                        "model": &model_id,
                        "resolvedModelId": &model_id,
                        "modelFingerprint": null,
                        "turns": turns,
                        "turnIndex": turns,
                        "context": {
                            "used": used,
                            "total": total,
                            "usagePct": usage_pct,
                            "turnCount": turns,
                            "freeTokens": total.saturating_sub(used)
                        }
                    }
                })
            }
            None => serde_json::json!({
                "result": null,
                "error": "unknown session id"
            }),
        };
        let raw = serde_json::value::to_raw_value(&body)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(raw.into()))
    }

    /// Push the fact-log transcript as historical session updates.
    ///
    /// The pager does not read the fact log itself. `session/load` is the
    /// path `--resume` already calls, and untagged updates are dropped once
    /// the load window closes.
    async fn replay_loaded_transcript(&self, session_id: &acp::SessionId, session: &AgentSession) {
        let mut meta = acp::Meta::new();
        meta.insert("isReplay".to_string(), serde_json::json!(true));
        for message in session.durable_transcript() {
            if !message.is_product_transcript() {
                continue;
            }
            let text = message.transcript_display_text();
            if text.trim().is_empty() {
                continue;
            }
            let block = acp::ContentBlock::Text(acp::TextContent::new(text));
            let update = if message.role.eq_ignore_ascii_case("assistant") {
                acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(block))
            } else if message.role.eq_ignore_ascii_case("user") {
                acp::SessionUpdate::UserMessageChunk(acp::ContentChunk::new(block))
            } else {
                continue;
            };
            let mut notification = acp::SessionNotification::new(session_id.clone(), update);
            notification.meta = Some(meta.clone());
            self.emit(notification).await;
        }
    }

    /// `x.ai/permission_posture` admits a named next-run posture.
    ///
    /// Optional `deny` rules are stored on that policy. This method does not
    /// execute a tool or write a caller-supplied path. A later prompt is what
    /// the real tool executor admits or denies.
    async fn handle_permission_posture(
        &self,
        args: &acp::ExtRequest,
    ) -> acp::Result<acp::ExtResponse> {
        let request: serde_json::Value =
            serde_json::from_str(args.params.get()).map_err(|error| {
                acp::Error::invalid_params().data(format!("permission posture request: {error}"))
            })?;
        let session_id = request
            .get("sessionId")
            .or_else(|| request.get("session_id"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                acp::Error::invalid_params().data("permission posture request: sessionId")
            })?;
        let requested = request
            .get("posture")
            .and_then(|value| value.as_str())
            .unwrap_or("default");
        let posture = canonical_posture(requested).ok_or_else(|| {
            acp::Error::invalid_params().data(format!("unknown posture: {requested}"))
        })?;
        let mut extra = Vec::new();
        match request.get("deny") {
            Some(value) if value.is_string() => {
                if let Some(rule) = value.as_str().filter(|rule| !rule.is_empty()) {
                    extra.push(rule.to_string());
                }
            }
            Some(value) => {
                if let Some(rules) = value.as_array() {
                    for rule in rules {
                        if let Some(rule) = rule.as_str().filter(|rule| !rule.is_empty()) {
                            extra.push(rule.to_string());
                        }
                    }
                }
            }
            None => {}
        }
        let (session, posture_name, denies) = {
            let mut sessions = self.sessions.lock().await;
            let live = sessions
                .get_mut(session_id)
                .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?;
            live.posture = posture.to_string();
            live.denies.extend(extra);
            remember_denies(live);
            (
                Arc::clone(&live.session),
                live.posture.clone(),
                live.denies.clone(),
            )
        };
        let policy = posture_policy(&posture_name, &denies);
        session
            .set_permission_policy(policy)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        let planning = if posture_name == "plan" {
            a3s_code_core::PlanningMode::Enabled
        } else {
            a3s_code_core::PlanningMode::Disabled
        };
        session
            .set_planning_mode(planning)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;

        let body = serde_json::json!({
            "result": {
                "sessionId": session_id,
                "permissionPosture": posture_name
            }
        });
        let raw = serde_json::value::to_raw_value(&body)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(raw.into()))
    }

    /// `x.ai/session/delete` is the pager's `/delete` confirmation.
    ///
    /// The pager removes its live view only after this returns without an
    /// error. The session directory under the a3s home is what `--continue`
    /// reads, so a null body would report success and leave that directory.
    async fn handle_session_delete(&self, args: &acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        let request: serde_json::Value =
            serde_json::from_str(args.params.get()).map_err(|error| {
                acp::Error::invalid_params().data(format!("session delete request: {error}"))
            })?;
        let session_id = request
            .get("sessionId")
            .or_else(|| request.get("session_id"))
            .and_then(|value| value.as_str())
            .filter(|value| uuid::Uuid::parse_str(value).is_ok())
            .ok_or_else(|| {
                acp::Error::invalid_params().data("session delete request: sessionId")
            })?;
        let cwd = request
            .get("cwd")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let workspace = {
            let mut sessions = self.sessions.lock().await;
            sessions.remove(session_id).map(|live| live.workspace)
        };
        // A deleted session's servers leave the global union: resync so the
        // remaining sessions' sets are exactly served (no stale superset).
        if workspace.is_some() {
            if let Err(error) = self.sync_session_mcp_servers(None, &[]).await {
                tracing::warn!(
                    session_id,
                    %error,
                    "failed to resync the MCP union after session delete"
                );
            }
        }
        self.goals.lock().await.remove(session_id);
        self.turn_generation.lock().await.remove(session_id);
        self.cancelled_generation.lock().await.remove(session_id);
        let mut failures = Vec::new();
        if let Some(workspace) = workspace.as_deref() {
            if let Err(error) = remove_fact_log(workspace, session_id) {
                failures.push(error);
            }
        }
        if let Some(cwd) = cwd.as_deref() {
            if let Err(error) = remove_fact_log(std::path::Path::new(cwd), session_id) {
                failures.push(error);
            }
        }
        if let Err(error) = remove_recorded_session(session_id) {
            failures.push(error);
        }
        if !failures.is_empty() {
            return Err(acp::Error::internal_error().data(failures.join("; ")));
        }
        let body = serde_json::json!({ "success": true });
        let raw = serde_json::value::to_raw_value(&body)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::ExtResponse::new(raw.into()))
    }
}

fn product_prompt_turns(workspace: &std::path::Path, session_id: &str) -> u64 {
    let thread = a3s_code_core::fact_control::thread_for_session(session_id);
    let Ok(facts) = a3s_code_core::fact_control::read_workspace_facts(workspace, &thread) else {
        return 0;
    };
    facts
        .iter()
        .filter(|fact| fact.kind == "user.message")
        .count() as u64
}

fn remove_fact_log(workspace: &std::path::Path, session_id: &str) -> Result<(), String> {
    let thread = a3s_code_core::fact_control::thread_for_session(session_id);
    let path = a3s_code_core::session_folder::legacy_effect_log_dir(workspace)
        .join(format!("{thread}.jsonl"));
    if path.is_file() {
        std::fs::remove_file(&path)
            .map_err(|error| format!("remove fact log {}: {error}", path.display()))?;
    }
    match a3s_code_core::session_folder::find_session_dir(workspace, session_id) {
        Ok(Some(dir)) => {
            let date_dir = dir.parent().map(std::path::Path::to_path_buf);
            std::fs::remove_dir_all(&dir)
                .map_err(|error| format!("remove session folder {}: {error}", dir.display()))?;
            if let Some(date_dir) = date_dir {
                match std::fs::remove_dir(&date_dir) {
                    Ok(()) => {}
                    Err(error)
                        if error.kind() == std::io::ErrorKind::NotFound
                            || error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                    Err(error) => {
                        return Err(format!(
                            "remove session date folder {}: {error}",
                            date_dir.display()
                        ));
                    }
                }
            }
        }
        Ok(None) => {}
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}

/// Remove `{a3s-home}/sessions/{cwd}/{session_id}` for every recorded cwd.
///
/// The directory name is the url-encoded cwd. Matching the session id keeps
/// the delete on that session's own record.
fn recorded_session_homes(
    a3s_home: Option<&std::ffi::OsStr>,
    grok_home: Option<&std::ffi::OsStr>,
    os_home: Option<&std::ffi::OsStr>,
) -> Vec<PathBuf> {
    let mut homes = Vec::new();
    if let Some(env) = a3s_home.filter(|value| !value.is_empty()) {
        homes.push(PathBuf::from(env));
    }
    if let Some(env) = grok_home.filter(|value| !value.is_empty()) {
        homes.push(PathBuf::from(env));
    }
    if let Some(home) = os_home.filter(|value| !value.is_empty()) {
        let home = PathBuf::from(home);
        homes.push(home.join(".a3s"));
        homes.push(home.join(".grok"));
    }
    homes.sort();
    homes.dedup();
    homes
}

fn remove_recorded_session_under(
    session_id: &str,
    homes: &[std::path::PathBuf],
) -> Result<(), String> {
    let mut failures = Vec::new();
    for home in homes {
        let sessions = home.join("sessions");
        if !sessions.exists() {
            continue;
        }
        let entries = match std::fs::read_dir(&sessions) {
            Ok(entries) => entries,
            Err(error) => {
                failures.push(format!("read {}: {error}", sessions.display()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    failures.push(format!("read {}: {error}", sessions.display()));
                    continue;
                }
            };
            let dir = entry.path().join(session_id);
            if dir.is_dir() {
                if let Err(error) = std::fs::remove_dir_all(&dir) {
                    failures.push(format!("remove {}: {error}", dir.display()));
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn remove_recorded_session(session_id: &str) -> Result<(), String> {
    let homes = recorded_session_homes(
        std::env::var_os("A3S_HOME").as_deref(),
        std::env::var_os("GROK_HOME").as_deref(),
        std::env::var_os("HOME").as_deref(),
    );
    remove_recorded_session_under(session_id, &homes)
}

fn usage_percent(used: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((used as f64) / (total as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8
}

enum ToolRowPaint {
    Open,
    Update,
}

/// The first announcement of an id opens one pager row. Confirmation and
/// execution both announce that id; the later one updates the same row.
fn note_tool_row(painted: &mut HashSet<String>, id: &str) -> ToolRowPaint {
    if painted.insert(id.to_string()) {
        ToolRowPaint::Open
    } else {
        ToolRowPaint::Update
    }
}

#[derive(Default)]
struct TurnState {
    observer: Option<crate::run_observation::Observer>,
    turn_failure: Option<String>,
    saw_text: bool,
    delta_text: String,
    end_text: Option<String>,
    /// The completion-gate terminal from the run's End event, forwarded to
    /// the client as an `x.ai/turn/outcome` notification just before the
    /// prompt response resolves (so it precedes the terminal marker).
    end_outcome: Option<serde_json::Value>,
    achieved_goal: Option<(String, usize, i64)>,
    subagent_started_ms: HashMap<String, u64>,
    child_progress: HashMap<String, ChildProgress>,
    prompted_questions: HashSet<String>,
    /// Tool calls already given a pager row. A later start for the same id
    /// updates that row instead of opening another Run card.
    painted_tools: HashSet<String>,
}

#[derive(Default)]
struct ChildProgress {
    turn_count: u32,
    tool_call_count: u32,
    tools_used: Vec<String>,
    error_count: u32,
    tokens_used: u64,
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
fn union_mcp_desired(
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
fn merge_with_origin<'a>(
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
fn same_server_definition(
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

fn session_mcp_configs(servers: &[acp::McpServer]) -> Vec<a3s_code_core::mcp::McpServerConfig> {
    servers.iter().filter_map(acp_mcp_to_config).collect()
}

fn acp_mcp_to_config(server: &acp::McpServer) -> Option<a3s_code_core::mcp::McpServerConfig> {
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

#[async_trait::async_trait(?Send)]
impl acp::Agent for A3sCodeAgent {
    async fn initialize(
        &self,
        args: acp::InitializeRequest,
    ) -> acp::Result<acp::InitializeResponse> {
        self.client_reads_files.set(args.client_capabilities.fs.read_text_file);
        let cwd = self.launch.workspace.to_string_lossy().to_string();
        let current = self.model_id.lock().await.clone();
        let effort_token = self.effort.lock().await.map(|limits| limits.token);
        let model_state = self.model_state_with(&current, effort_token);
        Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1)
            .agent_capabilities(
                acp::AgentCapabilities::new()
                    .load_session(true)
                    .prompt_capabilities(acp::PromptCapabilities::new().embedded_context(true)),
            )
            .auth_methods(Vec::new())
            .meta(
                serde_json::json!({
                    "grokShell": false,
                    "a3sCode": true,
                    "agentVersion": env!("CARGO_PKG_VERSION"),
                    "currentWorkingDirectory": cwd,
                    "modelState": model_state,
                    "availableCommands": commands::available_commands(),
                    "cancelRewind": false,
                    "sessionRecap": false,
                    "feedbackTraceOffer": false,
                    "a3s": self.workspace_admission_meta(),
                })
                .as_object()
                .cloned(),
            ))
    }

    async fn authenticate(
        &self,
        _args: acp::AuthenticateRequest,
    ) -> acp::Result<acp::AuthenticateResponse> {
        Ok(acp::AuthenticateResponse::default())
    }

    async fn new_session(
        &self,
        args: acp::NewSessionRequest,
    ) -> acp::Result<acp::NewSessionResponse> {
        let host = crate::host_context::studio_host_from_meta(args.meta.as_ref())
            .map_err(|error| acp::Error::invalid_params().data(error))?;
        self.validate_plugin_selection(args.meta.as_ref(), &args.mcp_servers)?;
        self.admit_studio_documents(args.meta.as_ref());
        let session_id = uuid::Uuid::new_v4().to_string();
        let workspace = if args.cwd.as_os_str().is_empty() {
            self.launch.workspace.clone()
        } else {
            args.cwd.clone()
        };
        if let Some(host) = &self.plugin_selection {
            host.validate_workspace(&self.launch.workspace)
                .and_then(|_| host.validate_workspace(&workspace))
                .map_err(|_| {
                    acp::Error::invalid_params().data("native plugin workspace is not admitted")
                })?;
        }
        a3s_code_core::session_folder::prepare_session_home(&workspace, &session_id)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        let session_mcp = session_mcp_configs(&args.mcp_servers);
        self.sync_session_mcp_servers(Some(session_id.as_ref()), &args.mcp_servers)
            .await?;
        let model_id = self.model_id.lock().await.clone();
        let effort = *self.effort.lock().await;
        let plugin_tools = crate::plugin_tools::entries_from_meta(args.meta.as_ref());
        let components = crate::harness_assemble::components_from_meta(args.meta.as_ref())
            .map_err(|error| acp::Error::invalid_params().data(error))?;
        let live = self
            .open_core_session(
                &session_id,
                SessionSetup {
                    workspace: workspace.clone(),
                    model_id: &model_id,
                    effort,
                    host,
                    components: &components,
                    mcp_servers: session_mcp,
                },
                false,
            )
            .await?;
        let live = self.finish_plugin_tools(live, plugin_tools).await?;
        let effort_token = self.effort.lock().await.map(|limits| limits.token);
        let models = self.model_state_with(&live.model_id, effort_token);
        let goal = match live.session.goal_definition() {
            Ok(Some(_)) => Ok(None),
            Ok(None) => crate::goal::load_goal(&workspace, &session_id)
                .await
                .map_err(|error| acp::Error::invalid_params().data(error.to_string())),
            Err(error) => Err(acp::Error::invalid_params().data(error.to_string())),
        };
        let goal = match goal {
            Ok(goal) => goal,
            Err(error) => {
                live.session.close().await;
                return Err(error);
            }
        };
        self.sessions.lock().await.insert(session_id.clone(), live);
        if let Some(goal) = goal {
            self.goals.lock().await.insert(session_id.clone(), goal);
        } else {
            self.goals.lock().await.remove(&session_id);
        }
        self.reapply_active_goal(&session_id).await;

        let config_options = Self::model_config_options(&models);
        self.announce_commands(&session_id);
        Ok(
            acp::NewSessionResponse::new(acp::SessionId::new(session_id))
                .models(models)
                .config_options(config_options),
        )
    }

    async fn load_session(
        &self,
        args: acp::LoadSessionRequest,
    ) -> acp::Result<acp::LoadSessionResponse> {
        let host = crate::host_context::studio_host_from_meta(args.meta.as_ref())
            .map_err(|error| acp::Error::invalid_params().data(error))?;
        self.validate_plugin_selection(args.meta.as_ref(), &args.mcp_servers)?;
        let session_id = args.session_id.0.to_string();
        let workspace = self.admit_resume(&session_id, &args.cwd).await?;
        if let Some(host) = &self.plugin_selection {
            host.validate_workspace(&self.launch.workspace)
                .and_then(|_| host.validate_workspace(&workspace))
                .map_err(|_| {
                    acp::Error::invalid_params().data("native plugin workspace is not admitted")
                })?;
        }
        let components = crate::harness_assemble::components_from_meta(args.meta.as_ref())
            .map_err(|error| acp::Error::invalid_params().data(error))?;
        self.admit_studio_documents(args.meta.as_ref());
        let session_mcp = session_mcp_configs(&args.mcp_servers);
        self.sync_session_mcp_servers(Some(session_id.as_ref()), &args.mcp_servers)
            .await?;
        let model_id = self.model_id.lock().await.clone();
        let effort = *self.effort.lock().await;
        let plugin_tools = crate::plugin_tools::entries_from_meta(args.meta.as_ref());
        let live = self
            .open_core_session(
                &session_id,
                SessionSetup {
                    workspace: workspace.clone(),
                    model_id: &model_id,
                    effort,
                    host,
                    components: &components,
                    mcp_servers: session_mcp,
                },
                true,
            )
            .await?;
        let live = self.finish_plugin_tools(live, plugin_tools).await?;
        let effort_token = self.effort.lock().await.map(|limits| limits.token);
        let models = self.model_state_with(&live.model_id, effort_token);
        let transcript = Arc::clone(&live.session);
        let goal = match live.session.goal_definition() {
            Ok(Some(_)) => Ok(None),
            Ok(None) => crate::goal::load_goal(&workspace, &session_id)
                .await
                .map_err(|error| acp::Error::invalid_params().data(error.to_string())),
            Err(error) => Err(acp::Error::invalid_params().data(error.to_string())),
        };
        let goal = match goal {
            Ok(goal) => goal,
            Err(error) => {
                live.session.close().await;
                return Err(error);
            }
        };
        self.sessions.lock().await.insert(session_id.clone(), live);
        if let Some(goal) = goal {
            self.goals.lock().await.insert(session_id.clone(), goal);
        } else {
            self.goals.lock().await.remove(&session_id);
        }
        self.reapply_active_goal(&session_id).await;
        // The pager keeps `isReplay` updates only while this load is in flight
        // (and for a short grace after). Emit before the response returns.
        self.replay_loaded_transcript(&args.session_id, &transcript)
            .await;
        let config_options = Self::model_config_options(&models);
        self.announce_commands(&session_id);
        Ok(acp::LoadSessionResponse::new()
            .models(models)
            .config_options(config_options))
    }

    /// ACP session modes: the A3S permission/mode surface. `plan` enables
    /// maker planning; every other mode runs with the session's configured
    /// planning default.
    async fn set_session_mode(
        &self,
        args: acp::SetSessionModeRequest,
    ) -> acp::Result<acp::SetSessionModeResponse> {
        let session_key = args.session_id.0.to_string();
        let mode_id = args.mode_id.0.to_string();
        let named = canonical_posture(&mode_id);
        let (session, posture, denies) = {
            let mut sessions = self.sessions.lock().await;
            let live = sessions
                .get_mut(&session_key)
                .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?;
            if let Some(name) = named {
                live.posture = name.to_string();
            }
            (
                Arc::clone(&live.session),
                live.posture.clone(),
                live.denies.clone(),
            )
        };
        if named.is_some() {
            session
                .set_permission_policy(posture_policy(&posture, &denies))
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        }
        let planning = if mode_id.eq_ignore_ascii_case("plan") {
            a3s_code_core::PlanningMode::Enabled
        } else {
            a3s_code_core::PlanningMode::Disabled
        };
        session
            .set_planning_mode(planning)
            .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
        Ok(acp::SetSessionModeResponse::new())
    }

    async fn set_session_model(
        &self,
        args: acp::SetSessionModelRequest,
    ) -> acp::Result<acp::SetSessionModelResponse> {
        let model_id = args.model_id.0.to_string();
        if !self.known_model(&model_id) {
            return Err(acp::Error::invalid_params().data(format!("unknown model id: {model_id}")));
        }

        let effort = match args.meta.as_ref() {
            Some(meta) if meta.contains_key("reasoningEffort") => {
                effort_limits_from_meta(Some(meta)).or(*self.effort.lock().await)
            }
            _ => *self.effort.lock().await,
        };

        let session_key = args.session_id.0.to_string();
        let existing = {
            let sessions = self.sessions.lock().await;
            sessions.get(&session_key).map(|live| {
                (
                    Arc::clone(&live.session),
                    live.workspace.clone(),
                    live.host.clone(),
                    live.plugin_tools.clone(),
                    live.components.clone(),
                    // Carried into the rebuilt session: the recorded set is
                    // this session's contribution to the global union, and
                    // dropping it here would let the next sync evict this
                    // session's servers while it stays live.
                    live.mcp_servers.clone(),
                )
            })
        };

        let live = if let Some((current, workspace, host, plugin_tools, components, mcp_servers)) =
            existing
        {
            let live = self
                .replace_core_session(
                    &current,
                    SessionSetup {
                        workspace,
                        model_id: &model_id,
                        effort,
                        host,
                        components: &components,
                        mcp_servers,
                    },
                )
                .await?;
            self.finish_plugin_tools(live, plugin_tools).await?
        } else {
            let components = crate::harness_assemble::components_from_meta(None)
                .map_err(|error| acp::Error::invalid_params().data(error))?;
            self.open_core_session(
                &session_key,
                SessionSetup {
                    workspace: self.launch.workspace.clone(),
                    model_id: &model_id,
                    effort,
                    host: crate::host_context::StudioHost::default(),
                    components: &components,
                    mcp_servers: Vec::new(),
                },
                false,
            )
            .await?
        };
        self.sessions.lock().await.insert(session_key.clone(), live);
        self.reapply_active_goal(&session_key).await;
        *self.model_id.lock().await = model_id;
        *self.effort.lock().await = effort;

        Ok(acp::SetSessionModelResponse::new())
    }

    async fn set_session_config_option(
        &self,
        args: acp::SetSessionConfigOptionRequest,
    ) -> acp::Result<acp::SetSessionConfigOptionResponse> {
        if args.config_id.0.as_ref() != MODEL_CONFIG_ID {
            return Err(acp::Error::invalid_params().data(format!(
                "unknown config option: {} (expected {MODEL_CONFIG_ID})",
                args.config_id.0
            )));
        }
        let acp::SessionConfigOptionValue::ValueId { value } = args.value else {
            return Err(acp::Error::invalid_params().data("model config option takes a model id"));
        };
        let model_id = value.0.to_string();
        self.set_session_model(
            acp::SetSessionModelRequest::new(args.session_id, acp::ModelId::new(model_id.clone()))
                .meta(args.meta),
        )
        .await?;
        let effort_token = self.effort.lock().await.map(|limits| limits.token);
        let models = self.model_state_with(&model_id, effort_token);
        Ok(acp::SetSessionConfigOptionResponse::new(
            Self::model_config_options(&models),
        ))
    }

    async fn prompt(&self, args: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
        tracing::info!(target: "perf_probe", "prompt_enter");
        let session_key = args.session_id.0.to_string();
        let turn_started = std::time::Instant::now();
        let (session, turn_queue, posture, denies) = {
            let sessions = self.sessions.lock().await;
            let live = sessions
                .get(&session_key)
                .ok_or_else(|| acp::Error::invalid_params().data("unknown session id"))?;
            *live
                .turn_started
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(turn_started);
            (
                Arc::clone(&live.session),
                Arc::clone(&live.turn_queue),
                live.posture.clone(),
                live.denies.clone(),
            )
        };

        let prompt = Self::prompt_text(&args.prompt);
        if prompt.trim().is_empty() {
            return Ok(acp::PromptResponse::new(acp::StopReason::EndTurn));
        }
        // Queue behind an active turn instead of failing: the pager's
        // send-now / queue-reissue expect the agent to accept and serialize.
        // Owned so a cancelled turn can move the guard into a detached join.
        let turn_permit = match turn_queue.acquire_owned().await {
            Some(permit) => permit,
            None => {
                return Err(acp::Error::invalid_params().data(format!(
                    "prompt queue is full ({MAX_QUEUED_PROMPTS} waiting behind the active \
                     turn); the session is still working through queued prompts"
                )))
            }
        };
        let generation = {
            let mut generations = self.turn_generation.lock().await;
            let slot = generations.entry(session_key.clone()).or_insert(0);
            *slot = slot.saturating_add(1);
            *slot
        };
        // Echo the client's prompt id on every notification (the ACP
        // prompt-ack contract): the pager disarms its ack watch on the
        // first update naming the prompt.
        let client_prompt_id = args
            .meta
            .as_ref()
            .and_then(|meta| meta.get("promptId"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if let Some(live) = self.sessions.lock().await.get(&session_key) {
            *live
                .active_prompt_id
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = client_prompt_id.clone();
        }
        if let Some(unattended) = &self.unattended {
            // A task that starts with `/goal` is task text, not a goal command.
            let response = self
                .unattended_prompt(
                    &args.session_id,
                    &session,
                    &prompt,
                    generation,
                    turn_permit,
                    unattended,
                )
                .await;
            return self.finish_prompt(&session_key, generation, response).await;
        }
        let turn = crate::host_context::turn_context_from_meta(args.meta.as_ref());
        let observation = crate::run_observation::Binding::from_meta(args.meta.as_ref());
        let response = if let Some(op) = crate::goal::parse_goal_command(&prompt) {
            tracing::info!(target: "perf_probe", "goal_command_branch");
            self.handle_goal(
                &args.session_id,
                &session,
                op,
                generation,
                turn_permit,
                observation.as_ref(),
            )
            .await
        } else {
            let goal = if session
                .goal_definition()
                .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?
                .is_some()
            {
                None
            } else {
                crate::goal::apply_session_goal(&session, crate::goal::GoalOp::Status)
                    .await
                    .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?
                    .next
            };
            {
                let mut goals = self.goals.lock().await;
                if let Some(goal) = &goal {
                    goals.insert(session_key.clone(), goal.clone());
                } else {
                    goals.remove(&session_key);
                }
            }
            let prompt = crate::goal::standing_goal_prompt(goal.as_ref(), &prompt);
            let prompt = crate::host_context::apply_turn_directives(&prompt, &turn);
            let mut gate = ResearchGate {
                session: &session,
                posture: &posture,
                denies: &denies,
                armed: false,
            };
            let mut response = if turn.research {
                match session.set_permission_policy(crate::host_context::research_policy(&denies)) {
                    Ok(()) => {
                        gate.armed = true;
                        self.stream_turn(
                            &args.session_id,
                            &session,
                            &prompt,
                            generation,
                            turn_permit,
                            observation.as_ref(),
                        )
                        .await
                    }
                    Err(error) => {
                        drop(turn_permit);
                        Err(acp::Error::internal_error().data(error.to_string()))
                    }
                }
            } else {
                self.stream_turn(
                    &args.session_id,
                    &session,
                    &prompt,
                    generation,
                    turn_permit,
                    observation.as_ref(),
                )
                .await
            };
            if gate.armed {
                match session.set_permission_policy(posture_policy(&posture, &denies)) {
                    Ok(()) => gate.armed = false,
                    Err(error) if response.is_ok() => {
                        response = Err(acp::Error::internal_error().data(error.to_string()));
                    }
                    Err(_) => {}
                }
            }
            response
        };
        tracing::info!(target: "perf_probe", "prompt_return");
        self.finish_prompt(&session_key, generation, response).await
    }

    async fn ext_method(&self, args: acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
        if args.method.as_ref() == "a3s/goal/import" {
            let command: a3s_code_core::goal_control::GoalDefinitionCommandV1 =
                serde_json::from_str(args.params.get())
                    .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
            let session_id = command.session_id.clone();
            let (session, turn_queue) = self
                .sessions
                .lock()
                .await
                .get(&session_id)
                .map(|live| (Arc::clone(&live.session), Arc::clone(&live.turn_queue)))
                .ok_or_else(|| acp::Error::resource_not_found(None))?;
            // A retiring legacy turn can still remove its source after Core returns.
            let _turn_permit = turn_queue
                .acquire_owned()
                .await
                .ok_or_else(|| acp::Error::invalid_params().data("goal import queue is full"))?;
            let imported = crate::goal::commit_import(&session, command)
                .await
                .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
            self.goals.lock().await.remove(&session_id);
            let raw = serde_json::value::to_raw_value(&imported)
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
            return Ok(acp::ExtResponse::new(raw.into()));
        }
        if args.method.as_ref() == "a3s/goal/import/preview" {
            let request: crate::goal::GoalImportPreviewRequest =
                serde_json::from_str(args.params.get())
                    .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
            let session = self
                .sessions
                .lock()
                .await
                .get(&request.session_id)
                .map(|live| Arc::clone(&live.session))
                .ok_or_else(|| acp::Error::resource_not_found(None))?;
            if session.is_closed() {
                return Err(acp::Error::resource_not_found(None));
            }
            if session
                .goal_definition()
                .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?
                .is_some()
            {
                return Err(
                    acp::Error::invalid_params().data("goal.conflict: session already bound")
                );
            }
            let preview = crate::goal::preview_import(session.workspace(), &request)
                .await
                .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
            let raw = serde_json::value::to_raw_value(&preview)
                .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
            return Ok(acp::ExtResponse::new(raw.into()));
        }
        if matches!(
            args.method.as_ref(),
            "a3s/goal/definition"
                | "a3s/goal/plan"
                | "a3s/goal/control"
                | "a3s/goal/wait"
                | "a3s/goal/state"
        ) {
            return self.handle_goal_command(&args).await;
        }
        if matches!(
            args.method.as_ref(),
            "a3s/review/target" | "a3s/review/admit" | "a3s/review/settle"
        ) {
            return self.handle_review(&args).await;
        }
        if args.method.as_ref() == "a3s/wiki/search" {
            return answer_wiki_search(&args).await;
        }
        if args
            .method
            .as_ref()
            .starts_with("x.ai/compact_conversation")
        {
            return self.handle_compact_extension(&args).await;
        }
        if args.method.as_ref().starts_with("x.ai/interject") {
            return self.handle_interject(&args).await;
        }
        if args.method.as_ref() == "x.ai/session/info" {
            return self.handle_session_info(&args).await;
        }
        if args.method.as_ref() == "x.ai/session/list" {
            return self.handle_session_list(&args).await;
        }
        if args.method.as_ref() == "x.ai/session/delete" {
            return self.handle_session_delete(&args).await;
        }
        if args.method.as_ref() == "x.ai/permission_posture" {
            return self.handle_permission_posture(&args).await;
        }
        Err(acp::Error::method_not_found().data(args.method.to_string()))
    }

    async fn cancel(&self, args: acp::CancelNotification) -> acp::Result<()> {
        let session_key = args.session_id.0.to_string();
        let generation = self
            .turn_generation
            .lock()
            .await
            .get(&session_key)
            .copied()
            .unwrap_or(0);
        self.cancelled_generation
            .lock()
            .await
            .insert(session_key.clone(), generation);
        let session = {
            let sessions = self.sessions.lock().await;
            sessions.get(&session_key).map(|live| live.session.clone())
        };
        if let Some(session) = session {
            let _ = session.cancel().await;
        }
        self.turn_cancel.notify_waiters();
        Ok(())
    }
}

async fn answer_wiki_search(args: &acp::ExtRequest) -> acp::Result<acp::ExtResponse> {
    let request: serde_json::Value = serde_json::from_str(args.params.get()).map_err(|error| {
        acp::Error::invalid_params().data(format!("wiki search request: {error}"))
    })?;
    let wiki = request
        .get("wiki")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned();
    let query = request
        .get("query")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_owned();
    let body = tokio::task::spawn_blocking(move || {
        crate::wiki_search::search_published_body(std::path::Path::new(&wiki), &query)
    })
    .await
    .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
    let raw = serde_json::value::to_raw_value(&body)
        .map_err(|error| acp::Error::internal_error().data(error.to_string()))?;
    Ok(acp::ExtResponse::new(raw.into()))
}

/// Map a3s-code planning tasks onto ACP `PlanEntry` so the pager plan strip updates.
fn task_to_plan_entry(task: &a3s_code_core::planning::Task) -> acp::PlanEntry {
    use a3s_code_core::planning::{TaskPriority, TaskStatus};
    let priority = match task.priority {
        TaskPriority::High => acp::PlanEntryPriority::High,
        TaskPriority::Medium => acp::PlanEntryPriority::Medium,
        TaskPriority::Low => acp::PlanEntryPriority::Low,
    };
    // Failed/Skipped/Cancelled collapse to Completed so the strip stays within ACP's status set.
    let status = match task.status {
        TaskStatus::Pending => acp::PlanEntryStatus::Pending,
        TaskStatus::InProgress => acp::PlanEntryStatus::InProgress,
        TaskStatus::Completed
        | TaskStatus::Failed
        | TaskStatus::Skipped
        | TaskStatus::Cancelled => acp::PlanEntryStatus::Completed,
    };
    let mut entry = acp::PlanEntry::new(task.content.clone(), priority, status);
    let mut meta = acp::Meta::new();
    meta.insert("id".into(), serde_json::Value::String(task.id.clone()));
    if matches!(
        task.status,
        TaskStatus::Failed | TaskStatus::Skipped | TaskStatus::Cancelled
    ) {
        meta.insert(
            "a3sStatus".into(),
            serde_json::Value::String(task.status.to_string()),
        );
    }
    entry = entry.meta(Some(meta));
    entry
}

/// Effort selection for one session.
///
/// `token` is the provider effort parameter when the model has one.
/// `thinking_budget` and `max_tool_rounds` are the host budget. Each level
/// has its own pair. `max` and `ultracode` keep the tool-round safety ceiling
/// as their cap; every other level is strictly below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EffortLimits {
    token: &'static str,
    thinking_budget: usize,
    max_tool_rounds: usize,
    max_parallel_tasks: usize,
    max_continuation_turns: u32,
}

/// Host budget levels offered when the provider has no native effort parameter.
/// `ultracode` stays off the menu: it is not a `ReasoningEffort` variant.
pub(crate) const HOST_EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Advertise the effort values `/effort` can apply.
///
/// A native menu wins when the provider has one (GLM stays on low/high/max).
/// Every other model still gets the host budget menu, because thinking budget
/// and tool rounds apply whether or not the provider has an effort parameter.
/// Each entry includes `value` so the pager's `ReasoningEffortOption` parser
/// accepts it.
fn reasoning_effort_catalog_meta(model_id: &str) -> serde_json::Map<String, serde_json::Value> {
    let native = a3s_code_core::llm::native_effort_menu(model_id);
    let levels: &[&str] = if native.is_empty() {
        HOST_EFFORT_LEVELS
    } else {
        native
    };
    let mut meta = serde_json::Map::new();
    meta.insert(
        "supportsReasoningEffort".into(),
        serde_json::Value::Bool(!levels.is_empty()),
    );
    let efforts = levels
        .iter()
        .map(|level| {
            let mut entry = serde_json::json!({
                "value": level,
                "id": level,
                "label": level,
                "description": effort_level_description(level),
            });
            if *level == "high" {
                entry["default"] = serde_json::Value::Bool(true);
            }
            entry
        })
        .collect::<Vec<_>>();
    meta.insert("reasoningEfforts".into(), serde_json::Value::Array(efforts));
    meta
}

fn effort_level_description(level: &str) -> &'static str {
    match level {
        "low" => "Faster, lighter reasoning",
        "medium" => "Balanced reasoning",
        "high" => "Heavy reasoning",
        "xhigh" => "Extended reasoning",
        "max" => "Maximum reasoning",
        _ => "Reasoning effort",
    }
}

fn note_child_progress(progress: &mut ChildProgress, status: &str, metadata: &serde_json::Value) {
    let tokens = progress_tokens_used(metadata);
    if tokens > progress.tokens_used {
        progress.tokens_used = tokens;
    }
    if status == "tool_completed" {
        progress.tool_call_count = progress.tool_call_count.saturating_add(1);
        if let Some(name) = metadata.get("tool").and_then(|value| value.as_str()) {
            if !progress.tools_used.iter().any(|existing| existing == name) {
                progress.tools_used.push(name.to_string());
            }
        }
        let exit_code = metadata
            .get("exit_code")
            .and_then(|value| value.as_i64())
            .unwrap_or(0);
        if exit_code != 0 || metadata.get("error_kind").is_some() {
            progress.error_count = progress.error_count.saturating_add(1);
        }
    } else if status == "turn_completed" {
        if let Some(turn) = metadata.get("turn").and_then(|value| value.as_u64()) {
            progress.turn_count = progress.turn_count.max(turn as u32);
        }
    }
}

fn subagent_finished_update(
    task_id: &str,
    child_session_id: &str,
    success: bool,
    output: &str,
    duration_ms: u64,
    progress: &ChildProgress,
) -> serde_json::Value {
    let status = if success { "completed" } else { "failed" };
    let error = if success {
        None
    } else if output.trim().is_empty() {
        Some("subagent failed".to_string())
    } else {
        Some(output.chars().take(2_000).collect::<String>())
    };
    let mut update = serde_json::json!({
        "sessionUpdate": "subagent_finished",
        "subagent_id": task_id,
        "child_session_id": child_session_id,
        "status": status,
        "tool_calls": progress.tool_call_count,
        "turns": progress.turn_count,
        "duration_ms": duration_ms,
        "tokens_used": progress.tokens_used,
        "will_wake": false,
    });
    if let Some(error) = error {
        update["error"] = serde_json::Value::String(error);
    }
    if !output.is_empty() {
        update["output"] = serde_json::Value::String(output.chars().take(4_000).collect());
    }
    update
}

struct InterjectParams {
    session_id: String,
    text: String,
}

fn parse_interject_params(raw: &str) -> Result<InterjectParams, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("interject request: {error}"))?;
    let session_id = value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .and_then(|item| item.as_str())
        .filter(|item| !item.is_empty())
        .ok_or_else(|| "interject request: sessionId".to_string())?;
    let text = value
        .get("text")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .ok_or_else(|| "interject request: text".to_string())?;
    Ok(InterjectParams {
        session_id: session_id.to_string(),
        text: text.to_string(),
    })
}

fn progress_duration_ms(started_ms: u64, now_ms: u64) -> u64 {
    if started_ms == 0 {
        return 0;
    }
    now_ms.saturating_sub(started_ms)
}

fn unix_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn progress_tokens_used(metadata: &serde_json::Value) -> u64 {
    metadata
        .get("prompt_tokens")
        .or_else(|| metadata.get("total_tokens"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0)
}

fn context_usage(window_tokens: u64, tokens_used: u64) -> (u64, u8) {
    if window_tokens == 0 {
        return (0, 0);
    }
    let pct = tokens_used.saturating_mul(100) / window_tokens;
    (window_tokens, pct.min(100) as u8)
}

/// Always-on turn boundary. The skill command is an example, not a matcher.
const SESSION_OUTCOME_BOUNDARY: &str = "\
Finish the user's outcome in this turn. Do not read or explain the product \
implementation unless the user asked to change that implementation. When a \
host command already does the outcome, name that command and stop. Example: \
the user installs a skill with `a3s code skills add <git-url>`.";

/// Depth steer from a3s `BudgetProfile.guideline`. Medium has none.
fn effort_guideline(token: &str) -> Option<&'static str> {
    match token {
        "low" => Some(
            "[effort: low] Favor speed and minimalism. Answer directly, make the smallest \
change that works (reading enough surrounding code to change it safely), and \
keep verification proportionate: still run the narrowest build/test/type-check \
that covers what you touched — just don't add checks or scope the task didn't \
warrant. Don't gold-plate.",
        ),
        "high" => Some(
            "[effort: high] Favor depth. Reason through the approach before acting. \
Depth applies to the user's outcome, not to rediscovering the running product. After \
changes, verify the narrow path you touched (build / test / type-check) and \
check the obvious edge cases, then re-read your own diff for correctness before \
finishing.",
        ),
        "xhigh" => Some(
            "[effort: xhigh] Work rigorously. Before choosing an approach, weigh at \
least one alternative. Verify thoroughly — run the relevant tests/build, probe \
edge cases and failure modes, and confirm the change actually does what was \
asked. Do a self-review pass for correctness and simplicity before concluding.",
        ),
        "max" => Some(
            "[effort: max] Maximum rigor; prefer correctness and completeness over speed. \
Decompose the problem, compare alternatives, and implement the strongest \
solution. Verify exhaustively: tests, build, edge cases, and boundary / \
adversarial inputs. Finish with a self-critique pass that actively hunts for \
what you may have missed or gotten wrong, and fix it before concluding.",
        ),
        "ultracode" => Some(
            "[ultracode] Scripted tool fan-out is available — you decide whether a turn needs \
it. Match the effort to the task: answer trivial or conversational input (a \
greeting, a single question, a one-step edit) directly, with no plan and no \
fan-out. When a task genuinely needs many coordinated tool calls, call the \
`program` tool with one sandboxed JavaScript PTC script.",
        ),
        _ => None,
    }
}

const HOST_PARALLEL_TASKS: usize = 8;
const HOST_CONTINUATION_TURNS: u32 = 8;

fn effort_limits(token: &str) -> Option<EffortLimits> {
    let ceiling = a3s_code_core::llm::TOOL_ROUND_SAFETY_CEILING;
    let (token, thinking_budget, max_tool_rounds) = match token.to_ascii_lowercase().as_str() {
        "none" => ("none", 1024, 4),
        "minimal" => ("minimal", 1536, 8),
        "low" => ("low", 2048, 16),
        "medium" => ("medium", 8192, 48),
        "high" => ("high", 16_384, 128),
        "xhigh" => ("xhigh", 32_768, 512),
        "max" => ("max", 65_536, ceiling),
        "ultracode" => ("ultracode", 65_536, ceiling),
        _ => return None,
    };
    Some(EffortLimits {
        token,
        thinking_budget,
        max_tool_rounds,
        max_parallel_tasks: HOST_PARALLEL_TASKS,
        max_continuation_turns: HOST_CONTINUATION_TURNS,
    })
}

fn effort_limits_from_meta(meta: Option<&acp::Meta>) -> Option<EffortLimits> {
    let effort = meta
        .and_then(|m| m.get("reasoningEffort"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    effort_limits(effort)
}

/// Map pager `/effort` `_meta.reasoningEffort` onto the provider effort token.
/// The pager effort surface lands with its slice; kept linked.
#[allow(dead_code)]
fn reasoning_effort_token(meta: Option<&acp::Meta>) -> Option<&'static str> {
    effort_limits_from_meta(meta).map(|limits| limits.token)
}

/// ACP `ToolKind` the pager uses to pick Execute / Read / Edit / Search / Fetch cards.
fn tool_kind_for(name: &str) -> acp::ToolKind {
    match name {
        "bash" | "shell" | "execute" | "git" => acp::ToolKind::Execute,
        "read" => acp::ToolKind::Read,
        "write" | "edit" | "patch" => acp::ToolKind::Edit,
        "grep" | "glob" | "search" | "semantic" | "hybrid" | "bm25" | "code_symbols"
        | "code_navigation" | "code_diagnostics" | "web_search" => acp::ToolKind::Search,
        "web_fetch" | "download" => acp::ToolKind::Fetch,
        _ => acp::ToolKind::Other,
    }
}

fn tool_title(name: &str) -> String {
    match name {
        "update_plan" => "Updating plan".to_string(),
        other => other.to_string(),
    }
}

/// Copy a3s argument names onto the fields the pager card renderer reads.
fn prepared_raw_input(name: &str, args: &serde_json::Value) -> Option<serde_json::Value> {
    if args.is_null() {
        return None;
    }
    let mut input = args.clone();
    let Some(obj) = input.as_object_mut() else {
        return Some(input);
    };
    match name {
        "ls" => {
            if let Some(path) = obj.get("path").cloned() {
                obj.entry("target_directory".to_string()).or_insert(path);
            }
        }
        "write" => {
            obj.insert("variant".into(), serde_json::json!("Write"));
        }
        "update_plan" => {
            obj.insert("variant".into(), serde_json::json!("TodoWrite"));
        }
        "web_search" => {
            obj.insert("variant".into(), serde_json::json!("WebSearch"));
        }
        "task" => {
            obj.insert("variant".into(), serde_json::json!("Task"));
        }
        "search" | "semantic" | "hybrid" | "bm25" => {
            // A let-chain form needs Rust 2024; keep the guard pair flat.
            #[allow(clippy::collapsible_if, clippy::single_match)]
            match (obj.get("pattern").is_none(), obj.get("query").cloned()) {
                (true, Some(query)) => {
                    obj.insert("pattern".into(), query);
                }
                _ => {}
            }
        }
        _ => {}
    }
    Some(input)
}

fn tool_call_content(
    name: &str,
    args: Option<&serde_json::Value>,
    output: &str,
) -> Vec<acp::ToolCallContent> {
    if let Some(diff) = edit_diff(name, args) {
        return vec![acp::ToolCallContent::from(diff)];
    }
    if output.is_empty() {
        return Vec::new();
    }
    let preview: String = output.chars().take(4_000).collect();
    vec![acp::ToolCallContent::from(acp::ContentBlock::Text(
        acp::TextContent::new(preview),
    ))]
}

fn edit_diff(name: &str, args: Option<&serde_json::Value>) -> Option<acp::Diff> {
    let args = args?;
    let path = args
        .get("file_path")
        .or_else(|| args.get("path"))
        .and_then(|v| v.as_str())?;
    match name {
        "edit" | "patch" => {
            let old_text = args.get("old_string").and_then(|v| v.as_str());
            let new_text = args
                .get("new_string")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let mut diff = acp::Diff::new(path, new_text);
            if let Some(old_text) = old_text {
                diff = diff.old_text(old_text.to_string());
            }
            Some(diff)
        }
        "write" => args
            .get("content")
            .and_then(|v| v.as_str())
            .map(|content| acp::Diff::new(path, content)),
        _ => None,
    }
}

/// Bash cards only show stdout when `raw_output` deserializes as `ToolOutput::Bash`.
/// Output fields are strings: a byte array costs a JSON number per byte.
/// Incremental stdout. `output` stays empty so the pager appends `output_delta`
/// instead of replacing the card with this slice.
fn streaming_bash_delta(delta: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "Bash",
        "output": "",
        "output_for_prompt": "",
        "exit_code": 0,
        "command": "",
        "truncated": false,
        "signal": null,
        "timed_out": false,
        "description": null,
        "current_dir": "",
        "output_file": "",
        "total_bytes": delta.len(),
        "output_delta": delta,
    })
}

fn tool_raw_output(
    name: &str,
    args: Option<&serde_json::Value>,
    output: &str,
    exit_code: i32,
) -> Option<serde_json::Value> {
    if !matches!(name, "bash" | "shell" | "execute") {
        return None;
    }
    let command = args
        .and_then(|args| args.get("command"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Some(serde_json::json!({
        "type": "Bash",
        "output": output,
        "output_for_prompt": output,
        "exit_code": exit_code,
        "command": command,
        "truncated": false,
        "signal": null,
        "timed_out": false,
        "description": null,
        "current_dir": "",
        "output_file": "",
        "total_bytes": output.len(),
    }))
}

/// Whether a `request_permission` response grants the tool.
fn permission_response_approved(response: &acp::RequestPermissionResponse) -> bool {
    match &response.outcome {
        acp::RequestPermissionOutcome::Selected(selected) => {
            // YOLO picks the first AllowOnce (`allow-once`). RejectOnce / cancel stay false.
            matches!(selected.option_id.0.as_ref(), "allow-once" | "allow-always")
        }
        acp::RequestPermissionOutcome::Cancelled => false,
        _ => false,
    }
}

/// Whether the user answered "don't ask again". Only this answer learns.
fn permission_response_always(response: &acp::RequestPermissionResponse) -> bool {
    matches!(
        &response.outcome,
        acp::RequestPermissionOutcome::Selected(selected)
            if selected.option_id.0.as_ref() == "allow-always"
    )
}

/// Meta key marking a `request_permission` as an agent question riding the
/// permission channel, not a permission ask; the payload carries
/// `{"allowFreeText": bool}`. Twin of `QUESTION_META_KEY` in
/// `vendor/a3s-code-ui/crates/codegen/a3s-code-workspace/src/permission/types.rs`
/// (this crate does not depend on the vendored workspace crate; keep them in sync).
const QUESTION_META_KEY: &str = "question";

/// The free text a user typed into a permission dialog's followup input; the
/// pager attaches it as `followup_message` response meta (its
/// `dispatch_permission_followup` path). Trimmed; empty/absent yields `None`.
fn followup_message(response: &acp::RequestPermissionResponse) -> Option<&str> {
    response
        .meta
        .as_ref()
        .and_then(|meta| meta.get("followup_message"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// Question choices are not permission grants. `RejectOnce` keeps YOLO from
/// selecting them via its `AllowOnce` drain.
fn question_permission_options(options: &[String]) -> Vec<acp::PermissionOption> {
    options
        .iter()
        .enumerate()
        .filter_map(|(index, option)| {
            let text = option.trim();
            if text.is_empty() {
                return None;
            }
            Some(acp::PermissionOption::new(
                acp::PermissionOptionId::new(format!("question-{index}")),
                text.to_string(),
                acp::PermissionOptionKind::RejectOnce,
            ))
        })
        .collect()
}

fn claim_question_prompt(seen: &mut std::collections::HashSet<String>, question_id: &str) -> bool {
    seen.insert(question_id.to_string())
}

/// The failure text for a turn worker that died without a terminal event:
/// a panic names its payload, anything else is reported as an abort. This
/// is the difference between a visible failed turn and a silent clean one.
fn worker_join_failure(join_error: tokio::task::JoinError) -> String {
    let detail = match join_error.try_into_panic() {
        Ok(payload) => payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic payload".to_string()),
        Err(join_error) => format!("worker aborted: {join_error}"),
    };
    format!("turn worker died: {detail}")
}

fn question_permission_request(
    session_id: &acp::SessionId,
    question_id: &str,
    question: &str,
    options: &[String],
    allow_free_text: bool,
) -> Option<acp::RequestPermissionRequest> {
    let mut choices = question_permission_options(options);
    if choices.is_empty() {
        // A free-text-only ask still needs a dialog to type into: one row
        // that carries no answer of its own (its index maps past the empty
        // options list in `answer_from_permission_outcome`, so a bare Enter
        // resolves the dialog without fabricating an answer).
        if !allow_free_text {
            return None;
        }
        choices = vec![acp::PermissionOption::new(
            acp::PermissionOptionId::new("question-0"),
            "Answer in text".to_owned(),
            acp::PermissionOptionKind::RejectOnce,
        )];
    }
    Some(
        acp::RequestPermissionRequest::new(
            session_id.clone(),
            acp::ToolCallUpdate::new(
                acp::ToolCallId::new(question_id.to_string()),
                acp::ToolCallUpdateFields::new()
                    .title(question.to_string())
                    .kind(acp::ToolKind::Other)
                    .status(acp::ToolCallStatus::Pending),
            ),
            choices,
        )
        // The marker lets a client render choices by name (they all ride the
        // RejectOnce kind) and offer the free-text row only when allowed.
        .meta(
            serde_json::json!({
                QUESTION_META_KEY: { "allowFreeText": allow_free_text },
            })
            .as_object()
            .cloned(),
        ),
    )
}

/// A choice opens the permission dialog. Blank options do not.
fn question_has_choices(options: &[String]) -> bool {
    options.iter().any(|option| !option.trim().is_empty())
}

/// End restates a reply that already streamed. Emit it only when no deltas arrived.
fn should_emit_end_text(saw_text: bool, text: &str) -> bool {
    !saw_text && !text.is_empty()
}

/// How long an `request_permission` ask may park before the agent fails
/// closed. Default 30 minutes (the same budget `ask_user` gives an
/// unanswered questionnaire); `A3S_CODE_PERMISSION_ASK_TIMEOUT_SECS`
/// overrides, `0` disables the bound entirely.
fn ask_deadline() -> std::time::Duration {
    const DEFAULT_SECS: u64 = 1800;
    let configured = std::env::var("A3S_CODE_PERMISSION_ASK_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok());
    let secs = match configured {
        // Explicit disable: no bound.
        Some(0) => u64::MAX,
        Some(secs) => secs,
        None => DEFAULT_SECS,
    };
    std::time::Duration::from_secs(secs)
}

/// The `x.ai/turn/outcome` notification payload carrying how the run closed
/// at the completion gate, so the client's terminal marker can say more than
/// a duration. Fields are additive; clients ignore unknown keys.
fn turn_outcome_payload(
    terminal: &a3s_code_core::harness_loop::CompletionTerminal,
    summary: &a3s_code_core::verification::VerificationSummary,
    forced_stop: Option<&a3s_code_core::harness_loop::ForcedStop>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "sessionUpdate": "turn_outcome",
        "terminal": terminal_kind(terminal),
        "status": format!("{:?}", summary.status).to_lowercase(),
        "requiredChecks": summary.required_check_count,
        "failedChecks": summary.failed_check_count,
    });
    if let Some(forced) = forced_stop {
        payload["forced"] =
            serde_json::json!(a3s_code_core::harness_loop::ForcedStop::as_str(*forced));
    }
    payload
}

fn terminal_kind(terminal: &a3s_code_core::harness_loop::CompletionTerminal) -> &'static str {
    match terminal {
        a3s_code_core::harness_loop::CompletionTerminal::Narrative => "narrative",
        a3s_code_core::harness_loop::CompletionTerminal::Verified { .. } => "verified",
        a3s_code_core::harness_loop::CompletionTerminal::Waived { .. } => "waived",
        a3s_code_core::harness_loop::CompletionTerminal::Distinct => "distinct",
    }
}

/// A Core `AgentEvent::Error` is a failed turn. The pager already renders
/// `Err` as TurnFailed; `Ok(EndTurn)` would say the turn completed.
fn agent_failure_error(message: &str) -> acp::Error {
    acp::Error::internal_error().data(message.to_string())
}

/// Resolve which live session a sampling consent ask belongs to: the
/// session whose declared `mcp_servers` names the server owns it; a
/// baseline (config-declared) server belongs to whichever session is
/// live; no live session means nobody to ask (the caller refuses).
fn resolve_consent_session(
    sessions: &HashMap<String, LiveSession>,
    server: &str,
) -> Option<acp::SessionId> {
    sessions
        .iter()
        .find(|(_, live)| {
            live.mcp_servers
                .iter()
                .any(|declared| declared.name == server)
        })
        .map(|(key, _)| acp::SessionId::new(key.clone()))
        .or_else(|| {
            sessions
                .keys()
                .next()
                .map(|key| acp::SessionId::new(key.clone()))
        })
}

/// The composition the wiring contract pins: consent bridge INSIDE the
/// per-server ceiling, the model voice capped on generation.
fn sampling_handler_for(
    server: &str,
    llm: &std::sync::Arc<dyn a3s_code_core::llm::LlmClient>,
    model: &str,
    consent: &tokio::sync::mpsc::Sender<crate::sampling_consent::SamplingConsentAsk>,
) -> std::sync::Arc<dyn a3s_code_core::mcp::transport::SamplingHandler> {
    use a3s_code_core::mcp::client::{
        ApprovalGatedSampling, BoundedSamplingHandler, LlmSamplingHandler,
    };
    let voice = std::sync::Arc::new(LlmSamplingHandler::new(
        std::sync::Arc::clone(llm),
        model.to_string(),
    ));
    let bridge = std::sync::Arc::new(crate::sampling_consent::SamplingConsentBridge::new(
        consent.clone(),
    ));
    let gated = std::sync::Arc::new(ApprovalGatedSampling::new(
        server.to_string(),
        voice,
        bridge,
    ));
    std::sync::Arc::new(BoundedSamplingHandler::new(gated))
}

/// Serve parked sampling asks: resolve the owning session, ask through
/// the request_permission channel (Allow once / Deny), and settle the
/// reply. Every non-approval path refuses (the bridge also times out).
async fn serve_sampling_consent(
    mut asks: tokio::sync::mpsc::Receiver<crate::sampling_consent::SamplingConsentAsk>,
    client: Rc<RefCell<Option<Rc<acp::AgentSideConnection>>>>,
    sessions: Arc<Mutex<HashMap<String, LiveSession>>>,
) {
    const OPTIONS: [&str; 2] = ["Allow once", "Deny"];
    while let Some(ask) = asks.recv().await {
        let allowed = async {
            let owning = {
                let sessions = sessions.lock().await;
                resolve_consent_session(&sessions, &ask.server)
            };
            // No session to ask and no live connection both refuse.
            let session_id = owning?;
            let conn = client.borrow().clone();
            let conn = conn?;
            let options: Vec<String> = OPTIONS.iter().map(|text| text.to_string()).collect();
            let question_id = format!("mcp-sampling-{}", ask.server);
            // Sampling consent is a closed yes/no; free text is never consent.
            let request = question_permission_request(
                &session_id,
                &question_id,
                &ask.question,
                &options,
                /*allow_free_text=*/ false,
            )?;
            let response = conn.request_permission(request).await.ok()?;
            let answer = if followup_message(&response).is_some() {
                // Typed text means the user answered in prose; the picked row
                // then names nothing trustworthy, and "Allow once" on it must
                // not grant sampling.
                None
            } else {
                answer_from_permission_outcome(&response.outcome, &options)
            };
            answer.as_deref().map(|answer| answer == "Allow once")
        }
        .await
        .unwrap_or(false);
        let _ = ask.reply.send(allowed);
    }
}

fn answer_from_permission_outcome(
    outcome: &acp::RequestPermissionOutcome,
    options: &[String],
) -> Option<String> {
    let acp::RequestPermissionOutcome::Selected(selected) = outcome else {
        return None;
    };
    let index = selected
        .option_id
        .0
        .as_ref()
        .strip_prefix("question-")?
        .parse::<usize>()
        .ok()?;
    let text = options.get(index)?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) use tests::test_launch;

#[cfg(test)]
mod session_catalog_tests;

#[cfg(test)]
mod goal_tests;

#[cfg(test)]
mod resume_tests;

#[cfg(test)]
mod session_setup_tests;

#[cfg(test)]
mod engineering_tests;

#[cfg(test)]
mod review_tests;

#[cfg(test)]
#[path = "agent_plugin_process_tests.rs"]
mod plugin_process_tests;

#[cfg(test)]
#[path = "document_read_agent_tests.rs"]
mod document_read_agent_tests;

#[cfg(test)]
#[path = "run_observation_agent_tests.rs"]
mod run_observation_agent_tests;
