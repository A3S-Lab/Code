use super::*;

/// A startup sub-phase routed to its own slot, so a producer timer's field is chosen by the enum rather than a string match.
#[derive(Clone, Copy, PartialEq, Eq, Debug, strum::IntoStaticStr, strum::EnumIter)]
#[strum(serialize_all = "snake_case")]
pub enum Subphase {
    SessionLoad,
    SessionReplay,
    SessionGitScan,
    SessionSpawn,
    InitProcess,
    ResolveConfig,
    RemoteSettings,
    ModelsManager,
    ManagedPolicyAuthWait,
    ManagedPolicyConfigSync,
}

/// Per-subphase timings keyed by [`Subphase`], plus the gaps that are not subphases.
#[derive(Clone, Copy)]
pub(crate) struct SubphaseTimings {
    session_load_ms: Option<u64>,
    session_replay_ms: Option<u64>,
    session_git_scan_ms: Option<u64>,
    session_spawn_ms: Option<u64>,
    init_process_ms: Option<u64>,
    resolve_config_ms: Option<u64>,
    remote_settings_ms: Option<u64>,
    models_manager_ms: Option<u64>,
    managed_policy_auth_wait_ms: Option<u64>,
    managed_policy_config_sync_ms: Option<u64>,
    pub(crate) prefetch_wait_ms: Option<u64>,
    pub(crate) time_to_first_frame_ms: Option<u64>,
    // The reported total; on the no-session path it lands before the frame, so `record_interactive_frame` pairs with it.
    pub(crate) startup_total_ms: Option<u64>,
}

impl SubphaseTimings {
    const fn new() -> Self {
        Self {
            session_load_ms: None,
            session_replay_ms: None,
            session_git_scan_ms: None,
            session_spawn_ms: None,
            init_process_ms: None,
            resolve_config_ms: None,
            remote_settings_ms: None,
            models_manager_ms: None,
            managed_policy_auth_wait_ms: None,
            managed_policy_config_sync_ms: None,
            prefetch_wait_ms: None,
            time_to_first_frame_ms: None,
            startup_total_ms: None,
        }
    }

    pub(crate) fn get(&self, sp: Subphase) -> Option<u64> {
        match sp {
            Subphase::SessionLoad => self.session_load_ms,
            Subphase::SessionReplay => self.session_replay_ms,
            Subphase::SessionGitScan => self.session_git_scan_ms,
            Subphase::SessionSpawn => self.session_spawn_ms,
            Subphase::InitProcess => self.init_process_ms,
            Subphase::ResolveConfig => self.resolve_config_ms,
            Subphase::RemoteSettings => self.remote_settings_ms,
            Subphase::ModelsManager => self.models_manager_ms,
            Subphase::ManagedPolicyAuthWait => self.managed_policy_auth_wait_ms,
            Subphase::ManagedPolicyConfigSync => self.managed_policy_config_sync_ms,
        }
    }

    fn set(&mut self, sp: Subphase, ms: u64) {
        let slot = match sp {
            Subphase::SessionLoad => &mut self.session_load_ms,
            Subphase::SessionReplay => &mut self.session_replay_ms,
            Subphase::SessionGitScan => &mut self.session_git_scan_ms,
            Subphase::SessionSpawn => &mut self.session_spawn_ms,
            Subphase::InitProcess => &mut self.init_process_ms,
            Subphase::ResolveConfig => &mut self.resolve_config_ms,
            Subphase::RemoteSettings => &mut self.remote_settings_ms,
            Subphase::ModelsManager => &mut self.models_manager_ms,
            Subphase::ManagedPolicyAuthWait => &mut self.managed_policy_auth_wait_ms,
            Subphase::ManagedPolicyConfigSync => &mut self.managed_policy_config_sync_ms,
        };
        // InitProcess is a once-per-process gap so the first write wins; the others re-stamp per attempt.
        if matches!(sp, Subphase::InitProcess) {
            slot.get_or_insert(ms);
        } else {
            *slot = Some(ms);
        }
    }

    /// Builds the wire event through the per-subphase accessor; `get`'s exhaustive
    /// match fails to compile until a new [`Subphase`] is routed to its field.
    pub(crate) fn startup_completed(
        &self,
        total_ms: u64,
        outcome: StartupOutcome,
        phases: String,
        auth_mode: AuthMode,
    ) -> crate::events::StartupCompleted {
        crate::events::StartupCompleted {
            total_ms,
            outcome,
            phases,
            auth_mode,
            prefetch_wait_ms: self.prefetch_wait_ms,
            session_load_ms: self.get(Subphase::SessionLoad),
            session_replay_ms: self.get(Subphase::SessionReplay),
            session_git_scan_ms: self.get(Subphase::SessionGitScan),
            session_spawn_ms: self.get(Subphase::SessionSpawn),
            init_process_ms: self.get(Subphase::InitProcess),
            resolve_config_ms: self.get(Subphase::ResolveConfig),
            remote_settings_ms: self.get(Subphase::RemoteSettings),
            models_manager_ms: self.get(Subphase::ModelsManager),
            managed_policy_auth_wait_ms: self.get(Subphase::ManagedPolicyAuthWait),
            managed_policy_config_sync_ms: self.get(Subphase::ManagedPolicyConfigSync),
            time_to_first_frame_ms: self.time_to_first_frame_ms,
        }
    }
}

impl Default for SubphaseTimings {
    fn default() -> Self {
        Self::new()
    }
}

static SUBPHASES: Mutex<SubphaseTimings> = Mutex::new(SubphaseTimings::new());

pub(crate) fn subphases() -> std::sync::MutexGuard<'static, SubphaseTimings> {
    SUBPHASES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Routes a `startup.*` sub-timing to the current attempt's own buffer; a no-op with no current attempt.
/// Not gated on DONE: the gate was what dropped sub-timers on warm runs.
pub fn record_sub_timing(name: &str, elapsed: Duration) {
    let Some(key) = name.strip_prefix("startup.") else {
        return;
    };
    if let Some(timer) = current() {
        timer.push_sub_timing(key, elapsed);
    }
}

pub fn record_prefetch_wait(elapsed: Duration) {
    subphases().prefetch_wait_ms = Some(duration_ms(elapsed));
}

pub(crate) fn record_subphase(sp: Subphase, elapsed: Duration) {
    subphases().set(sp, duration_ms(elapsed));
}
