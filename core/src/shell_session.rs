//! Session-scoped shell identity. Cwd and env do not cross sessions.
//!
//! Detach, poll, and kill stay permission-checked by the caller. Persisted
//! transcripts never copy secret-shaped env values. A timeout or cancel kills
//! the process group.

use anyhow::{anyhow, Result};
use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

const SECRET_MARKERS: &[&str] = &[
    "SECRET",
    "TOKEN",
    "PASSWORD",
    "API_KEY",
    "AUTHORIZATION",
    "PRIVATE_KEY",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAdmission {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedCommand {
    pub cwd: PathBuf,
    pub command: String,
    pub transcript_env: Vec<(String, String)>,
}

#[derive(Debug)]
struct Job {
    child: Child,
}

struct Session {
    cwd: PathBuf,
    /// When set, `cd` cannot leave this directory. Unset sessions keep a free cwd.
    root: Option<PathBuf>,
    env: HashMap<String, String>,
    jobs: HashMap<String, Job>,
}

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn bind_session(session_id: &str, cwd: &Path) {
    bind_session_with_root(session_id, cwd, None);
}

/// Bind a session whose `cd` cannot leave `root`. Isolation uses this so a
/// conversation shell cannot walk back onto the source tree.
pub fn bind_session_rooted(session_id: &str, cwd: &Path, root: &Path) {
    bind_session_with_root(session_id, cwd, Some(root.to_path_buf()));
}

fn bind_session_with_root(session_id: &str, cwd: &Path, root: Option<PathBuf>) {
    sessions().lock().expect("shell sessions").insert(
        session_id.to_string(),
        Session {
            cwd: cwd.to_path_buf(),
            root,
            env: HashMap::new(),
            jobs: HashMap::new(),
        },
    );
}

pub fn drop_session(session_id: &str) {
    let jobs = sessions()
        .lock()
        .expect("shell sessions")
        .remove(session_id)
        .map(|mut session| session.jobs.drain().map(|(_, job)| job).collect::<Vec<_>>());
    if let Some(jobs) = jobs {
        for mut job in jobs {
            let _ = kill_child(&mut job.child);
        }
    }
}

#[cfg(test)]
fn job_count(session_id: &str) -> usize {
    sessions()
        .lock()
        .expect("shell sessions")
        .get(session_id)
        .map(|session| session.jobs.len())
        .unwrap_or(0)
}

pub fn cwd(session_id: &str) -> Option<PathBuf> {
    sessions()
        .lock()
        .expect("shell sessions")
        .get(session_id)
        .map(|session| session.cwd.clone())
}

pub fn set_env(session_id: &str, key: &str, value: &str) -> Result<()> {
    let mut guard = sessions().lock().expect("shell sessions");
    let session = guard
        .get_mut(session_id)
        .ok_or_else(|| anyhow!("shell session {session_id} is not bound"))?;
    session.env.insert(key.to_string(), value.to_string());
    Ok(())
}

/// Model-visible env overlay. Secret-shaped keys are omitted, not redacted in place.
pub fn transcript_env(session_id: &str) -> Vec<(String, String)> {
    sessions()
        .lock()
        .expect("shell sessions")
        .get(session_id)
        .map(|session| {
            session
                .env
                .iter()
                .filter(|(key, _)| !is_secret_key(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// Apply a command to the session. A denial does not change cwd or start a job.
pub fn admit(
    session_id: &str,
    command: &str,
    admission: CommandAdmission,
) -> Result<AdmittedCommand> {
    let mut guard = sessions().lock().expect("shell sessions");
    let session = guard
        .get_mut(session_id)
        .ok_or_else(|| anyhow!("shell session {session_id} is not bound"))?;
    if admission == CommandAdmission::Deny {
        return Err(anyhow!("command denied inside shell session {session_id}"));
    }
    let trimmed = command.trim();
    if let Some(path) = simple_cd(trimmed) {
        let next = resolve_cd(&session.cwd, path);
        if let Some(root) = session.root.as_deref() {
            if !stays_under_root(&next, root) {
                return Err(anyhow!(
                    "cd cannot leave the isolation worktree for shell session {session_id}"
                ));
            }
        }
        session.cwd = next;
    }
    Ok(AdmittedCommand {
        cwd: session.cwd.clone(),
        command: trimmed.to_string(),
        transcript_env: session
            .env
            .iter()
            .filter(|(key, _)| !is_secret_key(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    })
}

pub fn detach(session_id: &str, command: &str, admission: CommandAdmission) -> Result<String> {
    let admitted = admit(session_id, command, admission)?;
    if admitted.command.starts_with("cd ") {
        return Err(anyhow!("cd is not a detachable job"));
    }
    let child = spawn(session_id, &admitted)?;
    let id = format!("job-{}", child.id());
    sessions()
        .lock()
        .expect("shell sessions")
        .get_mut(session_id)
        .ok_or_else(|| anyhow!("shell session {session_id} is not bound"))?
        .jobs
        .insert(id.clone(), Job { child });
    Ok(id)
}

pub fn poll(session_id: &str, job_id: &str) -> Result<String> {
    let mut guard = sessions().lock().expect("shell sessions");
    let job = guard
        .get_mut(session_id)
        .and_then(|session| session.jobs.get_mut(job_id))
        .ok_or_else(|| anyhow!("unknown job {job_id}"))?;
    match job.child.try_wait()? {
        Some(status) => Ok(format!("exited {status}")),
        None => Ok("running".to_string()),
    }
}

pub fn kill(session_id: &str, job_id: &str) -> Result<()> {
    let mut job = sessions()
        .lock()
        .expect("shell sessions")
        .get_mut(session_id)
        .and_then(|session| session.jobs.remove(job_id))
        .ok_or_else(|| anyhow!("unknown job {job_id}"))?;
    kill_child(&mut job.child)
}

/// Kill every detached job in the session. Used when the run is cancelled
/// so a timeout-independent cancel still reaps the process group.
pub fn kill_session(session_id: &str) {
    let jobs = sessions()
        .lock()
        .expect("shell sessions")
        .get_mut(session_id)
        .map(|session| session.jobs.drain().map(|(_, job)| job).collect::<Vec<_>>())
        .unwrap_or_default();
    for mut job in jobs {
        let _ = kill_child(&mut job.child);
    }
}

/// Run a command with a timeout and kill the process group if it expires.
pub fn run_bounded(session_id: &str, command: &str, timeout: Duration) -> Result<String> {
    let admitted = admit(session_id, command, CommandAdmission::Allow)?;
    if admitted.command.starts_with("cd ") {
        return Ok(format!("cwd {}", admitted.cwd.display()));
    }
    let mut child = spawn(session_id, &admitted)?;
    let started = std::time::Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok("exited".to_string());
        }
        if started.elapsed() >= timeout {
            kill_child(&mut child)?;
            return Err(anyhow!("shell timeout killed the process group"));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn spawn(session_id: &str, admitted: &AdmittedCommand) -> Result<Child> {
    let overlay = overlay_env(session_id);
    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("sh");
        command.arg("-c").arg(&admitted.command);
        command
    };
    #[cfg(windows)]
    let mut command = windows_detached_command(admitted, &overlay)?;
    command
        .current_dir(&admitted.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &overlay {
        command.env(key, value);
    }
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut child = command.spawn().map_err(|error| {
        anyhow!("failed to spawn detached shell for session {session_id}: {error}")
    })?;
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        if let Err(error) = crate::tools::builtin::bash::bind_windows_process_tree(
            child.as_raw_handle(),
            child.id(),
        ) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!(
                "failed to bind detached shell job for session {session_id}: {error}"
            ));
        }
    }
    Ok(child)
}

/// Detached jobs use the same host shell as foreground commands. Windows has
/// no `sh` on PATH; PowerShell 7 runs the compat shim so `printf` and `sleep`
/// still start a child the session can poll and kill.
#[cfg(windows)]
fn windows_detached_command(
    admitted: &AdmittedCommand,
    overlay: &HashMap<String, String>,
) -> Result<Command> {
    use std::os::windows::process::CommandExt;

    let powershell =
        crate::tools::builtin::bash::windows_host_powershell(&admitted.cwd).map_err(|error| {
            anyhow!(
                "failed to resolve PowerShell 7 for {}: {error}",
                admitted.cwd.display()
            )
        })?;
    let mut source = String::new();
    for key in overlay.keys() {
        if is_powershell_identifier(key) {
            source.push_str(&format!("${key} = $env:{key}\n"));
        }
    }
    source.push_str(&admitted.command);
    let wrapped = crate::tools::builtin::bash::build_powershell_command(&source);
    let encoded = crate::tools::builtin::bash::encode_powershell_command(&wrapped);
    let mut command = Command::new(&powershell);
    command.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
    ]);
    let encoded_chars = format!("{powershell:?} -EncodedCommand {encoded}")
        .encode_utf16()
        .count();
    if encoded_chars <= 30_000 {
        command.arg("-EncodedCommand").arg(encoded);
    } else {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("a3s-detach-{}-{unique}.ps1", std::process::id()));
        let literal = path.to_string_lossy().replace('\'', "''");
        let body = format!(
            "trap {{ exit 1 }}\nRegister-EngineEvent -SourceIdentifier PowerShell.Exiting -Action {{ Remove-Item -LiteralPath '{literal}' -Force -ErrorAction SilentlyContinue }} | Out-Null\n{wrapped}\nif (-not $?) {{ exit 1 }}\n"
        );
        std::fs::write(&path, body.as_bytes())
            .map_err(|error| anyhow!("failed to write detached shell script: {error}"))?;
        command.arg("-File").arg(&path);
    }
    command.creation_flags(crate::tools::builtin::bash::CREATE_NO_WINDOW);
    Ok(command)
}

#[cfg(windows)]
fn is_powershell_identifier(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn kill_child(child: &mut Child) -> Result<()> {
    // Unix kills the process group. On Windows the shell is assigned to a
    // Job Object at spawn; ending this process makes that job terminate the
    // remaining descendants.
    #[cfg(unix)]
    let pid = child.id();
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    let _ = child.wait();
    #[cfg(unix)]
    unsafe {
        let gone = libc::kill(pid as i32, 0) != 0;
        if !gone {
            return Err(anyhow!("child {pid} survived process-group kill"));
        }
    }
    Ok(())
}

/// Env overlay for the named session only. A shared cwd is not an identity.
fn overlay_env(session_id: &str) -> HashMap<String, String> {
    sessions()
        .lock()
        .expect("shell sessions")
        .get(session_id)
        .map(|session| session.env.clone())
        .unwrap_or_default()
}

fn simple_cd(command: &str) -> Option<&str> {
    let rest = command.strip_prefix("cd ")?.trim();
    if rest.is_empty()
        || rest.contains("&&")
        || rest.contains(';')
        || rest.contains('|')
        || rest.contains('>')
    {
        return None;
    }
    Some(rest)
}

fn resolve_cd(cwd: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn stays_under_root(next: &Path, root: &Path) -> bool {
    let lexical = normalize_lexical(next);
    let root = normalize_lexical(root);
    if !lexical.starts_with(&root) {
        return false;
    }
    let Ok(canonical) = std::fs::canonicalize(&lexical) else {
        return true;
    };
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    canonical.starts_with(root)
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn is_secret_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    SECRET_MARKERS.iter().any(|marker| upper.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cd_is_visible_in_the_same_session_and_not_in_another() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        bind_session("a", root.path());
        bind_session("b", root.path());
        admit("a", "cd nested", CommandAdmission::Allow).unwrap();
        assert_eq!(cwd("a").unwrap(), nested);
        assert_eq!(cwd("b").unwrap(), root.path());
        let denied = admit("a", "cd ..", CommandAdmission::Deny);
        assert!(denied.is_err());
        assert_eq!(cwd("a").unwrap(), nested);
        let denied_again = admit("a", "touch leaked.txt", CommandAdmission::Deny);
        assert!(denied_again
            .unwrap_err()
            .to_string()
            .contains("command denied inside shell session"));
        assert_eq!(cwd("a").unwrap(), nested);
        assert!(!nested.join("leaked.txt").exists());
        drop_session("a");
        drop_session("b");
    }

    #[test]
    fn detached_job_outlives_the_return_and_can_be_polled_and_killed() {
        let root = tempfile::tempdir().unwrap();
        bind_session("jobs", root.path());
        let id = detach("jobs", "sleep 30", CommandAdmission::Allow).unwrap();
        assert_eq!(poll("jobs", &id).unwrap(), "running");
        kill("jobs", &id).unwrap();
        assert!(poll("jobs", &id).is_err());
        drop_session("jobs");
    }

    #[test]
    fn command_overlay_does_not_cross_sessions_that_share_a_cwd() {
        let root = tempfile::tempdir().unwrap();
        let owner = "overlay-owner";
        let other = "overlay-other";
        bind_session(owner, root.path());
        bind_session(other, root.path());
        set_env(owner, "A3S_SHELL_OVERLAY", "owner-only").unwrap();
        set_env(other, "A3S_SHELL_OVERLAY", "other-only").unwrap();
        assert_eq!(
            overlay_env(other)
                .get("A3S_SHELL_OVERLAY")
                .map(String::as_str),
            Some("other-only")
        );
        run_bounded(
            other,
            "printf '%s' \"$A3S_SHELL_OVERLAY\" > other-overlay.txt",
            Duration::from_secs(5),
        )
        .unwrap();
        let written = std::fs::read_to_string(root.path().join("other-overlay.txt")).unwrap();
        assert_eq!(
            written.trim(),
            "other-only",
            "the writer session must record its own overlay, not the other session"
        );
        assert!(
            !written.contains("owner-only"),
            "overlay leaked across sessions that share a cwd: {written:?}"
        );
        drop_session(owner);
        drop_session(other);
    }

    #[test]
    fn cancel_kills_detached_jobs_in_the_session() {
        let root = tempfile::tempdir().unwrap();
        bind_session("cancel", root.path());
        let id = detach("cancel", "sleep 30", CommandAdmission::Allow).unwrap();
        assert_eq!(poll("cancel", &id).unwrap(), "running");
        kill_session("cancel");
        assert!(poll("cancel", &id).is_err());
        drop_session("cancel");
    }

    #[test]
    fn timeout_leaves_no_child_process() {
        let root = tempfile::tempdir().unwrap();
        bind_session("timeout", root.path());
        let error = run_bounded("timeout", "sleep 30", Duration::from_millis(80)).unwrap_err();
        assert!(error.to_string().contains("killed the process group"));
        drop_session("timeout");
    }

    #[cfg(windows)]
    #[test]
    fn killing_a_detached_job_stops_its_descendant_before_a_later_write() {
        let root = tempfile::tempdir().unwrap();
        let child_started = root.path().join("child-started");
        let leaked = root.path().join("leaked");
        let child_started_literal = child_started.to_string_lossy().replace('\'', "''");
        let leaked_literal = leaked.to_string_lossy().replace('\'', "''");
        let powershell = crate::tools::builtin::bash::windows_host_powershell(root.path())
            .expect("PowerShell 7");
        let powershell_literal = powershell.to_string_lossy().replace('\'', "''");
        let command = format!(
            "$child = Start-Process -FilePath '{powershell_literal}' -PassThru -WindowStyle Hidden \
             -ArgumentList '-NoLogo','-NoProfile','-NonInteractive','-Command','Set-Content -LiteralPath ''{child_started_literal}'' -Value started; Start-Sleep -Seconds 1; Set-Content -LiteralPath ''{leaked_literal}'' -Value leaked'; \
             Wait-Process -Id $child.Id"
        );
        let session = format!("detach-tree-{}", std::process::id());
        bind_session(&session, root.path());
        let id = detach(&session, &command, CommandAdmission::Allow).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        while !child_started.exists() {
            assert!(
                deadline > std::time::Instant::now(),
                "descendant did not start before the detached shell was killed"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        kill(&session, &id).unwrap();
        std::thread::sleep(Duration::from_millis(1_200));
        drop_session(&session);
        assert!(
            !leaked.exists(),
            "killing a detached job must stop a descendant before its later write"
        );
    }

    #[cfg(windows)]
    #[test]
    fn timing_out_a_shell_stops_its_descendant_before_a_later_write() {
        let root = tempfile::tempdir().unwrap();
        let child_started = root.path().join("child-started");
        let leaked = root.path().join("leaked");
        let child_started_literal = child_started.to_string_lossy().replace('\'', "''");
        let leaked_literal = leaked.to_string_lossy().replace('\'', "''");
        let powershell = crate::tools::builtin::bash::windows_host_powershell(root.path())
            .expect("PowerShell 7");
        let powershell_literal = powershell.to_string_lossy().replace('\'', "''");
        let command = format!(
            "$child = Start-Process -FilePath '{powershell_literal}' -PassThru -WindowStyle Hidden \
             -ArgumentList '-NoLogo','-NoProfile','-NonInteractive','-Command','Set-Content -LiteralPath ''{child_started_literal}'' -Value started; Start-Sleep -Seconds 30; Set-Content -LiteralPath ''{leaked_literal}'' -Value leaked'; \
             Wait-Process -Id $child.Id"
        );
        let session = format!("timeout-tree-{}", std::process::id());
        bind_session(&session, root.path());
        let session_for_run = session.clone();
        let runner = std::thread::spawn(move || {
            run_bounded(&session_for_run, &command, Duration::from_secs(8))
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        while !child_started.exists() {
            assert!(
                deadline > std::time::Instant::now(),
                "descendant did not start before the shell timeout"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let error = runner.join().expect("timeout runner").unwrap_err();
        assert!(
            error.to_string().contains("killed the process group"),
            "timeout did not report the process-group kill: {error}"
        );
        std::thread::sleep(Duration::from_millis(1_200));
        drop_session(&session);
        assert!(
            !leaked.exists(),
            "a shell timeout must stop a descendant before its later write"
        );
    }

    #[test]
    fn transcript_omits_secret_env() {
        let root = tempfile::tempdir().unwrap();
        bind_session("env", root.path());
        set_env("env", "PATH", "/usr/bin").unwrap();
        set_env("env", "OPENAI_API_KEY", "sk-secret").unwrap();
        let visible = transcript_env("env");
        assert!(visible.iter().any(|(key, _)| key == "PATH"));
        assert!(visible.iter().all(|(key, _)| key != "OPENAI_API_KEY"));
        drop_session("env");
    }

    /// S-WT-01: one rooted session, 100 commands, including escapes.
    #[test]
    #[ignore = "S-WT-01 rooted shell soak"]
    fn soak_rooted_shell_keeps_cwd_inside_one_hundred_commands() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        let outside =
            std::env::temp_dir().join(format!("a3s-shell-escape-{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&outside);
        let session = format!("soak-wt-{}", std::process::id());
        bind_session_rooted(&session, root.path(), root.path());
        let parent = root.path().parent().expect("parent");
        let absolute_escape = format!("cd {}", parent.display());

        for index in 0..100 {
            let before = cwd(&session).expect("cwd");
            let command = match index % 4 {
                0 => "cd nested",
                1 => "cd ..",
                2 => absolute_escape.as_str(),
                _ => "cd ..",
            };
            let result = admit(&session, command, CommandAdmission::Allow);
            let after = cwd(&session).expect("cwd");
            assert!(
                stays_under_root(&after, root.path()),
                "cycle {index} cwd left the root: {after:?}"
            );
            if result.is_err() {
                assert_eq!(before, after, "denied cd moved cwd on cycle {index}");
            }
            assert_eq!(
                job_count(&session),
                0,
                "cycle {index} leaked a detached job"
            );
            assert!(
                !outside.exists(),
                "cycle {index} created a file outside the root"
            );
        }

        drop_session(&session);
        let _ = std::fs::remove_file(&outside);
    }
}
