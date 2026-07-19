use super::*;
use crate::sandbox::BashSandbox;
use std::sync::Arc;
use tempfile::TempDir;

#[cfg(unix)]
fn fake_srt(capture_path: &Path) -> (TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("srt");
    std::fs::write(
        &binary,
        format!(
            "#!/bin/sh\n\
             if [ \"$1\" = \"--settings\" ]; then\n\
               cp \"$2\" '{}'\n\
               shift 2\n\
             fi\n\
             if [ \"$1\" = \"--\" ]; then\n\
               shift\n\
             fi\n\
             exec \"$@\"\n",
            capture_path.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&binary, permissions).unwrap();
    (directory, binary)
}

#[cfg(unix)]
#[tokio::test]
async fn adapter_passes_argv_workspace_timeout_env_and_settings() {
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join("services/api")).unwrap();
    std::fs::write(workspace.path().join("services/api/.env"), "SECRET=hidden").unwrap();
    let capture = workspace.path().join("captured-settings.json");
    let (_binary_dir, binary) = fake_srt(&capture);
    let sandbox = SrtBashSandbox::new(binary, workspace.path()).unwrap();
    let output = sandbox
        .exec(SandboxCommandRequest {
            command: "printf '%s|%s' \"$PWD\" \"$EXPLICIT_VALUE\"".to_string(),
            guest_workspace: "/workspace".to_string(),
            timeout_ms: 5_000,
            output_observer: None,
            env: Some(Arc::new(HashMap::from([(
                "EXPLICIT_VALUE".to_string(),
                "kept".to_string(),
            )]))),
        })
        .await
        .unwrap();

    assert!(!output.timed_out);
    assert_eq!(output.exit_code, 0);
    assert_eq!(
        output.stdout,
        format!(
            "{}|kept",
            workspace.path().canonicalize().unwrap().display()
        )
    );
    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(capture).unwrap()).unwrap();
    assert_eq!(settings["network"]["allowedDomains"], json!([]));
    assert_eq!(settings["network"]["allowLocalBinding"], false);
    assert!(settings["filesystem"]["allowWrite"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == &json!(workspace.path().canonicalize().unwrap())));
    assert!(settings["filesystem"]["denyWrite"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path.as_str().unwrap().ends_with("/.a3s")));
    assert!(settings["filesystem"]["denyWrite"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path.as_str().unwrap().ends_with("/.claude")));
    assert!(settings["filesystem"]["denyWrite"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path.as_str().unwrap().ends_with("/.mcp.json")));
    assert!(settings["filesystem"]["allowRead"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == &json!(workspace.path().canonicalize().unwrap())));
    assert!(settings["filesystem"]["denyRead"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path.as_str().unwrap().ends_with("/.codex/auth.json")));
    assert!(settings["filesystem"]["denyRead"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path == &json!(workspace.path().canonicalize().unwrap().join(".env"))));
    assert!(settings["filesystem"]["denyRead"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path
            == &json!(workspace
                .path()
                .canonicalize()
                .unwrap()
                .join("services/api/.env"))));
    assert!(settings["filesystem"]["denyWrite"]
        .as_array()
        .unwrap()
        .iter()
        .any(|path| path.as_str().unwrap().ends_with("/.codex/auth.json")));
    assert_eq!(settings["enableWeakerNestedSandbox"], false);
    assert_eq!(settings["allowAppleEvents"], false);
}

#[cfg(unix)]
#[tokio::test]
async fn adapter_denies_every_preexisting_workspace_hardlink_alias() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(workspace.join("nested")).unwrap();
    let outside = root.path().join("outside-secret");
    let alias = workspace.join("nested").join("apparently-safe.txt");
    std::fs::write(&outside, "outside-secret").unwrap();
    std::fs::hard_link(&outside, &alias).unwrap();
    let canonical_alias = alias.canonicalize().unwrap();

    let capture = root.path().join("captured-settings.json");
    let (_binary_dir, binary) = fake_srt(&capture);
    let sandbox = SrtBashSandbox::new(binary, &workspace).unwrap();
    sandbox
        .exec_command("printf ready", "/workspace")
        .await
        .unwrap();

    let settings: serde_json::Value =
        serde_json::from_slice(&std::fs::read(capture).unwrap()).unwrap();
    for boundary in ["denyRead", "denyWrite"] {
        assert!(
            settings["filesystem"][boundary]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path == &json!(canonical_alias)),
            "{boundary} omitted a multi-link workspace alias"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn adapter_preserves_stdout_and_stderr_as_separate_streams() {
    let workspace = tempfile::tempdir().unwrap();
    let capture = workspace.path().join("captured-settings.json");
    let (_binary_dir, binary) = fake_srt(&capture);
    let sandbox = SrtBashSandbox::new(binary, workspace.path()).unwrap();

    let output = sandbox
        .exec(SandboxCommandRequest {
            command: "printf stdout-value; printf stderr-value >&2".to_string(),
            guest_workspace: "/workspace".to_string(),
            timeout_ms: 5_000,
            output_observer: None,
            env: None,
        })
        .await
        .unwrap();

    assert!(!output.timed_out);
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, "stdout-value");
    assert_eq!(output.stderr, "stderr-value");
}

#[test]
fn child_environment_drops_ambient_secrets_and_pins_scratch_paths() {
    let scratch = tempfile::tempdir().unwrap();
    let environment = compose_child_env(None, scratch.path()).unwrap();

    assert!(!environment.contains_key(OsStr::new("OPENAI_API_KEY")));
    assert!(!environment.contains_key(OsStr::new("ANTHROPIC_API_KEY")));
    assert!(!environment.contains_key(OsStr::new("SSH_AUTH_SOCK")));
    assert_eq!(
        environment.get(OsStr::new("HOME")),
        Some(&scratch.path().as_os_str().to_os_string())
    );
    assert_eq!(
        environment.get(OsStr::new("TMPDIR")),
        Some(&scratch.path().as_os_str().to_os_string())
    );
    assert_eq!(
        environment.get(OsStr::new("XDG_CONFIG_HOME")),
        Some(&scratch.path().as_os_str().to_os_string())
    );
}

#[test]
fn child_environment_rejects_explicit_bootstrap_injection_variables() {
    let scratch = tempfile::tempdir().unwrap();
    let explicit = HashMap::from([
        ("SAFE_VALUE".to_string(), "kept".to_string()),
        ("BASH_ENV".to_string(), "/tmp/bash-hook".to_string()),
        (
            "node_options".to_string(),
            "--require=/tmp/hook.js".to_string(),
        ),
        ("NODE_PATH".to_string(), "/tmp/node-modules".to_string()),
        ("PYTHONPATH".to_string(), "/tmp/python".to_string()),
        ("RUBYOPT".to_string(), "-r/tmp/hook.rb".to_string()),
        ("PERL5OPT".to_string(), "-M/tmp/hook.pm".to_string()),
        ("LUA_INIT_5_4".to_string(), "@/tmp/hook.lua".to_string()),
        ("LD_PRELOAD".to_string(), "/tmp/hook.so".to_string()),
        (
            "DYLD_INSERT_LIBRARIES".to_string(),
            "/tmp/hook.dylib".to_string(),
        ),
        (
            "JAVA_TOOL_OPTIONS".to_string(),
            "-javaagent:/tmp/hook.jar".to_string(),
        ),
    ]);

    let environment = compose_child_env(Some(&explicit), scratch.path()).unwrap();

    assert_eq!(
        environment.get(OsStr::new("SAFE_VALUE")),
        Some(&OsString::from("kept"))
    );
    for key in explicit.keys().filter(|key| key.as_str() != "SAFE_VALUE") {
        assert!(
            !environment
                .keys()
                .any(|candidate| candidate.to_string_lossy().eq_ignore_ascii_case(key)),
            "{key} must not reach the sandbox child"
        );
    }
}

#[test]
fn supported_srt_version_range_is_explicit() {
    for version in ["0.0.66", "v0.0.66", "0.0.99-beta.1"] {
        ensure_supported_srt_version(version).unwrap();
    }
    for version in ["0.0.65", "0.1.0", "1.0.0", "unknown"] {
        assert!(
            ensure_supported_srt_version(version).is_err(),
            "{version} must not enter the trusted discovery path"
        );
    }
}

#[cfg(unix)]
#[test]
fn verified_installation_uses_the_expected_package_manifest_and_cli() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let package = root
        .path()
        .join("node_modules/@anthropic-ai/sandbox-runtime");
    let cli = package.join("dist/cli.js");
    std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
    std::fs::write(
        package.join("package.json"),
        serde_json::json!({
            "name": SRT_NPM_PACKAGE_NAME,
            "version": "0.0.66"
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(&cli, "#!/usr/bin/env node\n").unwrap();
    let mut permissions = std::fs::metadata(&cli).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&cli, permissions).unwrap();

    let installation = inspect_srt_installation(&cli).unwrap();
    assert_eq!(installation.cli, cli.canonicalize().unwrap());
    assert_eq!(installation.version, "0.0.66");
}

#[cfg(unix)]
#[test]
fn workspace_cannot_supply_the_sandbox_executable() {
    use std::os::unix::fs::PermissionsExt;

    let workspace = tempfile::tempdir().unwrap();
    let binary = workspace.path().join("srt");
    std::fs::write(&binary, "#!/bin/sh\nexec \"$@\"\n").unwrap();
    let mut permissions = std::fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&binary, permissions).unwrap();

    let error = SrtBashSandbox::new(&binary, workspace.path()).unwrap_err();
    assert!(error.to_string().contains("inside the active workspace"));
}

#[test]
fn sandbox_runtime_requires_an_explicit_path() {
    let workspace = tempfile::tempdir().unwrap();
    let error = SrtBashSandbox::new("srt", workspace.path()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "an explicit SRT executable path is required; PATH discovery is unsupported"
    );
}

#[test]
fn sensitive_paths_cover_agent_cloud_and_package_credentials() {
    let paths = default_sensitive_paths(Path::new("/home/a3s-test"));
    for suffix in [
        ".ssh",
        ".aws",
        ".kube",
        ".docker",
        ".codex/auth.json",
        ".claude/.credentials.json",
        ".git-credentials",
        "credentials/kimi-code.json",
        ".a3s/os-auth.json",
        ".cargo/credentials.toml",
    ] {
        assert!(
            paths.iter().any(|path| path.ends_with(suffix)),
            "missing sensitive path {suffix}"
        );
    }
}

/// Run explicitly with `A3S_TEST_SRT_BIN=/absolute/path/to/srt` to verify
/// the real OS boundary rather than the fake argv adapter above.
#[tokio::test]
#[ignore = "requires an installed srt runtime"]
async fn real_srt_allows_workspace_writes_and_blocks_outside_writes() {
    let binary = std::env::var_os("A3S_TEST_SRT_BIN")
        .map(PathBuf::from)
        .expect("set A3S_TEST_SRT_BIN");
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let outside = root.path().join("outside");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let sandbox = SrtBashSandbox::new(binary, &workspace).unwrap();

    let allowed = sandbox
        .exec_command("printf ok > inside.txt", "/workspace")
        .await
        .unwrap();
    assert_eq!(allowed.exit_code, 0, "{}", allowed.stderr);
    assert_eq!(
        std::fs::read_to_string(workspace.join("inside.txt")).unwrap(),
        "ok"
    );

    let denied = sandbox
        .exec_command(
            &format!("printf escaped > {}/escaped.txt", outside.display()),
            "/workspace",
        )
        .await
        .unwrap();
    assert_ne!(denied.exit_code, 0);
    assert!(!outside.join("escaped.txt").exists());

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, workspace.join("outside-link")).unwrap();
        let denied = sandbox
            .exec_command(
                "printf escaped > outside-link/symlink-escaped.txt",
                "/workspace",
            )
            .await
            .unwrap();
        assert_ne!(denied.exit_code, 0);
        assert!(!outside.join("symlink-escaped.txt").exists());
    }
}

/// Run explicitly with `A3S_TEST_SRT_BIN=/absolute/path/to/srt`.
#[tokio::test]
#[ignore = "requires an installed srt runtime"]
async fn real_srt_denies_network_and_protected_workspace_metadata() {
    let binary = std::env::var_os("A3S_TEST_SRT_BIN")
        .map(PathBuf::from)
        .expect("set A3S_TEST_SRT_BIN");
    let workspace = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
    std::fs::create_dir_all(workspace.path().join(".a3s")).unwrap();
    std::fs::create_dir_all(workspace.path().join(".claude")).unwrap();
    std::fs::write(workspace.path().join(".git/config"), "original-git").unwrap();
    std::fs::write(workspace.path().join(".a3s/policy.acl"), "original-policy").unwrap();
    std::fs::write(
        workspace.path().join(".claude/settings.json"),
        "original-agent-settings",
    )
    .unwrap();
    let sandbox = SrtBashSandbox::new(binary, workspace.path()).unwrap();

    let network = sandbox
        .exec_command(
            "curl --connect-timeout 2 --max-time 5 -fsS https://example.com >/dev/null",
            "/workspace",
        )
        .await
        .unwrap();
    assert_ne!(
        network.exit_code, 0,
        "network egress unexpectedly succeeded: {}{}",
        network.stdout, network.stderr
    );

    let local_binding = sandbox
        .exec_command(
            "python3 -c 'import socket; s=socket.socket(); s.bind((\"127.0.0.1\", 0))'",
            "/workspace",
        )
        .await
        .unwrap();
    assert_ne!(
        local_binding.exit_code, 0,
        "local binding unexpectedly succeeded: {}{}",
        local_binding.stdout, local_binding.stderr
    );

    let unix_socket = sandbox
        .exec_command(
            "python3 -c 'import socket; s=socket.socket(socket.AF_UNIX); s.bind(\"blocked.sock\")'",
            "/workspace",
        )
        .await
        .unwrap();
    assert_ne!(
        unix_socket.exit_code, 0,
        "Unix socket binding unexpectedly succeeded: {}{}",
        unix_socket.stdout, unix_socket.stderr
    );
    assert!(!workspace.path().join("blocked.sock").exists());

    for (command, path, original) in [
        (
            "printf changed > .git/config",
            ".git/config",
            "original-git",
        ),
        (
            "printf changed > .a3s/policy.acl",
            ".a3s/policy.acl",
            "original-policy",
        ),
        (
            "printf changed > .claude/settings.json",
            ".claude/settings.json",
            "original-agent-settings",
        ),
    ] {
        let denied = sandbox.exec_command(command, "/workspace").await.unwrap();
        assert_ne!(denied.exit_code, 0, "{command} unexpectedly succeeded");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(path)).unwrap(),
            original
        );
    }

    let create_protected = sandbox
        .exec_command(
            "mkdir -p .codex && printf changed > .codex/config",
            "/workspace",
        )
        .await
        .unwrap();
    assert_ne!(
        create_protected.exit_code, 0,
        "creating protected metadata unexpectedly succeeded"
    );
    assert!(!workspace.path().join(".codex/config").exists());

    let create_protected_file = sandbox
        .exec_command("printf changed > .mcp.json", "/workspace")
        .await
        .unwrap();
    assert_ne!(
        create_protected_file.exit_code, 0,
        "creating a protected control file unexpectedly succeeded"
    );
    assert!(!workspace.path().join(".mcp.json").exists());
}

/// Run explicitly with `A3S_TEST_SRT_BIN=/absolute/path/to/srt`.
#[tokio::test]
#[ignore = "requires an installed srt runtime"]
async fn real_srt_limits_user_reads_and_hides_workspace_credentials() {
    let binary = std::env::var_os("A3S_TEST_SRT_BIN")
        .map(PathBuf::from)
        .expect("set A3S_TEST_SRT_BIN");
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    let outside = root.path().join("outside-secret.txt");
    std::fs::create_dir_all(workspace.join(".codex")).unwrap();
    std::fs::create_dir_all(workspace.join("services/api")).unwrap();
    std::fs::write(workspace.join("visible.txt"), "workspace-visible").unwrap();
    std::fs::write(workspace.join(".env"), "WORKSPACE_SECRET=hidden").unwrap();
    std::fs::write(
        workspace.join("services/api/.env"),
        "NESTED_WORKSPACE_SECRET=hidden",
    )
    .unwrap();
    std::fs::write(workspace.join(".codex/auth.json"), "workspace-auth").unwrap();
    std::fs::write(&outside, "outside-hidden").unwrap();
    std::fs::hard_link(&outside, workspace.join("outside-hardlink")).unwrap();
    let sandbox = SrtBashSandbox::new(binary, &workspace).unwrap();

    let visible = sandbox
        .exec_command("cat visible.txt", "/workspace")
        .await
        .unwrap();
    assert_eq!(visible.exit_code, 0, "{}{}", visible.stdout, visible.stderr);
    assert_eq!(visible.stdout, "workspace-visible");

    for command in [
        format!("cat {}", outside.display()),
        "cat .env".to_string(),
        "cat services/api/.env".to_string(),
        "cat .codex/auth.json".to_string(),
        "cat outside-hardlink".to_string(),
    ] {
        let denied = sandbox.exec_command(&command, "/workspace").await.unwrap();
        assert_ne!(
            denied.exit_code, 0,
            "{command} unexpectedly exposed protected data: {}{}",
            denied.stdout, denied.stderr
        );
        assert!(!denied.stdout.contains("hidden"));
        assert!(!denied.stdout.contains("workspace-auth"));
    }

    let home = sandbox
        .exec_command("printf %s \"$HOME\"", "/workspace")
        .await
        .unwrap();
    assert_eq!(home.exit_code, 0, "{}{}", home.stdout, home.stderr);
    assert_ne!(
        Path::new(home.stdout.trim()),
        dirs::home_dir().as_deref().unwrap_or_else(|| Path::new(""))
    );
}

/// Run explicitly with `A3S_TEST_SRT_BIN=/absolute/path/to/srt`.
#[tokio::test]
#[ignore = "requires an installed srt runtime"]
async fn real_srt_keeps_the_local_rust_toolchain_usable_offline() {
    let binary = std::env::var_os("A3S_TEST_SRT_BIN")
        .map(PathBuf::from)
        .expect("set A3S_TEST_SRT_BIN");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"));
    let sandbox = SrtBashSandbox::new(binary, workspace).unwrap();

    let output = sandbox
        .exec_command(
            "cargo metadata --offline --no-deps --format-version 1 >/dev/null",
            "/workspace",
        )
        .await
        .unwrap();
    assert_eq!(
        output.exit_code, 0,
        "sandboxed Cargo metadata failed: {}{}",
        output.stdout, output.stderr
    );
}
