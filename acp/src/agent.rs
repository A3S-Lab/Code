//! `acp::Agent` backed by a3s-code-core fact-log sessions.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use a3s_code_core::{Agent, AgentEvent, AgentSession, SessionOptions};
use agent_client_protocol as acp;
use agent_client_protocol::Client;
use tokio::sync::{watch, Mutex, OwnedMutexGuard};
use tokio::task::JoinHandle;

use crate::config::LaunchConfig;

mod client_fs;
mod commands;
mod emit;
mod extensions;
mod goal_ext;
mod host_methods;
mod models;
mod permission_ask;
mod resume;
mod sampling;
mod session_catalog;
mod session_open;
mod tool_render;
mod turn;
mod unattended_turn;
mod workspace_scope;

use emit::{context_usage, note_child_progress};
#[cfg(test)]
use emit::{progress_duration_ms, progress_tokens_used, subagent_finished_update};
use extensions::{answer_wiki_search, product_prompt_turns};
#[cfg(test)]
use extensions::{parse_interject_params, recorded_session_homes, remove_recorded_session_under};
use host_methods::HostMethod;
use models::{effort_guideline, effort_limits, effort_limits_from_meta, EffortLimits};
#[cfg(test)]
use models::{reasoning_effort_catalog_meta, reasoning_effort_token};
#[cfg(test)]
use permission_ask::tool_permission_request;
use permission_ask::{
    answer_from_permission_outcome, claim_question_prompt, followup_message, question_has_choices,
    question_permission_request,
};
#[cfg(test)]
use permission_ask::{
    permission_response_always, permission_response_approved, question_permission_options,
};
#[cfg(test)]
use sampling::resolve_consent_session;
use sampling::serve_sampling_consent;
#[cfg(test)]
use session_open::union_mcp_desired;
use session_open::{acp_mcp_to_config, session_mcp_configs};
#[cfg(test)]
use tool_render::edit_diff;
use tool_render::{
    prepared_raw_input, streaming_bash_delta, task_to_plan_entry, tool_call_content, tool_kind_for,
    tool_raw_output, tool_title,
};
use turn::{agent_failure_error, worker_join_failure};
#[cfg(test)]
use turn::{should_emit_end_text, turn_outcome_payload};

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

/// Sessions whose `session/load` has started and not yet inserted a live
/// session. `x.ai/session/info` waits here. A lookup that runs first answers
/// `unknown session id` while the pager is still on "Loading session".
struct InflightSessionLoads {
    open: std::sync::Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl InflightSessionLoads {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            open: std::sync::Mutex::new(HashMap::new()),
        })
    }

    fn begin(self: &Arc<Self>, session_id: &str) -> SessionLoadGuard {
        let mut open = self.open.lock().unwrap_or_else(|err| err.into_inner());
        if open.contains_key(session_id) {
            return SessionLoadGuard {
                loads: None,
                id: String::new(),
            };
        }
        let (sender, _) = watch::channel(false);
        open.insert(session_id.to_string(), sender);
        SessionLoadGuard {
            loads: Some(Arc::clone(self)),
            id: session_id.to_string(),
        }
    }

    async fn wait(&self, session_id: &str) {
        let mut receiver = {
            let open = self.open.lock().unwrap_or_else(|err| err.into_inner());
            let Some(sender) = open.get(session_id) else {
                return;
            };
            sender.subscribe()
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            if *receiver.borrow() {
                return;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return;
            }
            match tokio::time::timeout(deadline - now, receiver.changed()).await {
                Ok(Ok(())) => {}
                _ => return,
            }
        }
    }

    fn publish(&self, session_id: &str) {
        let open = self.open.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(sender) = open.get(session_id) {
            let _ = sender.send(true);
        }
    }

    fn remove(&self, session_id: &str) {
        self.open
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(session_id);
    }
}

/// Drops the in-flight marker on every `load_session` exit, including `?`.
struct SessionLoadGuard {
    loads: Option<Arc<InflightSessionLoads>>,
    id: String,
}

impl SessionLoadGuard {
    fn ready(&self) {
        if let Some(loads) = &self.loads {
            loads.publish(&self.id);
        }
    }
}

impl Drop for SessionLoadGuard {
    fn drop(&mut self) {
        if let Some(loads) = self.loads.take() {
            loads.remove(&self.id);
        }
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
fn session_capability_note(
    semantic: bool,
    hooks: usize,
    pending_hooks: usize,
    mcp_servers: usize,
    memory: &str,
) -> String {
    let retrieval = if semantic {
        "search modes: grep, glob, bm25, semantic, hybrid.\n\
         If the index is degraded, semantic and hybrid leave the tool definition."
    } else {
        "search modes: grep, glob, bm25.\n语义检索关闭。"
    };
    format!(
        "Session capabilities:\n\
         {retrieval}\n\
         code_symbols and code_navigation are available. The language server starts on the first code_symbols or code_navigation call.\n\
         memory: {memory}.\n\
         hooks: {hooks}. pending hooks: {pending_hooks}. mcp: {mcp_servers}."
    )
}

/// Production embedding seam. Nothing is wired, so semantic retrieval stays off.
/// shortcut: no HTTP, ONNX, or local model; return a provider here when one exists.
fn session_embedding_provider() -> Option<Arc<dyn a3s_code_core::EmbeddingProvider>> {
    None
}

fn session_memory_dir(workspace: &std::path::Path, config: &a3s_code_core::CodeConfig) -> PathBuf {
    config
        .memory_dir
        .clone()
        .unwrap_or_else(|| a3s_code_core::state_root::workspace_state_dir(workspace).join("memory"))
}

fn discovered_command_hooks(
    workspace: &std::path::Path,
    home: Option<&std::path::Path>,
    trust_path: &std::path::Path,
) -> (
    Option<Arc<a3s_code_core::hooks::CommandHookExecutor>>,
    usize,
    usize,
) {
    match a3s_code_core::hooks::CommandHookExecutor::discover(
        workspace,
        home,
        trust_path.to_path_buf(),
    ) {
        Ok(executor) => {
            let (hooks, pending) = executor.discovered_and_pending();
            (Some(executor), hooks, pending)
        }
        Err(error) => {
            tracing::error!(%error, "command hooks were not loaded; this session has no hook executor");
            (None, 0, 0)
        }
    }
}

fn append_slot_extra(slots: &mut a3s_code_core::SystemPromptSlots, body: &str) {
    if body.is_empty() {
        return;
    }
    slots.extra = Some(match slots.extra.take() {
        Some(existing) if !existing.is_empty() => format!("{existing}\n\n{body}"),
        _ => body.to_string(),
    });
}

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
    /// Load markers for sessions not yet in `sessions`.
    session_loads: Arc<InflightSessionLoads>,
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
            session_loads: InflightSessionLoads::new(),
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

    #[allow(clippy::too_many_arguments)]
    fn host_session_options_with_hook_roots(
        options: SessionOptions,
        slots: &mut a3s_code_core::SystemPromptSlots,
        setup: &SessionSetup<'_>,
        config: &a3s_code_core::CodeConfig,
        session_store_dir: &std::path::Path,
        hook_home: Option<&std::path::Path>,
        trust_path: Option<&std::path::Path>,
        embedding: Option<Arc<dyn a3s_code_core::EmbeddingProvider>>,
    ) -> acp::Result<SessionOptions> {
        let (hook_executor, hook_count, pending_hooks) = match trust_path {
            Some(path) => discovered_command_hooks(&setup.workspace, hook_home, path),
            None => (None, 0, 0),
        };
        append_slot_extra(
            slots,
            &session_capability_note(
                embedding.is_some(),
                hook_count,
                pending_hooks,
                setup.mcp_servers.len(),
                "耐久集合",
            ),
        );
        let contracts = config
            .verification_contracts_for_workspace(&setup.workspace)
            .map_err(|error| acp::Error::invalid_params().data(error.to_string()))?;
        let guardrail = a3s_code_core::permissions::InteractiveToolGuardrail::default()
            .with_workspace(&setup.workspace);
        let mut options = options
            .with_prompt_slots(slots.clone())
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
        if let Some(executor) = hook_executor {
            options = options.with_hook_executor(executor);
        }
        options = options.with_memory(Arc::new(
            a3s_code_core::durable_memory_vec::LazyFileMemoryStore::new(session_memory_dir(
                &setup.workspace,
                config,
            )),
        ));
        if let Some(provider) = embedding {
            options = options
                .with_workspace_retrieval(a3s_code_core::WorkspaceRetrievalOptions::new(provider));
        }
        Ok(options.with_file_session_store(session_store_dir))
    }

    /// Fact counts and token totals for one live session.
    async fn snapshot_session_info(
        &self,
        session_id: &str,
    ) -> Option<(String, String, String, u64, u64, u64)> {
        let sessions = self.sessions.lock().await;
        let live = sessions.get(session_id)?;
        // The fact scheduler increments its turn only on `user.message`.
        // Tool rounds and assistant tool lines are other fact kinds, and
        // the folded transcript stores those lines with role=user.
        let turns = product_prompt_turns(live.workspace.as_path(), session_id);
        let (prompt_tokens, completion_tokens, total_tokens) = live.session.recorded_usage_tokens();
        let used = if total_tokens > 0 {
            total_tokens
        } else {
            prompt_tokens.saturating_add(completion_tokens)
        };
        let total = live.session.context_window_tokens() as u64;
        Some((
            live.workspace.display().to_string(),
            live.model_id.clone(),
            live.posture.clone(),
            turns,
            used,
            total,
        ))
    }
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

#[async_trait::async_trait(?Send)]
impl acp::Agent for A3sCodeAgent {
    async fn initialize(
        &self,
        args: acp::InitializeRequest,
    ) -> acp::Result<acp::InitializeResponse> {
        self.client_reads_files
            .set(args.client_capabilities.fs.read_text_file);
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
        let mut host = crate::host_context::studio_host_from_meta(args.meta.as_ref())
            .map_err(|error| acp::Error::invalid_params().data(error))?;
        if !args
            .meta
            .as_ref()
            .is_some_and(|meta| meta.contains_key("a3sEngineering"))
        {
            host.work_policy = Some(
                crate::engineering::named_policy("engineering-v1")
                    .map_err(|error| acp::Error::internal_error().data(error))?,
            );
        }
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
        let loading = self.session_loads.begin(&session_id);
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
        loading.ready();
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
        match HostMethod::parse(args.method.as_ref()) {
            Some(method) => self.answer_host_method(method, &args).await,
            None => Err(acp::Error::method_not_found().data(args.method.to_string())),
        }
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

/// Host budget levels offered when the provider has no native effort parameter.
/// `ultracode` stays off the menu: it is not a `ReasoningEffort` variant.
pub(crate) const HOST_EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Always-on turn boundary. The skill command is an example, not a matcher.
const SESSION_OUTCOME_BOUNDARY: &str = "\
Finish the user's outcome in this turn. Do not read or explain the product \
implementation unless the user asked to change that implementation. When a \
host command already does the outcome, name that command and stop. Example: \
the user installs a skill with `a3s code skills add <git-url>`.";

const HOST_PARALLEL_TASKS: usize = 8;
const HOST_CONTINUATION_TURNS: u32 = 8;

/// Meta key marking a `request_permission` as an agent question riding the
/// permission channel, not a permission ask; the payload carries
/// `{"allowFreeText": bool, "hasChoices": bool}`. Twin of `QUESTION_META_KEY` in
/// `vendor/a3s-code-ui/crates/codegen/a3s-code-workspace/src/permission/types.rs`
/// (this crate does not depend on the vendored workspace crate; keep them in sync).
const QUESTION_META_KEY: &str = "question";

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
mod retrieval_seam_tests;

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
