//! Named startup phases on a per-process timer, reported once to `unified.jsonl`, product events, and OTLP metrics.
//! A closed schema with pinned metric keys: time anything else with a `tracing` span, or give it its own schema.
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
mod frame_gap;
mod slow_phase;
mod subphase;
mod tokens;
pub(crate) use self::frame_gap::{
    PROCESS_INIT_RECORDED, close_first_frame_span, note_phase_end, open_first_frame_span,
    record_first_frame_gap, record_launch_gap, reset_frame_gap,
};
pub(crate) use self::slow_phase::{spawn_slow_phase_warnings, warn_over_budget};
#[cfg(test)]
pub(crate) use self::subphase::SubphaseTimings;
pub use self::subphase::{Subphase, record_prefetch_wait, record_sub_timing};
pub(crate) use self::subphase::{record_subphase, subphases};
pub use self::tokens::{PendingStartup, PhaseScope, ReadinessBudget, phase_scope};
/// `unified.jsonl` message keys, exported so consumers (the probe, tests) grep for the same strings this module writes.
pub const STARTUP_PHASE_MSG: &str = "startup phase";
pub const CONNECT_FINISHED_MSG: &str = "connect finished";
pub const STARTUP_COMPLETE_MSG: &str = "startup complete";
pub const STARTUP_INTERACTIVE_MSG: &str = "startup interactive";
pub const STARTUP_TIMING_MSG: &str = "startup timing";
pub const STARTUP_SLOW_PHASE_MSG: &str = "startup phase running long";
pub const STARTUP_OVER_BUDGET_MSG: &str = "startup phase over budget";
/// A launcher stamps the wall-clock spawn time, capturing the gap before our own clock starts.
pub const SPAWN_TIMESTAMP_ENV: &str = "GROK_SPAWN_TIMESTAMP_MS";
/// Benchmarks set this to stop right after the first confirmed frame.
pub const EXIT_AFTER_FIRST_RENDER_ENV: &str = "GROK_EXIT_AFTER_FIRST_RENDER";
#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::AsRefStr, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum StartupPhase {
    ConfigLoad,
    ManagedPolicy,
    Bootstrap,
    ModelCatalog,
    WorkerSpawn,
    LeaderConnect,
    AcpInitialize,
    EagerAuth,
    AppInit,
    SessionCreate,
}
impl StartupPhase {
    pub fn label(self) -> &'static str {
        self.into()
    }
    /// Regression-gate ceilings, not SLOs: they only flag a phase that grew far past its usual cost.
    fn budget(self) -> Duration {
        use StartupPhase::*;
        match self {
            ConfigLoad | ModelCatalog | WorkerSpawn | AppInit => Duration::from_secs(1),
            ManagedPolicy | EagerAuth => Duration::from_secs(2),
            AcpInitialize => Duration::from_secs(6),
            Bootstrap | LeaderConnect | SessionCreate => Duration::from_secs(3),
        }
    }
}
macro_rules! span_table {
    ($visibility:vis fn $name:ident, fn $under:ident($enum_name:ident) { $($variant:ident => $label:literal),* $(,)? }) => {
        $visibility fn $name(value: $enum_name) -> tracing::Span {
            match value {
                $($enum_name::$variant => tracing::info_span!($label),)*
            }
        }
        $visibility fn $under(value: $enum_name, parent: &tracing::Span) -> tracing::Span {
            match value {
                $($enum_name::$variant => tracing::info_span!(parent: parent, $label),)*
            }
        }
    };
    ($visibility:vis fn $name:ident($enum_name:ident, parent) { $($variant:ident => $label:literal),* $(,)? }) => {
        $visibility fn $name(value: $enum_name, parent: &tracing::Span) -> tracing::Span {
            match value {
                $($enum_name::$variant => tracing::info_span!(parent: parent, $label),)*
            }
        }
    };
}
pub(crate) use span_table;
span_table!(fn phase_span(StartupPhase, parent) {
    ConfigLoad => "startup.config_load",
    ManagedPolicy => "startup.managed_policy",
    Bootstrap => "startup.bootstrap",
    ModelCatalog => "startup.model_catalog",
    WorkerSpawn => "startup.worker_spawn",
    LeaderConnect => "startup.leader_connect",
    AcpInitialize => "startup.acp_initialize",
    EagerAuth => "startup.eager_auth",
    AppInit => "startup.app_init",
    SessionCreate => "startup.session_create",
});
span_table!(pub(crate) fn subphase_span(Subphase, parent) {
    SessionLoad => "startup.session_load",
    SessionReplay => "startup.session_replay",
    SessionGitScan => "startup.session_git_scan",
    SessionSpawn => "startup.session_spawn",
    InitProcess => "startup.bootstrap.init_process",
    ResolveConfig => "startup.bootstrap.resolve_config",
    RemoteSettings => "startup.bootstrap.remote_settings",
    ModelsManager => "startup.model_catalog.models_manager",
    ManagedPolicyAuthWait => "startup.managed_policy.auth_wait",
    ManagedPolicyConfigSync => "startup.managed_policy.config_sync",
});
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, strum::AsRefStr, strum::IntoStaticStr, serde::Serialize,
)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum StartupOutcome {
    Ok,
    Timeout,
    Cancelled,
    Error,
}
impl StartupOutcome {
    pub fn label(self) -> &'static str {
        self.into()
    }
}
/// Why the root span closes, so every close site names a reason instead of a bare string.
#[derive(Clone, Copy)]
pub(crate) enum RootClose {
    Reported(StartupOutcome),
    Superseded,
    Abandoned,
    Served,
    #[cfg(test)]
    Discarded,
}
impl RootClose {
    fn label(self) -> &'static str {
        match self {
            RootClose::Reported(outcome) => outcome.label(),
            RootClose::Superseded => "superseded",
            RootClose::Abandoned => "abandoned",
            RootClose::Served => "served",
            #[cfg(test)]
            RootClose::Discarded => "discarded",
        }
    }
    /// Only a reported or served attempt reaches its ready point, so only then does closing its open phase end
    /// the last phase and start the first-frame gap. Superseded and abandoned attempts are interruptions.
    fn ends_last_phase(self) -> bool {
        matches!(self, RootClose::Reported(_) | RootClose::Served)
    }
}
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    strum::AsRefStr,
    strum::IntoStaticStr,
    serde::Serialize,
)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    #[default]
    Unknown,
    Personal,
    Team,
    Deployment,
}
impl AuthMode {
    pub fn label(self) -> &'static str {
        self.into()
    }
}
/// Who reports the timer, so an embedded run does not report it twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Client,
    Agent,
}
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, strum::AsRefStr, strum::IntoStaticStr, serde::Serialize,
)]
#[strum(serialize_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Embedded,
    Leader,
}
impl AgentKind {
    pub fn label(self) -> &'static str {
        self.into()
    }
}
#[derive(Clone, Debug)]
pub struct PhaseSnapshot {
    pub completed: Vec<(StartupPhase, Duration)>,
    pub open: Option<(StartupPhase, Duration)>,
}
impl PhaseSnapshot {
    pub fn stuck_in(&self) -> &'static str {
        self.open.map_or("unknown", |(phase, _)| phase.label())
    }
    /// The slowest phase so far, including the one still open.
    pub fn longest_step(&self) -> Option<StartupPhase> {
        self.completed
            .iter()
            .copied()
            .chain(self.open)
            .max_by_key(|(_, elapsed)| *elapsed)
            .map(|(phase, _)| phase)
    }
    fn over_budget(&self) -> Vec<(StartupPhase, Duration)> {
        self.completed
            .iter()
            .copied()
            .chain(self.open)
            .filter(|(phase, elapsed)| *elapsed > phase.budget())
            .collect()
    }
    /// Completed phases read `phase=dur`; the open one reads `phase>=dur`.
    pub fn summary(&self) -> String {
        if self.completed.is_empty() && self.open.is_none() {
            return "no phases entered".to_string();
        }
        let mut out = String::new();
        for (phase, d) in &self.completed {
            if !out.is_empty() {
                out.push_str(", ");
            }
            let _ = write!(out, "{}={}", phase.label(), format_duration(*d));
        }
        if let Some((phase, open)) = self.open {
            if !out.is_empty() {
                out.push_str(", ");
            }
            let _ = write!(out, "{}>={}", phase.label(), format_duration(open));
        }
        out
    }
}
struct Inner {
    completed: Vec<(StartupPhase, Duration)>,
    current: Option<(StartupPhase, Instant)>,
    current_span: Option<tracing::Span>,
    root_span: Option<tracing::Span>,
    auth_mode: AuthMode,
    owner: Owner,
}
impl Inner {
    /// Takes the root and any open-phase span to drop after the lock; `None` once the root has already closed.
    fn take_spans_to_close(&mut self) -> Option<(tracing::Span, Option<tracing::Span>)> {
        let root = self.root_span.take()?;
        Some((root, self.current_span.take()))
    }
}
pub struct StartupTimer {
    started: Instant,
    inner: Mutex<Inner>,
    /// This attempt's `startup.*` sub-timers, owned so they cannot bleed into another attempt.
    sub_timers: Mutex<Vec<(String, u64)>>,
    /// Drains this attempt's sub-timers at most once, whichever site reaches it first.
    sub_timers_drained: AtomicBool,
}
impl StartupTimer {
    pub(crate) fn new(owner: Owner) -> Self {
        Self {
            started: Instant::now(),
            inner: Mutex::new(Inner {
                completed: Vec::new(),
                current: None,
                current_span: None,
                root_span: Some(tracing::info_span!(
                    "startup",
                    outcome = tracing::field::Empty
                )),
                auth_mode: AuthMode::Unknown,
                owner,
            }),
            sub_timers: Mutex::new(Vec::new()),
            sub_timers_drained: AtomicBool::new(false),
        }
    }
    fn push_sub_timing(&self, key: &str, elapsed: Duration) {
        self.sub_timers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((key.to_owned(), duration_ms(elapsed)));
    }
    /// Drains this attempt's sub-timers exactly once and emits them with `outcome` and this attempt's auth mode.
    fn drain_sub_timers(&self, outcome: StartupOutcome) {
        if self.sub_timers_drained.swap(true, Ordering::Relaxed) {
            return;
        }
        let timings: Vec<(String, u64)> = {
            let mut buf = self.sub_timers.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut *buf)
        };
        if timings.is_empty() {
            return;
        }
        crate::session_ctx::log_event(crate::events::StartupSubTimers {
            timings,
            outcome,
            auth_mode: self.auth_mode(),
        });
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Closes the open phase; re-entering the open phase is ignored, so two layers can name the same step and it is measured once.
    /// Returns whether a prior phase closed, so the caller can stamp the phase boundary.
    pub fn enter(&self, phase: StartupPhase) -> bool {
        let now = Instant::now();
        let finished_span;
        let closed_prev;
        let root_span;
        {
            let mut g = self.lock();
            let Some(root) = g.root_span.clone() else {
                return false;
            };
            if matches!(g.current, Some((open, _)) if open == phase) {
                return false;
            }
            closed_prev = g.current.is_some();
            if let Some((prev, t0)) = g.current.take() {
                g.completed.push((prev, now.saturating_duration_since(t0)));
            }
            g.current = Some((phase, now));
            finished_span = g.current_span.take();
            root_span = root;
        }
        drop(finished_span);
        let span = phase_span(phase, &root_span);
        let orphaned = {
            let mut g = self.lock();
            if g.root_span.is_some() {
                g.current_span = Some(span);
                None
            } else {
                g.current = None;
                Some(span)
            }
        };
        drop(orphaned);
        let elapsed_ms = duration_ms(self.started.elapsed());
        tracing::info!(phase = %phase.label(), elapsed_ms, "startup phase");
        crate::unified_log::info(
            STARTUP_PHASE_MSG,
            None,
            Some(serde_json::json!({ "phase": phase.label(), "elapsed_ms": elapsed_ms })),
        );
        closed_prev
    }
    fn close_open_phase(&self) {
        let now = Instant::now();
        let finished_span;
        let closed;
        {
            let mut g = self.lock();
            closed = g.current.take();
            if let Some((prev, t0)) = closed {
                g.completed.push((prev, now.saturating_duration_since(t0)));
            }
            finished_span = g.current_span.take();
        }
        drop(finished_span);
        if closed.is_some() {
            note_phase_end();
        }
    }
    /// Closes the root span exactly once, recording `outcome`, and closes the open phase span with it.
    /// The record and the drops run after the lock, so subscriber hooks never fire while the guard is held.
    fn close_root_span(&self, reason: RootClose) {
        let Some((root, open_phase)) = self.lock().take_spans_to_close() else {
            return;
        };
        root.record("outcome", reason.label());
        let closed_phase = open_phase.is_some();
        drop(open_phase);
        drop(root);
        if closed_phase && reason.ends_last_phase() {
            note_phase_end();
        }
    }
    pub fn set_auth_mode(&self, mode: AuthMode) {
        self.lock().auth_mode = mode;
    }
    pub fn auth_mode(&self) -> AuthMode {
        self.lock().auth_mode
    }
    pub fn owner(&self) -> Owner {
        self.lock().owner
    }
    fn open_phase_age(&self) -> Option<(StartupPhase, Duration)> {
        self.lock().current.map(|(p, t0)| (p, t0.elapsed()))
    }
    /// Clones the root span so an off-timer span can parent onto it; `None` once the root has closed.
    pub(crate) fn root_span(&self) -> Option<tracing::Span> {
        self.lock().root_span.clone()
    }
    /// One read, so a caller reporting several facts can't mix moments.
    pub fn phase_snapshot(&self) -> PhaseSnapshot {
        let now = Instant::now();
        let g = self.lock();
        PhaseSnapshot {
            completed: g.completed.clone(),
            open: g
                .current
                .map(|(phase, t0)| (phase, now.saturating_duration_since(t0))),
        }
    }
    pub fn summary(&self) -> String {
        self.phase_snapshot().summary()
    }
    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
    pub fn phase_durations_ms(&self) -> BTreeMap<String, u64> {
        let now = Instant::now();
        let g = self.lock();
        let mut map: BTreeMap<String, u64> = BTreeMap::new();
        for (phase, d) in &g.completed {
            *map.entry(phase.label().to_owned()).or_default() += duration_ms(*d);
        }
        if let Some((phase, t0)) = g.current {
            *map.entry(phase.label().to_owned()).or_default() +=
                duration_ms(now.saturating_duration_since(t0));
        }
        map
    }
    pub fn emit_telemetry(
        &self,
        connect_target: AgentKind,
        outcome: StartupOutcome,
        timeout_secs: Option<u64>,
        embedded_fallback: bool,
    ) {
        if outcome == StartupOutcome::Ok {
            self.close_open_phase();
        }
        let timings = self.phase_snapshot();
        let stuck_in = (outcome == StartupOutcome::Timeout).then(|| timings.stuck_in().to_string());
        let phases = timings.summary();
        let elapsed_ms = duration_ms(self.elapsed());
        let ctx = serde_json::json!({
            "connect_target": connect_target,
            "outcome": outcome,
            "stuck_in": stuck_in,
            "phases": phases,
            "elapsed_ms": elapsed_ms,
            "auth_mode": self.auth_mode(),
        });
        crate::unified_log::info(CONNECT_FINISHED_MSG, None, Some(ctx));
        crate::session_ctx::log_event(crate::events::AgentConnect {
            connect_target,
            outcome,
            stuck_in,
            phases,
            phase_durations_ms: self.phase_durations_ms(),
            elapsed_ms,
            timeout_secs,
            embedded_fallback,
            auth_mode: self.auth_mode(),
        });
        if outcome != StartupOutcome::Ok {
            self.drain_sub_timers(outcome);
        }
    }
}
static CURRENT: Mutex<Option<Arc<StartupTimer>>> = Mutex::new(None);
static DONE: AtomicBool = AtomicBool::new(false);
static PROCESS_START: LazyLock<Instant> = LazyLock::new(Instant::now);
static INTERACTIVE: AtomicBool = AtomicBool::new(false);
static STARTUP_AUTH_MODE: Mutex<AuthMode> = Mutex::new(AuthMode::Unknown);
/// The first paint. Stamps time-to-first-frame once and opens the settle span; the acknowledged
/// frame that ends the launch is [`record_interactive_frame`].
pub fn record_first_draw() {
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    let elapsed_ms = duration_ms(process_elapsed());
    let first_draw = {
        let mut sub = subphases();
        let first_draw = sub.time_to_first_frame_ms.is_none();
        if first_draw {
            sub.time_to_first_frame_ms = Some(elapsed_ms);
        }
        first_draw
    };
    if first_draw {
        open_first_frame_span();
    }
}
/// Records process start to the first confirmed frame once; returns whether this call recorded it.
pub fn record_interactive_frame() -> bool {
    if INTERACTIVE.swap(true, Ordering::Relaxed) {
        return false;
    }
    close_first_frame_span();
    if let Some(timer) = current() {
        timer.close_open_phase();
        timer.close_root_span(RootClose::Reported(StartupOutcome::Ok));
    }
    record_first_frame_gap();
    let event = crate::events::StartupInteractive {
        interactive_ms: duration_ms(process_elapsed()),
        startup_total_ms: subphases().startup_total_ms,
        spawn_to_first_frame_ms: spawn_to_first_frame_ms(),
        auth_mode: *STARTUP_AUTH_MODE.lock().unwrap_or_else(|e| e.into_inner()),
    };
    if let Ok(record) = serde_json::to_value(&event) {
        crate::unified_log::info(STARTUP_INTERACTIVE_MSG, None, Some(record));
    }
    crate::session_ctx::log_event(event);
    true
}
/// Whether a benchmark asked to quit after the first confirmed frame.
pub fn exit_after_first_render() -> bool {
    std::env::var(EXIT_AFTER_FIRST_RENDER_ENV).is_ok_and(|v| !v.is_empty() && v != "0")
}
/// Wall-clock milliseconds from the launcher-stamped spawn time to now, if stamped.
fn spawn_to_first_frame_ms() -> Option<u64> {
    let spawn_ms: u64 = std::env::var(SPAWN_TIMESTAMP_ENV).ok()?.parse().ok()?;
    let now_ms = duration_ms(SystemTime::now().duration_since(UNIX_EPOCH).ok()?);
    now_ms.checked_sub(spawn_ms)
}
/// Call first in `main`; the clock otherwise starts at first use and totals undercount.
pub fn mark_process_start() {
    LazyLock::force(&PROCESS_START);
}
pub fn process_elapsed() -> Duration {
    PROCESS_START.elapsed()
}
fn current() -> Option<Arc<StartupTimer>> {
    CURRENT
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(Arc::clone)
}
/// Installs a new attempt, unless startup already ended; after that the returned timer records locally only.
pub fn begin(owner: Owner) -> Arc<StartupTimer> {
    let timer = Arc::new(StartupTimer::new(owner));
    let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    let done = DONE.load(Ordering::Relaxed);
    let superseded = (!done).then(|| current.take()).flatten();
    if !done {
        *current = Some(Arc::clone(&timer));
        reset_frame_gap();
        spawn_slow_phase_warnings();
    }
    drop(current);
    if let Some(prev) = superseded {
        prev.close_root_span(RootClose::Superseded);
        prev.drain_sub_timers(StartupOutcome::Cancelled);
    }
    timer
}
pub(crate) fn agent_owned() -> Option<Arc<StartupTimer>> {
    current().filter(|p| p.owner() == Owner::Agent)
}
pub fn is_active() -> bool {
    !DONE.load(Ordering::Relaxed) && current().is_some()
}
pub fn current_phase_span() -> Option<tracing::Span> {
    current()?.lock().current_span.clone()
}
#[derive(Clone)]
pub struct SpawnTraceContext {
    pub parent: tracing::Span,
}
impl SpawnTraceContext {
    pub fn new(startup_span: Option<tracing::Span>, request_span: tracing::Span) -> Self {
        Self {
            parent: startup_span.unwrap_or(request_span),
        }
    }
}
fn clear() {
    DONE.store(true, Ordering::Relaxed);
    let timer = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(timer) = &timer {
        timer.close_root_span(RootClose::Abandoned);
    }
    drop(timer);
    close_first_frame_span();
    reset_frame_gap();
}
/// Stops recording for a standalone agent at its first client, so idle waiting is not counted; client-owned runs are unaffected.
pub fn mark_agent_serving() {
    if let Some(timer) = agent_owned() {
        timer.close_root_span(RootClose::Served);
        record_first_frame_gap();
        timer.drain_sub_timers(StartupOutcome::Ok);
        clear();
    }
}
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    DONE.store(false, Ordering::Relaxed);
    let timer = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(timer) = &timer {
        timer.close_root_span(RootClose::Discarded);
    }
    drop(timer);
    *subphases() = SubphaseTimings::default();
    INTERACTIVE.store(false, Ordering::Relaxed);
    *STARTUP_AUTH_MODE.lock().unwrap_or_else(|e| e.into_inner()) = AuthMode::Unknown;
    PROCESS_INIT_RECORDED.store(false, Ordering::Relaxed);
    reset_frame_gap();
}
/// Lazily installs an agent-owned timer, covering the standalone leader and agent server; a no-op once startup is done.
pub fn enter(phase: StartupPhase) {
    if DONE.load(Ordering::Relaxed) || INTERACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let timer = match current() {
        Some(timer) => timer,
        None => begin(Owner::Agent),
    };
    if !PROCESS_INIT_RECORDED.swap(true, Ordering::Relaxed) {
        record_launch_gap("startup.process_init", process_elapsed());
    }
    if timer.enter(phase) {
        note_phase_end();
    }
}
pub fn set_auth_mode(mode: AuthMode) {
    *STARTUP_AUTH_MODE.lock().unwrap_or_else(|e| e.into_inner()) = mode;
    if let Some(timer) = current() {
        timer.set_auth_mode(mode);
    }
}
/// Excludes a utility command from startup recording entirely.
pub fn mark_utility_process() {
    clear();
}
/// Records the startup total, at most once per process.
/// A failure the user can retry records nothing, so the eventual success still counts.
pub(crate) fn report_total(outcome: StartupOutcome) {
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let timer = CURRENT.lock().unwrap_or_else(|e| e.into_inner()).take();
    let auth_mode = timer
        .as_ref()
        .map(|t| t.auth_mode())
        .unwrap_or(AuthMode::Unknown);
    if let Some(timer) = &timer {
        timer.drain_sub_timers(outcome);
    }
    let total_ms = duration_ms(process_elapsed());
    subphases().startup_total_ms = Some(total_ms);
    let phases = match &timer {
        Some(p) => {
            if outcome == StartupOutcome::Ok {
                p.close_open_phase();
            } else {
                warn_over_budget(&p.phase_snapshot());
            }
            p.close_root_span(RootClose::Reported(outcome));
            p.summary()
        }
        None => String::new(),
    };
    let sub = *subphases();
    let event = sub.startup_completed(total_ms, outcome, phases, auth_mode);
    if let Ok(record) = serde_json::to_value(&event) {
        crate::unified_log::info(STARTUP_COMPLETE_MSG, None, Some(record));
    }
    crate::session_ctx::log_event(event);
}
/// Whole milliseconds, saturating instead of wrapping on the (never-reached) overflow, matching `subagent_spawn`.
fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
pub fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}
#[cfg(test)]
pub(crate) static SERIAL: Mutex<()> = Mutex::new(());
#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
