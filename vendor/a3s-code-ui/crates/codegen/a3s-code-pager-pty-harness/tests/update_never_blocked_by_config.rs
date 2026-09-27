//! `a3s update` is a recovery command: a config failure must not block it, and a WinGet install hands off to WinGet.
//!
//! A local server serves the channel pointer and records each request path.
//! The config test serves the binary's own version, so a healthy run exits 0 ("already up to date").
//! A run with a corrupt config must exit 0 too; reintroducing a config `?` fails exactly that run.
//! The pointer must equal the current version: the installer converges in both directions, so an older pointer triggers a downgrade attempt.
//!
//! A copy of the binary inside a WinGet package dir must exit 0, print the WinGet command, and write no update state,
//! even with a stale `installer` in config or an npm env hint. Without an org version cap it makes no update request;
//! with one it reads only the stable pointer and names the exact allowed version, or no command when nothing is
//! allowed or it is already there. `--check` must not save a channel switch, and a `--version` pin below the org floor
//! must fail before the hand-off.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};

use a3s_code_pager_pty_harness::pager_binary;
use serde_json::Value;

// A child forked while the copy's write fd is open, even pager_binary's cargo build, fails the copy's exec with "Text file busy".
static EXEC_LOCK: Mutex<()> = Mutex::new(());

/// Spawn a local server that answers every request with the channel pointer body and records each request path.
fn spawn_pointer_server(
    body: Arc<Mutex<String>>,
    requests: Arc<Mutex<Vec<String>>>,
) -> (std::net::TcpListener, String) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = listener.try_clone().unwrap();
    std::thread::spawn(move || {
        for stream in serving.incoming() {
            let Ok(stream) = stream else { return };
            let mut reader = BufReader::new(&stream);
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            if let Some(path) = request_line.split_whitespace().nth(1) {
                requests
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(path.to_owned());
            }
            // Drain headers: unread input at close resets the connection and can drop the reply.
            let mut header = String::new();
            while reader.read_line(&mut header).is_ok_and(|n| n > 0) && header != "\r\n" {
                header.clear();
            }
            let version = body.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let _ = (&stream).write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    version.len(),
                    version
                )
                .as_bytes(),
            );
        }
    });
    (listener, base)
}

/// `exe` with an isolated `home`, pointed at the local pointer base.
fn grok_command(exe: &Path, home: &Path, base: &str) -> Command {
    let mut command = Command::new(exe);
    command
        .env_clear()
        .env("HOME", home)
        .env("GROK_HOME", home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("GROK_CLI_BASE_URL", base);
    a3s_tty_utils::detach_std_command(&mut command);
    command
}

/// Run `command` to completion, holding [`EXEC_LOCK`] for the spawn only.
fn output(mut command: Command) -> Output {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[allow(clippy::disallowed_methods)] // waited on right below
    let child = {
        let _exec = EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        command.spawn().expect("spawn grok")
    };
    child.wait_with_output().expect("wait for grok")
}

/// Run `a3s update` in a fresh isolated home against the local pointer base.
fn run_update(base: &str, config_toml: &str, extra_args: &[&str]) -> Output {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.toml"), config_toml).unwrap();
    let exe = {
        let _exec = EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        pager_binary().expect("resolve pager binary")
    };
    let mut command = grok_command(&exe, home.path(), base);
    command.arg("update").args(extra_args);
    output(command)
}

/// Copies (never links) the binary into a user-scope WinGet package dir, so the running exe's path is the package path.
fn copy_into_winget_package(root: &Path) -> PathBuf {
    let package = root.join(
        "Local/Microsoft/WinGet/Packages/A3S.GrokBuild_Microsoft.Winget.Source_8wekyb3d8bbwe",
    );
    std::fs::create_dir_all(&package).unwrap();
    let exe = package.join(format!("grok{}", std::env::consts::EXE_SUFFIX));
    let _exec = EXEC_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::fs::copy(pager_binary().expect("resolve pager binary"), &exe).unwrap();
    exe
}

/// Update artifacts present under `home`, among those the updater writes.
fn update_artifacts(home: &Path) -> Vec<&'static str> {
    ["bin", "downloads", "version.json"]
        .into_iter()
        .filter(|name| home.join(name).exists())
        .collect()
}

/// The valid run proves the environment resolves to success, so a nonzero corrupt run can only mean a config failure aborted the update.
#[test]
fn corrupt_config_never_changes_update_outcome() {
    let body = Arc::new(Mutex::new("0.0.1".to_owned()));
    let (_listener, base) = spawn_pointer_server(body.clone(), Arc::default());

    // Probe the binary's own version so the pointer matches it exactly.
    let check = run_update(&base, "[cli]\n", &["--check", "--json"]);
    let status: Value = serde_json::from_slice(&check.stdout)
        .unwrap_or_else(|e| panic!("update --check --json must emit JSON: {e}"));
    let current = status["currentVersion"]
        .as_str()
        .expect("currentVersion in update --check --json")
        .to_owned();
    *body.lock().unwrap_or_else(|e| e.into_inner()) = current;

    let valid = run_update(&base, "[cli]\n", &[]);
    assert!(
        valid.status.success(),
        "healthy grok update against the local base must exit 0\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&valid.stdout),
        String::from_utf8_lossy(&valid.stderr)
    );

    let corrupt = run_update(&base, "this is not toml {{{[[[", &[]);
    assert!(
        corrupt.status.success(),
        "a corrupt config.toml must not block grok update\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&corrupt.stdout),
        String::from_utf8_lossy(&corrupt.stderr)
    );
}

#[test]
fn winget_install_update_hands_off_without_update_writes() {
    let body = Arc::new(Mutex::new("999.0.0".to_owned()));
    let requests: Arc<Mutex<Vec<String>>> = Arc::default();
    let (_listener, base) = spawn_pointer_server(body, requests.clone());
    let package_root = tempfile::tempdir().unwrap();
    let exe = copy_into_winget_package(package_root.path());
    let home = tempfile::tempdir().unwrap();
    let config = "[cli]\ninstaller = \"internal\"\nchannel = \"alpha\"\n";
    std::fs::write(home.path().join("config.toml"), config).unwrap();

    // The second run adds an npm hint, which must not outrank the WinGet location.
    let runs: [&[(&str, &str)]; 2] = [
        &[],
        &[("npm_config_user_agent", "npm/10.8.0 node/v22 win32 x64")],
    ];
    for envs in runs {
        let mut command = grok_command(&exe, home.path(), &base);
        command.arg("update").envs(envs.iter().copied());
        let update = output(command);
        let stderr = String::from_utf8_lossy(&update.stderr);
        assert!(
            update.status.success(),
            "grok update on a WinGet install must exit 0 (env {envs:?})\nstderr:\n{stderr}"
        );
        assert!(
            stderr.contains("winget upgrade --id A3S.GrokBuild -e")
                && stderr.contains("WinGet ships only the stable channel"),
            "grok update must hand off to WinGet and flag the ignored alpha channel (env {envs:?})\nstderr:\n{stderr}"
        );
        let logged = requests.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(Vec::<String>::new(), logged);
        assert_eq!(Vec::<&str>::new(), update_artifacts(home.path()));
        assert_eq!(
            config,
            std::fs::read_to_string(home.path().join("config.toml")).unwrap()
        );
    }

    let mut pinned = grok_command(&exe, home.path(), &base);
    pinned
        .args(["update", "--version", "0.0.1"])
        .env("GROK_REQUIRED_MINIMUM_VERSION", "0.0.2");
    let pinned = output(pinned);
    let stderr = String::from_utf8_lossy(&pinned.stderr);
    assert!(
        !pinned.status.success()
            && stderr.contains("the minimum allowed version is 0.0.2")
            && !stderr.contains("winget install"),
        "a pin below the org floor must fail before the WinGet hand-off\nstderr:\n{stderr}"
    );

    struct CappedCase {
        envs: &'static [(&'static str, &'static str)],
        succeeds: bool,
        expected: &'static str,
        prints_install: bool,
    }
    let install_5 = "winget install --id A3S.GrokBuild -e --version 5.0.0 --force";
    let capped_cases = [
        CappedCase {
            envs: &[
                ("GROK_MAXIMUM_VERSION", "5.0.0"),
                ("GROK_TEST_VERSION", "1.0.0"),
            ],
            succeeds: true,
            expected: install_5,
            prints_install: true,
        },
        CappedCase {
            envs: &[
                ("GROK_REQUIRED_MAXIMUM_VERSION", "5.0.0"),
                ("GROK_TEST_VERSION", "6.0.0"),
            ],
            succeeds: true,
            expected: install_5,
            prints_install: true,
        },
        CappedCase {
            envs: &[
                ("GROK_MAXIMUM_VERSION", "5.0.0"),
                ("GROK_TEST_VERSION", "5.0.0"),
            ],
            succeeds: true,
            expected: "Already up to date (5.0.0).",
            prints_install: false,
        },
        CappedCase {
            envs: &[
                ("GROK_MAXIMUM_VERSION", "5.0.0"),
                ("GROK_TEST_VERSION", "6.0.0"),
            ],
            succeeds: true,
            expected: "Already up to date (6.0.0).",
            prints_install: false,
        },
        CappedCase {
            envs: &[
                ("GROK_MAXIMUM_VERSION", "5.0.0"),
                ("GROK_MINIMUM_VERSION", "6.0.0"),
            ],
            succeeds: true,
            expected: "is not an allowed update",
            prints_install: false,
        },
        CappedCase {
            envs: &[
                ("GROK_MAXIMUM_VERSION", "2000.0.0"),
                ("GROK_REQUIRED_MINIMUM_VERSION", "1000.0.0"),
            ],
            succeeds: false,
            expected: "newer than the latest available release (999.0.0)",
            prints_install: false,
        },
    ];
    // `winget upgrade` would jump past the cap, so a capped org never gets it.
    for case in capped_cases {
        let mut command = grok_command(&exe, home.path(), &base);
        command.arg("update").envs(case.envs.iter().copied());
        let update = output(command);
        let stderr = String::from_utf8_lossy(&update.stderr);
        assert!(
            update.status.success() == case.succeeds
                && stderr.contains(case.expected)
                && !stderr.contains("winget upgrade")
                && (case.prints_install || !stderr.contains("winget install")),
            "capped WinGet update (env {:?}) must print {:?}\nstderr:\n{stderr}",
            case.envs,
            case.expected
        );
        assert_eq!(Vec::<&str>::new(), update_artifacts(home.path()));
    }

    let mut check = grok_command(&exe, home.path(), &base);
    check.args(["update", "--check", "--json", "--enterprise"]);
    let status: Value = serde_json::from_slice(&output(check).stdout)
        .unwrap_or_else(|e| panic!("update --check --json must emit JSON: {e}"));
    assert_eq!(
        (Some("winget"), Some(true), Some("stable")),
        (
            status.get("installer").and_then(Value::as_str),
            status.get("updateAvailable").and_then(Value::as_bool),
            status.get("channel").and_then(Value::as_str),
        )
    );
    let logged = requests.lock().unwrap_or_else(|e| e.into_inner()).clone();
    assert!(
        !logged.is_empty() && logged.iter().all(|path| path == "/stable"),
        "a WinGet --check reads only the stable pointer: {logged:?}"
    );
    assert_eq!(vec!["version.json"], update_artifacts(home.path()));
    assert_eq!(
        config,
        std::fs::read_to_string(home.path().join("config.toml")).unwrap()
    );

    let mut explicit = grok_command(&exe, home.path(), &base);
    explicit
        .args(["update", "--check", "--json"])
        .env("GROK_INSTALLER", "internal");
    let status: Value = serde_json::from_slice(&output(explicit).stdout)
        .unwrap_or_else(|e| panic!("update --check --json must emit JSON: {e}"));
    assert_eq!(
        Some("internal"),
        status.get("installer").and_then(Value::as_str)
    );
}
