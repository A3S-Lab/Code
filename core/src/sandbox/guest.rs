//! macOS command execution inside one HVF guest per call.
//!
//! The shared-kernel adapter stays refused. This backend is selected only
//! after the shim, the system image, and HVF are all present. Each admitted
//! command is a script in the workspace share. The guest runs that script as
//! the init of a new PID namespace and the namespace is gone before this
//! call returns. `krun_start_enter` consumes the worker, so the guest is not
//! reused across calls.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use super::native::native_process_lifetime_unqualified;
use super::{BashSandbox, SandboxCommandRequest, SandboxExecutionOutput, SandboxOutput};
use crate::workspace::CommandOutputSummary;

/// Manifest path for the immutable guest system image.
pub const GUEST_SYSTEM_IMAGE_ENV: &str = "A3S_CODE_GUEST_SYSTEM_IMAGE";
/// Signed `a3s-oci-krun-shim` that implements `guest-command`.
pub const GUEST_KRUN_SHIM_ENV: &str = "A3S_CODE_KRUN_SHIM";

const GUEST_COMMAND_SCHEMA: &str = "a3s.oci.guest-command.v1";
const GUEST_SCRIPT_NAME: &str = ".a3s-guest-command";
const TIMEOUT_EXIT_CODE: i32 = 124;
const EXEC_COMMAND_TIMEOUT_MS: u64 = 600_000;
const MAX_CONSOLE_BYTES: usize = 1024 * 1024;
const MAX_SHIM_BYTES: u64 = 64 * 1024;
const STOP_NONE: u8 = 0;
const STOP_TIMEOUT: u8 = 1;
const STOP_SHUTDOWN: u8 = 2;
/// The caller dropped this command. The next command on this sandbox clears it.
const STOP_CANCEL: u8 = 3;

/// Pure settlement. Refusal and an unknown exit do not require a guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandSettlement {
    Refused,
    Settled { exit_code: i32, timed_out: bool },
    Indeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuestObservation {
    ProbeUnavailable,
    Exited(i32),
    TimedOut,
    Signaled,
    MissingStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostStop {
    None,
    Timeout,
    Shutdown,
}

pub(crate) fn settle(observation: GuestObservation) -> CommandSettlement {
    match observation {
        GuestObservation::ProbeUnavailable => CommandSettlement::Refused,
        GuestObservation::Exited(exit_code) => CommandSettlement::Settled {
            exit_code,
            timed_out: false,
        },
        GuestObservation::TimedOut => CommandSettlement::Settled {
            exit_code: TIMEOUT_EXIT_CODE,
            timed_out: true,
        },
        GuestObservation::Signaled | GuestObservation::MissingStatus => {
            CommandSettlement::Indeterminate
        }
    }
}

pub(crate) fn observe_json(stop: HostStop, bytes: &[u8]) -> GuestObservation {
    let report = serde_json::from_slice::<ShimReport>(bytes).ok();
    observe(stop, report.as_ref())
}

fn observe(stop: HostStop, report: Option<&ShimReport>) -> GuestObservation {
    match stop {
        HostStop::Shutdown => return GuestObservation::Signaled,
        HostStop::Timeout => return GuestObservation::TimedOut,
        HostStop::None => {}
    }
    let Some(report) = report else {
        return GuestObservation::MissingStatus;
    };
    if report.schema_version != GUEST_COMMAND_SCHEMA {
        return GuestObservation::MissingStatus;
    }
    match report.settlement.as_str() {
        "refused" => GuestObservation::ProbeUnavailable,
        "timed_out" => GuestObservation::TimedOut,
        "settled" if report.vm_entered => match report.exit_code {
            Some(exit_code) => GuestObservation::Exited(exit_code),
            None => GuestObservation::MissingStatus,
        },
        _ => GuestObservation::MissingStatus,
    }
}

#[derive(Debug, Deserialize)]
struct ShimReport {
    schema_version: String,
    settlement: String,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    vm_entered: bool,
    /// Present on refused and indeterminate reports. Settlement uses
    /// `settlement` and `vm_entered`; the text stays in the raw report.
    #[serde(default)]
    #[allow(dead_code)]
    reason: Option<String>,
}

/// Guest backend installed only after [`Self::discover`] succeeds.
pub struct GuestLifetimeSandbox {
    workspace: PathBuf,
    manifest: PathBuf,
    shim: PathBuf,
    active: Mutex<Option<u32>>,
    stop: Arc<AtomicU8>,
}

impl GuestLifetimeSandbox {
    /// Locate the shim and image from the process environment and check HVF.
    ///
    /// This does not start a user command. A missing artifact or HVF is the
    /// stable native refusal so the caller keeps the shared-kernel path.
    pub fn discover(workspace: &Path) -> anyhow::Result<Self> {
        let manifest = env_path(GUEST_SYSTEM_IMAGE_ENV)?;
        let shim = env_path(GUEST_KRUN_SHIM_ENV)?;
        let sandbox = Self::from_artifacts(workspace, &manifest, &shim)?;
        sandbox.prepare()?;
        Ok(sandbox)
    }

    pub fn from_artifacts(workspace: &Path, manifest: &Path, shim: &Path) -> anyhow::Result<Self> {
        let workspace = existing_dir(workspace)?;
        let manifest = existing_file(manifest)?;
        let shim = existing_file(shim)?;
        Ok(Self {
            workspace,
            manifest,
            shim,
            active: Mutex::new(None),
            stop: Arc::new(AtomicU8::new(STOP_NONE)),
        })
    }

    fn prepare(&self) -> anyhow::Result<()> {
        if !self.manifest.is_file() || !self.shim.is_file() || !self.workspace.is_dir() {
            return Err(native_process_lifetime_unqualified());
        }
        if !hvf_available() {
            return Err(native_process_lifetime_unqualified());
        }
        Ok(())
    }

    fn lock_active(&self) -> MutexGuard<'_, Option<u32>> {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn exec_guest(
        &self,
        request: SandboxCommandRequest,
    ) -> anyhow::Result<SandboxExecutionOutput> {
        self.prepare()?;
        if request.timeout_ms == 0 {
            self.emit(&request, "", "", true, false).await;
            return Ok(SandboxExecutionOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: TIMEOUT_EXIT_CODE,
                timed_out: true,
            });
        }

        // A prior command's timeout or drop must not stick. Explicit shutdown does.
        loop {
            let current = self.stop.load(Ordering::SeqCst);
            match current {
                STOP_NONE => break,
                STOP_TIMEOUT | STOP_CANCEL => {
                    if self
                        .stop
                        .compare_exchange(current, STOP_NONE, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        break;
                    }
                }
                STOP_SHUTDOWN => {
                    return Err(indeterminate(
                        "guest command settlement is indeterminate: sandbox is shut down",
                    ));
                }
                other => {
                    return Err(indeterminate(format!(
                        "guest command settlement is indeterminate: stop state {other}"
                    )));
                }
            }
        }
        let mut reservation = Reservation::begin(self)?;
        if self.stop.load(Ordering::SeqCst) == STOP_SHUTDOWN {
            return Err(indeterminate(
                "guest command was cancelled before the guest started",
            ));
        }

        let script_path = self.workspace.join(GUEST_SCRIPT_NAME);
        let _script = write_guest_script(&script_path, &request.command, request.env.as_deref())?;

        let console_dir = tempfile::tempdir().map_err(|error| {
            indeterminate(format!(
                "failed to create the guest console directory: {error}"
            ))
        })?;
        let console_path = console_dir.path().join("console.log");

        let timeout_secs = request.timeout_ms.div_ceil(1000).max(1);
        let mut command = Command::new(&self.shim);
        command
            .arg("guest-command")
            .arg("--owner-pid")
            .arg(std::process::id().to_string())
            .arg("--system-image-manifest")
            .arg(&self.manifest)
            .arg("--runtime-share")
            .arg(&self.workspace)
            .arg("--console")
            .arg(&console_path)
            .arg("--timeout-secs")
            .arg(timeout_secs.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            command.process_group(0);
        }
        let child = command.spawn().map_err(|error| {
            native_process_lifetime_unqualified().context(format!(
                "failed to start the guest shim {}: {error}",
                self.shim.display()
            ))
        })?;
        let pid = child.id();
        reservation.arm(pid);
        // Only the waiter kills, and only after `try_wait` has shown the shim
        // is still alive. A kill after `wait` would target a recycled pid.
        let stop = Arc::clone(&self.stop);
        let thread_stop = Arc::clone(&stop);
        let (sender, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("a3s-guest-wait".into())
            .spawn(move || {
                let result = wait_shim(child, &thread_stop);
                let _ = sender.send(result);
            });
        let thread = match thread {
            Ok(thread) => thread,
            Err(error) => {
                kill_process_group(pid);
                reap_pid(pid);
                return Err(indeterminate(format!(
                    "failed to wait for the guest shim: {error}"
                )));
            }
        };
        let mut waiter = Waiter {
            thread: Some(thread),
            stop: Arc::clone(&stop),
        };
        let recv = tokio::task::spawn_blocking(move || receiver.recv());
        tokio::pin!(recv);
        let timeout = Duration::from_millis(request.timeout_ms);
        let joined = tokio::select! {
            joined = recv.as_mut() => joined,
            _ = tokio::time::sleep(timeout) => {
                let _ = stop.compare_exchange(
                    STOP_NONE,
                    STOP_TIMEOUT,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                recv.await
            }
        };
        // Join without changing the stop flag. Drop of an unfinished waiter
        // cancels this generation only; it does not shut the sandbox down.
        waiter.finish();

        let (status, report_bytes, shim_stderr) = match joined {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => {
                return Err(indeterminate("guest shim wait channel closed"));
            }
            Err(_) => {
                return Err(indeterminate("guest shim wait task panicked"));
            }
        };
        let status = status.map_err(|error| {
            indeterminate(format!("failed to wait for the guest shim: {error}"))
        })?;
        let report_text = String::from_utf8_lossy(&report_bytes);
        let shim_error = String::from_utf8_lossy(&shim_stderr);
        let stop = match self.stop.load(Ordering::SeqCst) {
            STOP_TIMEOUT => HostStop::Timeout,
            STOP_SHUTDOWN | STOP_CANCEL => HostStop::Shutdown,
            _ => HostStop::None,
        };
        let observation = observe_json(stop, report_bytes.as_slice());
        let (stdout, truncated) = read_capped(&console_path, MAX_CONSOLE_BYTES);
        match settle(observation) {
            CommandSettlement::Refused => {
                let detail = refusal_detail(&report_text, &shim_error, status);
                Err(native_process_lifetime_unqualified().context(detail))
            }
            CommandSettlement::Indeterminate => Err(indeterminate(format!(
                "guest command settlement is indeterminate (shim status {status}; report {report_text}; stderr {shim_error})"
            ))),
            CommandSettlement::Settled {
                exit_code,
                timed_out,
            } => {
                self.emit(&request, &stdout, "", timed_out, truncated).await;
                Ok(SandboxExecutionOutput {
                    stdout,
                    stderr: String::new(),
                    exit_code,
                    timed_out,
                })
            }
        }
    }

    async fn emit(
        &self,
        request: &SandboxCommandRequest,
        stdout: &str,
        stderr: &str,
        timed_out: bool,
        truncated: bool,
    ) {
        let Some(observer) = &request.output_observer else {
            return;
        };
        if !stdout.is_empty() {
            observer.on_output_delta(stdout).await;
        }
        if !stderr.is_empty() {
            observer.on_output_delta(stderr).await;
        }
        let total = stdout.len() + stderr.len();
        observer
            .on_output_complete(&CommandOutputSummary {
                total_bytes: total,
                captured_bytes: total,
                truncated,
                timed_out,
            })
            .await;
    }
}

impl Drop for GuestLifetimeSandbox {
    fn drop(&mut self) {
        self.stop.store(STOP_SHUTDOWN, Ordering::SeqCst);
    }
}

#[async_trait]
impl BashSandbox for GuestLifetimeSandbox {
    fn validate_execution_available(&self) -> anyhow::Result<()> {
        self.prepare()
    }

    async fn exec_command(
        &self,
        command: &str,
        guest_workspace: &str,
    ) -> anyhow::Result<SandboxOutput> {
        let output = self
            .exec(SandboxCommandRequest {
                command: command.to_string(),
                guest_workspace: guest_workspace.to_string(),
                timeout_ms: EXEC_COMMAND_TIMEOUT_MS,
                output_observer: None,
                env: None,
            })
            .await?;
        Ok(SandboxOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
        })
    }

    async fn exec(&self, request: SandboxCommandRequest) -> anyhow::Result<SandboxExecutionOutput> {
        self.exec_guest(request).await
    }

    fn supports_background_execution(&self) -> bool {
        false
    }

    async fn shutdown(&self) {
        self.stop.store(STOP_SHUTDOWN, Ordering::SeqCst);
    }
}

struct Reservation<'a> {
    sandbox: &'a GuestLifetimeSandbox,
    pid: u32,
}

impl<'a> Reservation<'a> {
    fn begin(sandbox: &'a GuestLifetimeSandbox) -> anyhow::Result<Self> {
        let mut active = sandbox.lock_active();
        if active.is_some() {
            return Err(indeterminate(
                "guest command settlement is indeterminate: another command is still active",
            ));
        }
        *active = Some(0);
        Ok(Self { sandbox, pid: 0 })
    }

    fn arm(&mut self, pid: u32) {
        self.pid = pid;
        *self.sandbox.lock_active() = Some(pid);
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut active = self.sandbox.lock_active();
        if *active == Some(self.pid) || (*active == Some(0) && self.pid == 0) {
            *active = None;
        }
    }
}

struct Waiter {
    thread: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicU8>,
}

impl Waiter {
    fn finish(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if self.thread.is_some() {
            // Kill this generation. Do not overwrite an explicit shutdown
            // or a deadline that is already stopping the same process.
            let _ = self.stop.compare_exchange(
                STOP_NONE,
                STOP_CANCEL,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
        }
        self.finish();
    }
}

struct ScriptFile(PathBuf);

impl Drop for ScriptFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn wait_shim(
    mut child: std::process::Child,
    stop: &AtomicU8,
) -> (std::io::Result<ExitStatus>, Vec<u8>, Vec<u8>) {
    let pid = child.id();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                let flag = stop.load(Ordering::SeqCst);
                if flag == STOP_TIMEOUT || flag == STOP_SHUTDOWN || flag == STOP_CANCEL {
                    kill_process_group(pid);
                    break child.wait();
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => break Err(error),
        }
    };
    let stdout = read_pipe(child.stdout.take());
    let stderr = read_pipe(child.stderr.take());
    (status, stdout, stderr)
}

fn reap_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::waitpid(pid as i32, std::ptr::null_mut(), 0);
    }
}

fn read_pipe(pipe: Option<impl Read>) -> Vec<u8> {
    let Some(pipe) = pipe else {
        return Vec::new();
    };
    let mut output = Vec::new();
    let _ = pipe.take(MAX_SHIM_BYTES).read_to_end(&mut output);
    output
}

fn read_capped(path: &Path, cap: usize) -> (String, bool) {
    let Ok(file) = File::open(path) else {
        return (String::new(), false);
    };
    let mut limited = file.take(cap as u64 + 1);
    let mut bytes = Vec::new();
    if limited.read_to_end(&mut bytes).is_err() {
        return (String::new(), false);
    }
    let truncated = bytes.len() > cap;
    if truncated {
        bytes.truncate(cap);
    }
    (String::from_utf8_lossy(&bytes).into_owned(), truncated)
}

fn write_guest_script(
    path: &Path,
    command: &str,
    env: Option<&HashMap<String, String>>,
) -> anyhow::Result<ScriptFile> {
    if let Some(env) = env {
        for key in env.keys() {
            if !portable_env_name(key) {
                anyhow::bail!("guest command environment name is not a portable shell identifier");
            }
        }
    }
    let mut body = String::new();
    if let Some(env) = env {
        let mut keys: Vec<&String> = env.keys().collect();
        keys.sort();
        for key in keys {
            if let Some(value) = env.get(key) {
                body.push_str("export ");
                body.push_str(key);
                body.push('=');
                body.push_str(&shell_quote(value));
                body.push('\n');
            }
        }
    }
    body.push_str(command);
    if !command.ends_with('\n') {
        body.push('\n');
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o700);
    }
    let mut file = options.open(path)?;
    let script = ScriptFile(path.to_path_buf());
    std::io::Write::write_all(&mut file, body.as_bytes())?;
    Ok(script)
}

fn portable_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn shell_quote(value: &str) -> String {
    let mut quoted = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}

fn env_path(key: &str) -> anyhow::Result<PathBuf> {
    let value = std::env::var_os(key).ok_or_else(native_process_lifetime_unqualified)?;
    if value.is_empty() {
        return Err(native_process_lifetime_unqualified());
    }
    Ok(PathBuf::from(value))
}

fn existing_dir(path: &Path) -> anyhow::Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .map_err(|_| native_process_lifetime_unqualified())?;
    if !canonical.is_dir() {
        return Err(native_process_lifetime_unqualified());
    }
    Ok(canonical)
}

fn existing_file(path: &Path) -> anyhow::Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .map_err(|_| native_process_lifetime_unqualified())?;
    if !canonical.is_file() {
        return Err(native_process_lifetime_unqualified());
    }
    Ok(canonical)
}

fn refusal_detail(report: &str, stderr: &str, status: ExitStatus) -> String {
    format!("guest shim status {status}; report {report}; stderr {stderr}")
}

fn indeterminate(detail: impl Into<String>) -> anyhow::Error {
    anyhow::anyhow!(
        "guest command settlement is indeterminate: {}",
        detail.into()
    )
}

fn hvf_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        let mut value: i32 = 0;
        let mut len: libc::size_t = std::mem::size_of::<i32>();
        let Ok(name) = std::ffi::CString::new("kern.hv_support") else {
            return false;
        };
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                &mut value as *mut i32 as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        rc == 0 && value == 1
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

fn kill_process_group(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

#[cfg(test)]
mod settle_tests;

#[cfg(all(test, target_os = "macos"))]
mod fail_closed_tests;
