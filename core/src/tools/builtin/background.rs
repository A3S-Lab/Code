//! WC-BG1: background execution as a Run-owned governed effect.
//!
//! Design contract (docs/a3s-code-world-class-gap-roadmap.md, `WC-BG1`):
//! a model-facing tool starts a process in the background and its output
//! lines arrive as `background.output` facts through the normal ingest path
//! — the session's one fact log, appended via the Run's shared `FileLog`
//! instance so seq assignment serializes against the actor's own appends.
//! There is no component-internal timer and no second loop: the reader is
//! effect-owned I/O, the model sees arrivals at its next natural call, and
//! the existing scope-close semantics settle the task before the Run's
//! successful completion is published.
//!
//! Cancellation follows ownership: the effect is registered BEFORE a
//! Run-supervised worker calls the configured sandbox, so teardown always
//! has a cancellation path. A task started
//! without a capability context (plain host-direct context) is refused —
//! an unowned background process is a leak, not a degradation.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use a3s_effect::{background_output_fact, FileLog, LogStore};
use async_trait::async_trait;

use crate::tools::types::{Tool, ToolContext, ToolErrorKind, ToolOutput};
use crate::workspace::{CommandOutputObserver, CommandOutputSummary};

#[cfg(test)]
pub(crate) mod test_backend;

/// Concurrent background tasks one Run may own. Exceeding the ceiling is a
/// tool error, never a queue.
pub(crate) const BG_PER_RUN_CEILING: usize = 4;
const BG_MAX_TASK_NAME_BYTES: usize = 128;
/// One emitted line is truncated to this many bytes before it is recorded.
pub(crate) const BG_MAX_LINE_BYTES: usize = 4_096;
/// Total recorded facts per task (stdout + stderr + markers). Past the cap
/// one truncation fact lands and the stream is no longer recorded.
pub(crate) const BG_FACT_LINES_PER_TASK: u64 = 256;
/// Retained window for `tail`, bounded in lines.
pub(crate) const BG_RING_LINES: usize = 64;
/// Upper bound a `tail` may wait for a pattern.
pub(crate) const BG_TAIL_MAX_WAIT_MS: u64 = 30_000;
/// Upper bound the `tail` result text may occupy.
pub(crate) const BG_TAIL_OUTPUT_CAP: usize = 16 * 1024;
/// Grace for termination at settlement and at explicit `stop`.
const BG_GRACE: Duration = Duration::from_secs(2);
/// Background commands are additionally bounded even if their Run remains open.
const BG_EXECUTION_TIMEOUT_MS: u64 = 3_600_000;

/// Durable terminal classification. Failed, timed-out, and unknown effects
/// never gain a successful settlement proof from their display text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BackgroundDisposition {
    Completed,
    Cancelled,
    Failed,
    TimedOut,
    Unknown,
}

impl BackgroundDisposition {
    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            "timed_out" => Ok(Self::TimedOut),
            "unknown" => Ok(Self::Unknown),
            other => anyhow::bail!("background disposition must be one of the closed set [completed, cancelled, failed, timed_out, unknown]; got {other:?}"),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Unknown => "unknown",
        }
    }
}

/// The bounded evidence used to acknowledge the process lifetime. The drop
/// contract is explicitly opted into by the host backend; no OS wait status
/// or complete pipe drain is invented for a cancelled execution future.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessSettlementBasis {
    ExecutionReturned,
    BackendDropContract,
    Unproven,
    /// The backend returned a typed pre-spawn refusal. No process was
    /// created, so there is no lifetime left to prove.
    AdmissionRefused,
}

impl ProcessSettlementBasis {
    pub(crate) fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "execution_returned" => Ok(Self::ExecutionReturned),
            "backend_drop_contract" => Ok(Self::BackendDropContract),
            "unproven" => Ok(Self::Unproven),
            "admission_refused" => Ok(Self::AdmissionRefused),
            other => anyhow::bail!("background process settlement basis must be one of the closed set [execution_returned, backend_drop_contract, unproven, admission_refused]; got {other:?}"),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ExecutionReturned => "execution_returned",
            Self::BackendDropContract => "backend_drop_contract",
            Self::Unproven => "unproven",
            Self::AdmissionRefused => "admission_refused",
        }
    }
}

#[derive(Default)]
struct OutputLineBuffer {
    pending: String,
    truncated: bool,
}

struct BackgroundTask {
    name: String,
    task_id: String,
    /// Agent admission identity, distinct from the capability scope UUID.
    source_run: String,
    run: uuid::Uuid,
    log: Arc<FileLog>,
    thread: String,
    /// Request to terminate; the reaper owns the child and performs the
    /// kill so exactly one owner waits on it.
    kill_switch: tokio_util::sync::CancellationToken,
    /// Final disposition, set once by the reaper.
    status: std::sync::Mutex<Option<String>>,
    /// First failure to retain evidence. A process exit cannot erase it.
    failure: std::sync::Mutex<Option<String>>,
    output_evidence_failed: std::sync::atomic::AtomicBool,
    output_lines: std::sync::Mutex<OutputLineBuffer>,
    output_summary: std::sync::Mutex<Option<CommandOutputSummary>>,
    observed_output_bytes: std::sync::atomic::AtomicU64,
    /// Retained output lines for `tail`.
    ring: std::sync::Mutex<VecDeque<String>>,
    /// Counters owned by the reader/reaper tasks.
    emitted: std::sync::atomic::AtomicU64,
    truncated_noted: std::sync::atomic::AtomicBool,
    /// Signalled on every recorded line and at exit.
    notify: tokio::sync::Notify,
}

impl BackgroundTask {
    fn new(
        name: String,
        task_id: String,
        source_run: String,
        run: uuid::Uuid,
        log: Arc<FileLog>,
        thread: String,
    ) -> Self {
        Self {
            name,
            task_id,
            source_run,
            run,
            log,
            thread,
            kill_switch: tokio_util::sync::CancellationToken::new(),
            status: std::sync::Mutex::new(None),
            failure: std::sync::Mutex::new(None),
            output_evidence_failed: std::sync::atomic::AtomicBool::new(false),
            output_lines: std::sync::Mutex::new(OutputLineBuffer::default()),
            output_summary: std::sync::Mutex::new(None),
            observed_output_bytes: std::sync::atomic::AtomicU64::new(0),
            ring: std::sync::Mutex::new(VecDeque::new()),
            emitted: std::sync::atomic::AtomicU64::new(0),
            truncated_noted: std::sync::atomic::AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        }
    }

    fn remember_failure(&self, message: String) {
        let mut failure = self
            .failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if failure.is_none() {
            *failure = Some(truncate_utf8(&message, BG_MAX_LINE_BYTES));
        }
    }

    fn failure(&self) -> Option<String> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn append(&self, mut fact: a3s_effect::NewFact) {
        // Names may be reused by later Runs on the same session log. The
        // immutable task identity, rather than its display name, owns keys.
        fact.key = format!(
            "bg:{}:{}",
            self.task_id,
            fact.key.rsplit(':').next().unwrap_or("0")
        );
        fact.payload["task_id"] = serde_json::json!(self.task_id);
        fact.payload["source_run"] = serde_json::json!(self.source_run);
        fact.payload["run"] = serde_json::json!(self.run.to_string());
        if let Err(error) = self.log.append(&self.thread, &[fact], None) {
            self.output_evidence_failed
                .store(true, std::sync::atomic::Ordering::SeqCst);
            self.remember_failure(format!("background evidence append failed: {error}"));
            self.kill_switch.cancel();
        }
    }

    fn record_line(&self, line: &str, stream: Option<&str>) {
        let line = truncate_utf8(line, BG_MAX_LINE_BYTES);
        let seq = self
            .emitted
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        if seq > BG_FACT_LINES_PER_TASK {
            if !self
                .truncated_noted
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                self.append(background_output_fact(
                    &self.name,
                    "",
                    seq,
                    Some("truncated"),
                    stream,
                ));
            }
        } else {
            self.append(background_output_fact(&self.name, &line, seq, None, stream));
        }
        let mut ring = self
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if ring.len() >= BG_RING_LINES {
            ring.pop_front();
        }
        ring.push_back(match stream {
            Some("stderr") => format!("(stderr) {line}"),
            _ => line,
        });
        self.notify.notify_waiters();
    }

    fn status(&self) -> Option<String> {
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn wait_for_settlement(&self) -> bool {
        let deadline = tokio::time::Instant::now() + BG_GRACE;
        loop {
            // Subscribe before inspecting state: Notify::notify_waiters does
            // not retain a wakeup for a waiter created after completion.
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.status().is_some() {
                return true;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let _ = tokio::time::timeout(remaining, notified).await;
        }
    }

    fn finish_partial_output(&self) {
        let mut lines = self
            .output_lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !lines.pending.is_empty() || lines.truncated {
            let mut line = std::mem::take(&mut lines.pending);
            if std::mem::take(&mut lines.truncated) {
                line.push('…');
            }
            self.record_line(&line, None);
        }
    }

    fn finish(&self, status: String, mut metadata: serde_json::Value) {
        let status = truncate_utf8(&status, BG_MAX_LINE_BYTES);
        self.finish_partial_output();
        let retained = !self
            .output_evidence_failed
            .load(std::sync::atomic::Ordering::SeqCst);
        metadata["output_facts_retained"] = serde_json::json!(retained);
        if !retained {
            // A transient append failure can recover before the terminal is
            // written. Persist its failure instead of losing it on restart.
            metadata["disposition"] = serde_json::json!(BackgroundDisposition::Unknown.as_str());
            metadata["process_settled"] = serde_json::json!(false);
            metadata["process_settlement_basis"] =
                serde_json::json!(ProcessSettlementBasis::Unproven.as_str());
        }
        let seq = self
            .emitted
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        let mut fact = background_output_fact(&self.name, &status, seq, Some("exited"), None);
        if let Some(fields) = metadata.as_object() {
            for (key, value) in fields {
                fact.payload[key] = value.clone();
            }
        }
        self.append(fact);
        *self
            .status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(status);
        self.notify.notify_waiters();
    }
}

#[async_trait]
impl CommandOutputObserver for BackgroundTask {
    async fn on_output_delta(&self, delta: &str) {
        self.observed_output_bytes.fetch_add(
            u64::try_from(delta.len()).unwrap_or(u64::MAX),
            std::sync::atomic::Ordering::Relaxed,
        );
        let mut lines = self
            .output_lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A producer can emit an arbitrarily long unterminated line. Keep
        // bounded UTF-8 state while continuing to drain the sandbox observer.
        for character in delta.chars() {
            if character == '\n' {
                let mut line = std::mem::take(&mut lines.pending);
                if std::mem::take(&mut lines.truncated) {
                    line.push('…');
                }
                self.record_line(line.trim_end_matches('\r'), None);
            } else if !lines.truncated {
                if lines.pending.len() + character.len_utf8() <= BG_MAX_LINE_BYTES {
                    lines.pending.push(character);
                } else {
                    lines.truncated = true;
                }
            }
        }
    }

    async fn on_output_complete(&self, summary: &CommandOutputSummary) {
        *self
            .output_summary
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(*summary);
    }
}

fn truncate_utf8(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut boundary = max;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}…", &value[..boundary])
}

/// Live background tasks keyed by owning Run (instance id) then task name.
#[derive(Default)]
struct BackgroundRuns(std::sync::Mutex<HashMap<uuid::Uuid, HashMap<String, Arc<BackgroundTask>>>>);

impl BackgroundRuns {
    fn names_for(&self, run: uuid::Uuid) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&run)
            .map(|tasks| tasks.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn insert(&self, run: uuid::Uuid, task: Arc<BackgroundTask>) -> bool {
        let mut runs = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tasks = runs.entry(run).or_default();
        if tasks.len() >= BG_PER_RUN_CEILING || tasks.contains_key(&task.name) {
            return false;
        }
        tasks.insert(task.name.clone(), task);
        true
    }

    fn get(&self, run: uuid::Uuid, name: &str) -> Option<Arc<BackgroundTask>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&run)
            .and_then(|tasks| tasks.get(name).cloned())
    }

    fn remove(&self, run: uuid::Uuid, name: &str, task_id: &str) {
        let mut runs = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(tasks) = runs.get_mut(&run) {
            if tasks.get(name).is_some_and(|task| task.task_id == task_id) {
                tasks.remove(name);
            }
            if tasks.is_empty() {
                runs.remove(&run);
            }
        }
    }
}

/// The Run-owned settlement effect: terminate the task, let the reaper land
/// the final `exited` fact, and only then allow the Run to complete. A task
/// that refuses to die inside the grace window refuses the Run's successful
/// completion instead of leaking silently.
struct BackgroundSettlementEffect {
    runs: Arc<BackgroundRuns>,
    run: uuid::Uuid,
    task: Arc<BackgroundTask>,
}

#[async_trait]
impl crate::capability::CapabilityEffect for BackgroundSettlementEffect {
    fn name(&self) -> &str {
        "background-process-settlement"
    }

    async fn close(self: Box<Self>) -> Result<(), crate::capability::CapabilityEffectError> {
        self.task.kill_switch.cancel();
        if !self.task.wait_for_settlement().await {
            return Err(crate::capability::CapabilityEffectError::new(format!(
                "background task '{}' did not settle within the grace window",
                self.task.name
            )));
        }
        self.runs
            .remove(self.run, &self.task.name, &self.task.task_id);
        if let Some(failure) = self.task.failure() {
            return Err(crate::capability::CapabilityEffectError::new(failure));
        }
        Ok(())
    }
}

pub struct BackgroundTool {
    runs: Arc<BackgroundRuns>,
}

impl Default for BackgroundTool {
    fn default() -> Self {
        Self {
            runs: Arc::new(BackgroundRuns::default()),
        }
    }
}

#[async_trait]
impl Tool for BackgroundTool {
    fn name(&self) -> &str {
        "background"
    }

    fn description(&self) -> &str {
        "Run a shell command in the background and react to its output later. \
         start launches the command under the current Run: each output line \
         arrives as a fact and reaches you on your next turn, and the task is \
         terminated when the Run completes. tail returns the recent output \
         window and can wait for a pattern. stop terminates one task early. \
         Background tasks use the configured sandbox and share the workspace. \
         They are bounded: at most 4 per Run, 256 recorded lines each, and one \
         hour of execution."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "tail", "stop"],
                    "description": "Required. start launches, tail reads recent output (optionally waiting for a pattern), stop terminates early."
                },
                "name": {
                    "type": "string",
                    "description": "Task name, unique within the Run. Required for every action."
                },
                "command": {
                    "type": "string",
                    "description": "For start: the shell command to run in the workspace."
                },
                "wait_for": {
                    "type": "string",
                    "description": "For tail: return as soon as a retained line contains this substring."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "For tail: bound the wait (default 5000, max 30000)."
                }
            },
            "required": ["action", "name"]
        })
    }

    fn validate_execution_available(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<()> {
        // An unavailable command backend must not open an observation debt.
        // Tail and stop remain available for cleanup of existing children.
        if args.get("action").and_then(serde_json::Value::as_str) != Some("start") {
            return Ok(());
        }
        let Some(sandbox) = &ctx.sandbox else {
            anyhow::bail!("background execution requires a configured sandbox");
        };
        sandbox.validate_execution_available()?;
        anyhow::ensure!(
            sandbox.supports_background_execution(),
            "this sandbox backend does not support background execution"
        );
        Ok(())
    }

    async fn execute(
        &self,
        args: &serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let action = args.get("action").and_then(|v| v.as_str()).unwrap_or("");
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let Some(name) = name else {
            return Ok(ToolOutput::error(
                "name parameter is required (non-empty task name)",
            ));
        };
        if name.len() > BG_MAX_TASK_NAME_BYTES {
            return Ok(ToolOutput::error(format!(
                "background task name exceeds the {BG_MAX_TASK_NAME_BYTES}-byte bound"
            )));
        }
        match action {
            "start" => self.start(args, name, ctx).await,
            "tail" => self.tail(args, name, ctx).await,
            "stop" => self.stop(name, ctx).await,
            other => Ok(ToolOutput::error(format!(
                "unknown action '{other}': the closed set is start, tail, stop"
            ))),
        }
    }
}

impl BackgroundTool {
    async fn start(
        &self,
        args: &serde_json::Value,
        name: &str,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let Some(run) = ctx.capability_run_instance_id().ok() else {
            return Ok(ToolOutput::error(
                "background execution requires a Run-owned capability context; \
                 this context owns no Run, and an unowned background process \
                 would be a leak. Run the command in the foreground instead.",
            ));
        };
        let Some(source_run) = ctx
            .run_id()
            .filter(|run| !run.trim().is_empty() && run.len() <= 1_024)
        else {
            return Ok(ToolOutput::error(
                "background execution requires an admitted source Run identity",
            ));
        };
        let Some(log) = ctx.fact_log.clone() else {
            return Ok(ToolOutput::error(
                "background execution requires the Run's shared fact log; \
                 this context does not carry one",
            ));
        };
        let Some(session_id) = ctx.session_id.as_deref() else {
            return Ok(ToolOutput::error(
                "background execution requires a session identity",
            ));
        };
        let Some(command) = args.get("command").and_then(|v| v.as_str()) else {
            return Ok(ToolOutput::error(
                "command parameter is required for background start",
            ));
        };
        let Some(sandbox) = ctx
            .sandbox
            .clone()
            .filter(|sandbox| sandbox.supports_background_execution())
        else {
            let message = "background execution requires a configured sandbox with streaming output, capture accounting, and cancellation guarantees; this backend does not support background execution";
            let mut output = ToolOutput::error(message);
            output.error_kind = Some(ToolErrorKind::Unsupported {
                message: message.to_string(),
            });
            return Ok(output);
        };
        if ctx.workspace_services.local_root().is_none() {
            return Ok(ToolOutput::error(
                "background execution requires an observable local workspace",
            ));
        }
        if let Err(error) = ctx
            .workspace_operation(crate::capability::WorkspaceOperation::Execute)
            .and_then(|access| access.check())
        {
            return Ok(ToolOutput::error(format!(
                "background execution workspace authority is unavailable: {error}"
            )));
        }
        let Some(capability) = ctx.capability_context() else {
            return Ok(ToolOutput::error(
                "background execution requires a live Run owner",
            ));
        };
        let run_cancellation = match capability.background_scope().cancellation() {
            Ok(cancellation) => cancellation,
            Err(error) => {
                return Ok(ToolOutput::error(format!(
                    "background Run owner is unavailable: {error}"
                )))
            }
        };
        let existing = self.runs.names_for(run);
        if existing.iter().any(|item| item == name) {
            return Ok(ToolOutput::error(format!(
                "a background task named '{name}' already exists in this Run; names are unique per Run"
            )));
        }
        if existing.len() >= BG_PER_RUN_CEILING {
            return Ok(ToolOutput::error(format!(
                "this Run already owns {}/{} background tasks ({}); the per-Run ceiling is a bound, not a queue — stop one before starting another",
                existing.len(),
                BG_PER_RUN_CEILING,
                existing.join(", ")
            )));
        }

        let task_id = format!("background-{}", uuid::Uuid::new_v4());
        let task = Arc::new(BackgroundTask::new(
            name.to_string(),
            task_id.clone(),
            source_run.to_string(),
            run,
            log,
            crate::fact_control::thread_for_session(session_id),
        ));

        if !self.runs.insert(run, Arc::clone(&task)) {
            return Ok(ToolOutput::error(format!(
                "background task '{name}' lost the admission race in this Run"
            )));
        }
        // Both the worker and its settlement acknowledgment belong to the
        // Run before the sandbox can acquire a process.
        if let Err(error) = ctx.register_run_capability_effect(BackgroundSettlementEffect {
            runs: Arc::clone(&self.runs),
            run,
            task: Arc::clone(&task),
        }) {
            self.runs.remove(run, name, &task_id);
            return Ok(ToolOutput::error(format!(
                "background settlement could not be registered to the Run: {error}"
            )));
        }
        crate::porcelain::reserve_workspace_child_with_settlement(
            &task_id,
            task.kill_switch.clone(),
        );
        let worker = execute_background(
            Arc::clone(&task),
            sandbox,
            command.to_string(),
            ctx.clone(),
            run_cancellation,
        );
        if let Err(error) = capability
            .background_scope()
            .spawn_task(task_id.clone(), worker)
        {
            crate::porcelain::settle_workspace_child_paths(&task_id, Vec::new());
            task.finish(
                format!("worker admission refused: {error}"),
                serde_json::json!({ "workspace_child": task_id, "changed_paths": [] }),
            );
            self.runs.remove(run, name, &task_id);
            return Ok(ToolOutput::error(format!(
                "background worker could not be registered to the Run: {error}"
            )));
        }

        Ok(ToolOutput::success(format!(
            "Started background task '{name}'. Output lines arrive as facts and reach you on your next turn; call background tail (name: {name}) to read the retained window or wait for a pattern. The task is terminated when this Run completes."
        ))
        .with_metadata(serde_json::json!({ "name": name, "run": run.to_string(), "source_run": source_run, "workspace_child": task_id })))
    }

    async fn tail(
        &self,
        args: &serde_json::Value,
        name: &str,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let Some(run) = ctx.capability_run_instance_id().ok() else {
            return Ok(ToolOutput::error(
                "background tail requires a Run-owned capability context",
            ));
        };
        let Some(task) = self.runs.get(run, name) else {
            let names = self.runs.names_for(run);
            return Ok(ToolOutput::error(format!(
                "no background task named '{name}' in this Run{}",
                if names.is_empty() {
                    String::new()
                } else {
                    format!("; live tasks: {}", names.join(", "))
                }
            )));
        };
        let wait_for = args.get("wait_for").and_then(|v| v.as_str());
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(5_000)
            .min(BG_TAIL_MAX_WAIT_MS);
        if let Some(pattern) = wait_for {
            let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
            loop {
                let notified = task.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let matched = {
                    let ring = task
                        .ring
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    ring.iter().any(|line| line.contains(pattern))
                };
                if matched {
                    break;
                }
                if task.status().is_some() {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let _ = tokio::time::timeout(remaining, notified).await;
            }
        }
        let lines: Vec<String> = task
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect();
        let found = wait_for.is_some_and(|pattern| lines.iter().any(|line| line.contains(pattern)));
        let body = render_window(&lines, BG_TAIL_OUTPUT_CAP);
        let status = task.status();
        let mut output = format!("background '{name}' (last {} lines):\n{body}", lines.len());
        if let Some(pattern) = wait_for {
            output.push_str(&format!(
                "\npattern '{pattern}': {}",
                if found { "seen" } else { "not seen" }
            ));
        }
        match status {
            Some(status) => output.push_str(&format!("\nstatus: {status}")),
            None => output.push_str("\nstatus: running"),
        }
        let result = if let Some(failure) = task.failure() {
            ToolOutput::error(format!("{output}\nevidence failure: {failure}"))
        } else {
            ToolOutput::success(output)
        };
        Ok(result.with_metadata(serde_json::json!({ "name": name, "found": found })))
    }

    async fn stop(&self, name: &str, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let Some(run) = ctx.capability_run_instance_id().ok() else {
            return Ok(ToolOutput::error(
                "background stop requires a Run-owned capability context",
            ));
        };
        let Some(task) = self.runs.get(run, name) else {
            let names = self.runs.names_for(run);
            return Ok(ToolOutput::error(format!(
                "no background task named '{name}' in this Run{}",
                if names.is_empty() {
                    String::new()
                } else {
                    format!("; live tasks: {}", names.join(", "))
                }
            )));
        };
        task.kill_switch.cancel();
        task.wait_for_settlement().await;
        let lines: Vec<String> = task
            .ring
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect();
        let body = render_window(&lines, BG_TAIL_OUTPUT_CAP);
        let Some(status) = task.status() else {
            return Ok(ToolOutput::error(format!(
                "background task '{name}' did not settle within the grace window"
            )));
        };
        if let Some(failure) = task.failure() {
            return Ok(ToolOutput::error(format!(
                "background task '{name}' settled without complete evidence: {failure}"
            )));
        }
        self.runs.remove(run, name, &task.task_id);
        Ok(ToolOutput::success(format!(
            "Stopped background task '{name}'.\nstatus: {status}\nlast {} lines:\n{body}",
            lines.len()
        ))
        .with_metadata(serde_json::json!({ "name": name })))
    }
}

/// The Run supervisor owns this worker. Dropping the execution future is
/// supported only by backends that explicitly guarantee descendant cleanup.
async fn execute_background(
    task: Arc<BackgroundTask>,
    sandbox: Arc<dyn crate::sandbox::BashSandbox>,
    command: String,
    ctx: ToolContext,
    run_cancellation: tokio_util::sync::CancellationToken,
) -> Result<(), crate::capability::CapabilityEffectError> {
    let mut child_guard = crate::porcelain::WorkspaceChildGuard::new(&task.task_id);
    let span = super::bash::CommandSpan::begin(ctx.workspace.as_path()).await;
    let mut execution = Box::pin(sandbox.exec(crate::sandbox::SandboxCommandRequest {
        command: super::bash::command_for_sandbox(&command),
        guest_workspace: "/workspace".to_string(),
        timeout_ms: BG_EXECUTION_TIMEOUT_MS,
        output_observer: Some(Arc::clone(&task) as Arc<dyn CommandOutputObserver>),
        env: span.env(ctx.command_env.clone()),
    }));
    let (result, cancelled, deadline_reached) = tokio::select! {
        biased;
        _ = run_cancellation.cancelled() => (None, true, false),
        _ = task.kill_switch.cancelled() => (None, true, false),
        _ = tokio::time::sleep(Duration::from_millis(BG_EXECUTION_TIMEOUT_MS)) => (None, false, true),
        result = &mut execution => (Some(result), false, false),
    };
    // Release the native process-group guard before observing final paths.
    drop(execution);
    let observed = span.finish_observed(&ctx).await;
    let effects = crate::porcelain::observe_path_effects(ctx.workspace.as_path(), observed.clone());
    let observation_incomplete = observed.incomplete
        || effects.completeness == a3s_effect::ToolObservationCompleteness::Incomplete
        || effects.validate().is_err();
    let paths = observed.paths;
    if observation_incomplete {
        task.remember_failure("background workspace observation is incomplete".to_string());
    }
    let summary = *task
        .output_summary
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let refused_before_start = matches!(
        &result,
        Some(Err(error))
            if crate::sandbox::native::is_native_process_lifetime_unqualified(error)
    );
    let (status, exit_code, timed_out, disposition) = match result {
        Some(Ok(output)) => {
            if summary.is_none() {
                task.remember_failure(
                    "sandbox omitted background output capture accounting".to_string(),
                );
            }
            // The native adapter uses -1 when a signal exit has no code.
            // That sentinel is not a known process exit status.
            let exit_code = (output.exit_code >= 0).then_some(output.exit_code);
            if exit_code.is_none() {
                task.remember_failure("sandbox omitted a known background exit code".to_string());
            }
            (
                format!("exit status {}", output.exit_code),
                exit_code,
                output.timed_out,
                if output.timed_out {
                    BackgroundDisposition::TimedOut
                } else if exit_code.is_none() || summary.is_none() || observation_incomplete {
                    BackgroundDisposition::Unknown
                } else {
                    BackgroundDisposition::Completed
                },
            )
        }
        Some(Err(error)) if refused_before_start => {
            // No process was created. Do not remember this as missing
            // execution evidence; a genuine workspace-watch failure above
            // remains the only uncertainty.
            (
                error.to_string(),
                None,
                false,
                BackgroundDisposition::Failed,
            )
        }
        Some(Err(error)) => {
            task.remember_failure(format!("sandbox background execution failed: {error}"));
            (
                format!("execution failed: {error}"),
                None,
                false,
                BackgroundDisposition::Failed,
            )
        }
        None if cancelled => (
            "terminated by request".to_string(),
            None,
            false,
            if observation_incomplete {
                BackgroundDisposition::Unknown
            } else {
                BackgroundDisposition::Cancelled
            },
        ),
        None => (
            "background execution timed out".to_string(),
            None,
            deadline_reached,
            BackgroundDisposition::TimedOut,
        ),
    };
    let settlement_basis = if refused_before_start && !observation_incomplete {
        ProcessSettlementBasis::AdmissionRefused
    } else {
        match disposition {
            BackgroundDisposition::Completed => ProcessSettlementBasis::ExecutionReturned,
            BackgroundDisposition::Cancelled => ProcessSettlementBasis::BackendDropContract,
            BackgroundDisposition::Failed
            | BackgroundDisposition::TimedOut
            | BackgroundDisposition::Unknown => ProcessSettlementBasis::Unproven,
        }
    };
    let process_settled = settlement_basis != ProcessSettlementBasis::Unproven;
    let capture = summary.map(|summary| {
        serde_json::json!({
            "total_bytes": summary.total_bytes,
            "captured_bytes": summary.captured_bytes,
            "truncated": summary.truncated,
            "timed_out": summary.timed_out,
        })
    });
    task.finish(
        status,
        serde_json::json!({
            "workspace_child": task.task_id,
            "changed_paths": paths,
            "observation_incomplete": observation_incomplete,
            "effects": effects,
            "exit_code": exit_code,
            "cancelled": cancelled,
            "timed_out": timed_out,
            "disposition": disposition.as_str(),
            "process_settled": process_settled,
            "process_settlement_basis": settlement_basis.as_str(),
            // This worker is reachable only after explicit backend opt-in.
            // drop(execution) above precedes the complete workspace read.
            "execution_future_released": true,
            "output": capture,
            // Cancellation drops the backend future and may leave unread
            // pipe bytes even if a summary arrived before cancellation won.
            "output_complete": !cancelled && summary.is_some(),
            "output_observed_bytes": task.observed_output_bytes.load(std::sync::atomic::Ordering::Relaxed),
        }),
    );
    child_guard.settle_paths(paths);
    match task.failure() {
        Some(failure) => Err(crate::capability::CapabilityEffectError::new(failure)),
        None => Ok(()),
    }
}

/// Keep the tail of the window within `cap` bytes, dropping whole lines from
/// the front and noting how many went.
fn render_window(lines: &[String], cap: usize) -> String {
    let mut kept: VecDeque<String> = VecDeque::new();
    let mut used = 0usize;
    let mut dropped = 0usize;
    for line in lines {
        let cost = line.len() + 1;
        // Evict from the front until the newest line fits; the window keeps
        // the tail, not the head.
        while used + cost > cap && !kept.is_empty() {
            if let Some(front) = kept.pop_front() {
                used -= front.len() + 1;
                dropped += 1;
            }
        }
        if used + cost > cap {
            // A single line wider than the whole cap: drop it rather than
            // return an unbounded body.
            dropped += 1;
            continue;
        }
        kept.push_back(line.clone());
        used += cost;
    }
    let mut body = kept.into_iter().collect::<Vec<_>>().join("\n");
    if dropped > 0 {
        body = format!("… {dropped} older line(s) dropped\n{body}");
    }
    body
}

#[cfg(test)]
mod tests;
