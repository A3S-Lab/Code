//! Local command sandbox backed by the `srt` process wrapper.
//!
//! The adapter deliberately has no unsandboxed fallback. Hosts decide whether
//! an unavailable sandbox should require approval or fail closed before a tool
//! reaches this implementation.

use super::{
    BashSandbox, SandboxCommandRequest, SandboxExecutionOutput, SandboxOutput,
    PROTECTED_WORKSPACE_DIRECTORIES, PROTECTED_WORKSPACE_FILES,
};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_WORKSPACE_SCAN_ENTRIES: usize = 1_000_000;
const MAX_WORKSPACE_SCAN_DEPTH: usize = 64;
const MAX_WORKSPACE_HARDLINK_DENIES: usize = 4_096;
/// npm package accepted by the verified SRT constructor.
pub const SRT_NPM_PACKAGE_NAME: &str = "@anthropic-ai/sandbox-runtime";
/// Exact SRT version provisioned by the current A3S CLI managed-runtime path.
///
/// Core accepts the tested compatibility range below so a host can roll a
/// compatible patch independently. The CLI deliberately installs one exact
/// version until an A3S-signed component artifact replaces registry bootstrap.
pub const MANAGED_SRT_VERSION: &str = "0.0.66";
const MINIMUM_SRT_VERSION: (u64, u64, u64) = (0, 0, 66);
const MAXIMUM_SRT_VERSION_EXCLUSIVE: (u64, u64, u64) = (0, 1, 0);

/// An `srt`-backed local command sandbox rooted at one workspace.
#[derive(Debug)]
pub struct SrtBashSandbox {
    binary: PathBuf,
    node: Option<PathBuf>,
    shell: PathBuf,
    #[cfg(not(windows))]
    env_binary: PathBuf,
    workspace: PathBuf,
    workspace_hardlink_paths: Vec<PathBuf>,
}

impl SrtBashSandbox {
    /// Build an adapter from an explicit `srt` executable.
    pub fn new(binary: impl Into<PathBuf>, workspace: impl Into<PathBuf>) -> Result<Self> {
        let binary = binary.into();
        if binary.components().count() == 1 {
            bail!("an explicit SRT executable path is required; PATH discovery is unsupported");
        }
        let workspace = workspace
            .into()
            .canonicalize()
            .context("failed to canonicalize the SRT workspace")?;
        if !workspace.is_dir() {
            bail!("SRT workspace is not a directory: {}", workspace.display());
        }
        let binary = resolve_executable(binary, Some(&workspace))?;
        if binary.starts_with(&workspace) {
            bail!(
                "refusing to trust an SRT executable from inside the active workspace: {}",
                binary.display()
            );
        }
        let node = node_script(&binary)
            .then(|| resolve_executable(PathBuf::from("node"), Some(&workspace)))
            .transpose()
            .context("failed to resolve a trusted Node.js launcher for SRT")?;
        Self::from_resolved(binary, node, workspace)
    }

    /// Build an adapter from a compatible npm installation at an exact path.
    ///
    /// Unlike [`Self::new`], this path verifies the package identity and tested
    /// version before accepting its CLI. It is intended for embedding hosts
    /// that provision SRT outside `PATH`.
    pub fn from_verified_npm(
        binary: impl Into<PathBuf>,
        workspace: impl Into<PathBuf>,
    ) -> Result<Self> {
        let binary = binary.into();
        let installation = inspect_srt_installation(&binary)?;
        ensure_supported_srt_version(&installation.version)?;
        Self::new(installation.cli, workspace)
    }

    /// Build from a verified npm installation and an explicitly selected Node
    /// executable.
    ///
    /// Managed hosts use this form so command startup cannot select a different
    /// `node` from a later `PATH` entry after provisioning has completed.
    pub fn from_verified_npm_with_node(
        binary: impl Into<PathBuf>,
        node: impl Into<PathBuf>,
        workspace: impl Into<PathBuf>,
    ) -> Result<Self> {
        let workspace = workspace
            .into()
            .canonicalize()
            .context("failed to canonicalize the SRT workspace")?;
        if !workspace.is_dir() {
            bail!("SRT workspace is not a directory: {}", workspace.display());
        }
        let installation = inspect_srt_installation(&binary.into())?;
        ensure_supported_srt_version(&installation.version)?;
        let binary = resolve_executable(installation.cli, Some(&workspace))?;
        let node = resolve_executable(node.into(), Some(&workspace))
            .context("failed to resolve the managed Node.js launcher for SRT")?;
        Self::from_resolved(binary, Some(node), workspace)
    }

    pub fn binary(&self) -> &Path {
        &self.binary
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    fn settings(&self, scratch: &Path) -> Result<serde_json::Value> {
        let mut deny_write = protected_workspace_paths(&self.workspace);
        if let Some(git_dir) = resolved_git_dir(&self.workspace) {
            deny_write.push(git_dir);
        }
        let mut sensitive_paths = sensitive_paths();
        sensitive_paths.extend(workspace_sensitive_paths(&self.workspace)?);
        sensitive_paths.extend(self.workspace_hardlink_paths.iter().cloned());
        let mut deny_read = sensitive_paths.clone();
        deny_read.extend(read_denied_roots());
        let mut allow_read = readable_tool_paths(&self.workspace, scratch);
        deny_write.extend(sensitive_paths.iter().cloned());
        deduplicate_paths(&mut deny_write);
        deduplicate_paths(&mut sensitive_paths);
        deduplicate_paths(&mut deny_read);
        deduplicate_paths(&mut allow_read);

        Ok(json!({
            "network": {
                "allowedDomains": [],
                "deniedDomains": [],
                "allowUnixSockets": [],
                "allowAllUnixSockets": false,
                "allowLocalBinding": false
            },
            "filesystem": {
                "allowWrite": path_strings([self.workspace.as_path(), scratch]),
                "denyWrite": path_strings(deny_write.iter().map(PathBuf::as_path)),
                "denyRead": path_strings(deny_read.iter().map(PathBuf::as_path)),
                // User and temporary roots are hidden, then the workspace,
                // private scratch directory, and explicit toolchain roots are
                // re-exposed. The tested SRT range preserves more-specific
                // literal credential denies inside an allowed workspace.
                "allowRead": path_strings(allow_read.iter().map(PathBuf::as_path))
            },
            "mandatoryDenySearchDepth": 10,
            "enableWeakerNestedSandbox": false,
            "enableWeakerNetworkIsolation": false,
            "allowAppleEvents": false
        }))
    }

    fn from_resolved(binary: PathBuf, node: Option<PathBuf>, workspace: PathBuf) -> Result<Self> {
        #[cfg(not(windows))]
        let shell = resolve_executable(PathBuf::from("bash"), Some(&workspace))
            .context("failed to resolve a trusted bash executable for SRT")?;
        #[cfg(windows)]
        let shell = resolve_executable(PathBuf::from("powershell.exe"), Some(&workspace))
            .context("failed to resolve a trusted PowerShell executable for SRT")?;
        #[cfg(not(windows))]
        let env_binary = resolve_executable(PathBuf::from("env"), Some(&workspace))
            .context("failed to resolve a trusted env executable for SRT")?;
        let workspace_hardlink_paths = workspace_hardlink_paths(&workspace)?;
        Ok(Self {
            binary,
            node,
            shell,
            #[cfg(not(windows))]
            env_binary,
            workspace,
            workspace_hardlink_paths,
        })
    }

    async fn execute_request(
        &self,
        request: SandboxCommandRequest,
    ) -> Result<SandboxExecutionOutput> {
        let scratch = tempfile::Builder::new()
            .prefix("a3s-code-srt-")
            .tempdir()
            .context("failed to create SRT scratch directory")?;
        let settings_path = scratch.path().join("settings.json");
        let settings = serde_json::to_vec(&self.settings(scratch.path())?)
            .context("failed to serialize SRT settings")?;
        tokio::fs::write(&settings_path, settings)
            .await
            .context("failed to write SRT settings")?;

        let mut command = if let Some(node) = &self.node {
            let mut command = Command::new(node);
            command.arg(&self.binary);
            command
        } else {
            Command::new(&self.binary)
        };
        command
            .arg("--settings")
            .arg(&settings_path)
            // Stop the SRT CLI parser before the wrapped executable's flags.
            // Without this delimiter, flags such as `env -i` or PowerShell's
            // `-NoProfile` are consumed as SRT options before the sandbox
            // command is constructed.
            .arg("--");
        #[cfg(not(windows))]
        {
            command.arg(&self.env_binary).arg("-i");
            for (key, value) in compose_child_env(request.env.as_deref(), scratch.path())? {
                command.arg(environment_assignment(&key, &value));
            }
            command.arg(&self.shell).arg("-c").arg(&request.command);
        }
        #[cfg(windows)]
        {
            let wrapped = crate::tools::builtin::bash::build_powershell_command(&request.command);
            let encoded = crate::tools::builtin::bash::encode_powershell_command(&wrapped);
            command
                .arg(&self.shell)
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-EncodedCommand",
                    &encoded,
                ])
                .creation_flags(crate::tools::builtin::bash::CREATE_NO_WINDOW);
        }
        command
            .current_dir(&self.workspace)
            .env_clear()
            .envs(compose_srt_process_env(
                request.env.as_deref(),
                scratch.path(),
                &self.workspace,
            )?)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        crate::tools::process::configure_process_group(&mut command);

        let mut child = command
            .spawn()
            .with_context(|| format!("failed to start SRT executable {}", self.binary.display()))?;
        let process = crate::tools::process::read_process_output(
            &mut child,
            request.timeout_ms,
            request.output_observer.as_deref(),
        )
        .await
        .context("failed to wait for SRT command")?;
        if process.timed_out {
            return Ok(SandboxExecutionOutput {
                stdout: process.stdout,
                stderr: process.stderr,
                exit_code: -1,
                timed_out: true,
            });
        }

        Ok(SandboxExecutionOutput {
            stdout: process.stdout,
            stderr: process.stderr,
            exit_code: process
                .status
                .and_then(|status| status.code())
                .unwrap_or(-1),
            timed_out: false,
        })
    }
}

#[async_trait]
impl BashSandbox for SrtBashSandbox {
    async fn exec_command(&self, command: &str, guest_workspace: &str) -> Result<SandboxOutput> {
        let output = self
            .execute_request(SandboxCommandRequest {
                command: command.to_string(),
                guest_workspace: guest_workspace.to_string(),
                timeout_ms: DEFAULT_TIMEOUT_MS,
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

    async fn exec(&self, request: SandboxCommandRequest) -> Result<SandboxExecutionOutput> {
        self.execute_request(request).await
    }

    async fn shutdown(&self) {}
}

#[derive(Debug)]
struct SrtInstallation {
    cli: PathBuf,
    version: String,
}

fn inspect_srt_installation(binary: &Path) -> Result<SrtInstallation> {
    let canonical = binary
        .canonicalize()
        .with_context(|| format!("failed to resolve SRT executable {}", binary.display()))?;
    let mut roots = canonical
        .ancestors()
        .take(5)
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    if binary
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name.eq_ignore_ascii_case(".bin"))
    {
        if let Some(node_modules) = binary.parent().and_then(Path::parent) {
            roots.push(node_modules.join("@anthropic-ai").join("sandbox-runtime"));
        }
    }
    deduplicate_paths(&mut roots);

    for root in roots {
        let manifest_path = root.join("package.json");
        let Ok(source) = std::fs::read(&manifest_path) else {
            continue;
        };
        let manifest: serde_json::Value = serde_json::from_slice(&source)
            .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
        if manifest.get("name").and_then(serde_json::Value::as_str) != Some(SRT_NPM_PACKAGE_NAME) {
            continue;
        }
        let version = manifest
            .get("version")
            .and_then(serde_json::Value::as_str)
            .filter(|version| !version.trim().is_empty())
            .ok_or_else(|| anyhow!("SRT package manifest has no version"))?
            .to_string();
        let cli = root
            .join("dist")
            .join("cli.js")
            .canonicalize()
            .context("failed to resolve the SRT package CLI")?;
        if !cli.is_file() {
            bail!("SRT package CLI is not a file: {}", cli.display());
        }
        return Ok(SrtInstallation { cli, version });
    }

    bail!(
        "refusing unverified `srt` from {}: expected package {}",
        binary.display(),
        SRT_NPM_PACKAGE_NAME
    )
}

fn ensure_supported_srt_version(version: &str) -> Result<()> {
    let parsed = parse_semver_triplet(version)
        .ok_or_else(|| anyhow!("unsupported SRT version format: {version}"))?;
    if parsed < MINIMUM_SRT_VERSION || parsed >= MAXIMUM_SRT_VERSION_EXCLUSIVE {
        bail!(
            "unsupported SRT version {version}; expected >= {}.{}.{} and < {}.{}.{}",
            MINIMUM_SRT_VERSION.0,
            MINIMUM_SRT_VERSION.1,
            MINIMUM_SRT_VERSION.2,
            MAXIMUM_SRT_VERSION_EXCLUSIVE.0,
            MAXIMUM_SRT_VERSION_EXCLUSIVE.1,
            MAXIMUM_SRT_VERSION_EXCLUSIVE.2,
        );
    }
    Ok(())
}

fn parse_semver_triplet(version: &str) -> Option<(u64, u64, u64)> {
    let core = version
        .trim()
        .strip_prefix('v')
        .unwrap_or(version.trim())
        .split(['-', '+'])
        .next()?;
    let mut components = core.split('.');
    let parsed = (
        components.next()?.parse().ok()?,
        components.next()?.parse().ok()?,
        components.next()?.parse().ok()?,
    );
    components.next().is_none().then_some(parsed)
}

fn node_script(path: &Path) -> bool {
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("js"))
    {
        return true;
    }
    std::fs::read(path)
        .ok()
        .and_then(|source| source.get(..source.len().min(128)).map(Vec::from))
        .and_then(|prefix| String::from_utf8(prefix).ok())
        .is_some_and(|prefix| {
            prefix
                .lines()
                .next()
                .is_some_and(|line| line.starts_with("#!") && line.contains("node"))
        })
}

fn resolve_executable(binary: PathBuf, excluded_root: Option<&Path>) -> Result<PathBuf> {
    let candidate = if binary.components().count() == 1 {
        find_executable_on_path(&binary, excluded_root).ok_or_else(|| {
            anyhow!(
                "required executable was not found on PATH: {}",
                binary.display()
            )
        })?
    } else {
        binary
    };
    let candidate = candidate
        .canonicalize()
        .with_context(|| format!("failed to resolve executable {}", candidate.display()))?;
    if !candidate.is_file() {
        bail!("executable is not a file: {}", candidate.display());
    }
    if !is_executable(&candidate) {
        bail!("executable is not executable: {}", candidate.display());
    }
    if excluded_root.is_some_and(|root| candidate.starts_with(root)) {
        bail!(
            "refusing executable from inside the active workspace: {}",
            candidate.display()
        );
    }
    Ok(candidate)
}

fn find_executable_on_path(
    binary: impl AsRef<OsStr>,
    excluded_root: Option<&Path>,
) -> Option<PathBuf> {
    let binary = binary.as_ref();
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(binary);
        if executable_is_trusted(&candidate, excluded_root) {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            for extension in executable_extensions() {
                let candidate = directory.join(format!(
                    "{}{}",
                    binary.to_string_lossy(),
                    extension.to_string_lossy()
                ));
                if executable_is_trusted(&candidate, excluded_root) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn executable_is_trusted(candidate: &Path, excluded_root: Option<&Path>) -> bool {
    if !candidate.is_file() || !is_executable(candidate) {
        return false;
    }
    let Ok(canonical) = candidate.canonicalize() else {
        return false;
    };
    !excluded_root.is_some_and(|root| canonical.starts_with(root))
}

#[cfg(windows)]
fn executable_extensions() -> Vec<OsString> {
    std::env::var_os("PATHEXT")
        .map(|value| {
            value
                .to_string_lossy()
                .split(';')
                .filter(|value| !value.is_empty())
                .map(OsString::from)
                .collect()
        })
        .unwrap_or_else(|| {
            [".COM", ".EXE", ".BAT", ".CMD"]
                .into_iter()
                .map(OsString::from)
                .collect()
        })
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn compose_child_env(
    explicit: Option<&HashMap<String, String>>,
    scratch: &Path,
) -> Result<HashMap<OsString, OsString>> {
    const SAFE_KEYS: &[&str] = &[
        "PATH",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "TERM",
        "COLORTERM",
        "NO_COLOR",
        "CI",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "RUSTC_WRAPPER",
        "GOPATH",
        "GOROOT",
        "GOMODCACHE",
        "NVM_DIR",
        "FNM_DIR",
        "VOLTA_HOME",
        "BUN_INSTALL",
        "DENO_DIR",
        "PNPM_HOME",
        "JAVA_HOME",
        "GRADLE_USER_HOME",
        "MAVEN_HOME",
        "SDKROOT",
        "DEVELOPER_DIR",
        "PKG_CONFIG_PATH",
        "LIBRARY_PATH",
        "CPATH",
        "CC",
        "CXX",
        "AR",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
    ];

    let mut environment = HashMap::new();
    for key in SAFE_KEYS {
        if let Some(value) = std::env::var_os(key) {
            environment.insert(OsString::from(key), value);
        }
    }
    for (key, value) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("LC_") {
            environment.insert(key, value);
        }
    }
    if let Some(explicit) = explicit {
        for (key, value) in explicit {
            if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
                bail!("invalid explicit command environment entry: {key:?}");
            }
            environment.insert(OsString::from(key), OsString::from(value));
        }
    }
    // Explicit command variables may add ordinary data, but they must not
    // re-enable shell, language-runtime, or dynamic-loader bootstrap hooks.
    // Apply this after explicit values so callers cannot override the denylist.
    remove_bootstrap_injection_variables(&mut environment);

    let scratch = scratch.as_os_str().to_os_string();
    environment.insert(OsString::from("HOME"), scratch.clone());
    environment.insert(OsString::from("TMPDIR"), scratch.clone());
    environment.insert(OsString::from("TMP"), scratch.clone());
    environment.insert(OsString::from("TEMP"), scratch.clone());
    environment.insert(OsString::from("XDG_CACHE_HOME"), scratch.clone());
    environment.insert(OsString::from("XDG_CONFIG_HOME"), scratch.clone());
    environment.insert(OsString::from("XDG_DATA_HOME"), scratch.clone());
    environment.insert(OsString::from("XDG_STATE_HOME"), scratch);
    Ok(environment)
}

fn compose_srt_process_env(
    explicit: Option<&HashMap<String, String>>,
    scratch: &Path,
    workspace: &Path,
) -> Result<HashMap<OsString, OsString>> {
    #[cfg(not(windows))]
    {
        let _ = explicit;
        let _ = scratch;
        Ok(compose_wrapper_env(workspace))
    }
    #[cfg(windows)]
    {
        let mut environment = compose_child_env(explicit, scratch)?;
        remove_bootstrap_injection_variables(&mut environment);
        if let Some(path) = trusted_wrapper_path(workspace) {
            environment.insert(OsString::from("PATH"), path);
        } else {
            environment.remove(OsStr::new("PATH"));
        }
        Ok(environment)
    }
}

fn compose_wrapper_env(workspace: &Path) -> HashMap<OsString, OsString> {
    const SAFE_KEYS: &[&str] = &[
        "HOME",
        "USER",
        "LOGNAME",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
    ];
    let mut environment = HashMap::new();
    for key in SAFE_KEYS {
        if let Some(value) = std::env::var_os(key) {
            environment.insert(OsString::from(key), value);
        }
    }
    for (key, value) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("LC_") {
            environment.insert(key, value);
        }
    }
    if let Some(path) = trusted_wrapper_path(workspace) {
        environment.insert(OsString::from("PATH"), path);
    }
    remove_bootstrap_injection_variables(&mut environment);
    environment
}

fn trusted_wrapper_path(workspace: &Path) -> Option<OsString> {
    let path = std::env::var_os("PATH")?;
    let directories = std::env::split_paths(&path)
        .filter_map(|directory| {
            let absolute = if directory.is_absolute() {
                directory
            } else {
                std::env::current_dir().ok()?.join(directory)
            };
            let canonical = absolute.canonicalize().ok()?;
            (canonical.is_dir() && !canonical.starts_with(workspace)).then_some(canonical)
        })
        .collect::<Vec<_>>();
    std::env::join_paths(directories).ok()
}

fn remove_bootstrap_injection_variables(environment: &mut HashMap<OsString, OsString>) {
    const BLOCKED: &[&str] = &[
        "BASH_ENV",
        "ENV",
        "NODE_OPTIONS",
        "NODE_PATH",
        "PYTHONHOME",
        "PYTHONPATH",
        "PYTHONSTARTUP",
        "PYTHONINSPECT",
        "RUBYOPT",
        "RUBYLIB",
        "PERL5OPT",
        "PERL5LIB",
        "LUA_INIT",
        "JAVA_TOOL_OPTIONS",
        "JDK_JAVA_OPTIONS",
        "_JAVA_OPTIONS",
        "LD_PRELOAD",
        "LD_LIBRARY_PATH",
        "DYLD_INSERT_LIBRARIES",
        "DYLD_LIBRARY_PATH",
    ];
    environment.retain(|key, _| {
        let key = key.to_string_lossy();
        !BLOCKED
            .iter()
            .any(|blocked| key.eq_ignore_ascii_case(blocked))
            && !key.to_ascii_uppercase().starts_with("LUA_INIT_")
    });
}

fn environment_assignment(key: &OsStr, value: &OsStr) -> OsString {
    let mut assignment = key.to_os_string();
    assignment.push("=");
    assignment.push(value);
    assignment
}

pub(crate) fn sensitive_paths() -> Vec<PathBuf> {
    let mut paths = dirs::home_dir()
        .map(|home| default_sensitive_paths(&home))
        .unwrap_or_default();

    extend_configured_secret(&mut paths, "CODEX_HOME", Some("auth.json"));
    extend_configured_secret(&mut paths, "CLAUDE_CONFIG_DIR", Some(".credentials.json"));
    extend_configured_secret(&mut paths, "CARGO_HOME", Some("credentials"));
    extend_configured_secret(&mut paths, "CARGO_HOME", Some("credentials.toml"));
    for variable in ["A3S_KIMI_HOME", "KIMI_CODE_HOME", "KIMI_SHARE_DIR"] {
        extend_configured_secret(&mut paths, variable, Some("credentials/kimi-code.json"));
    }
    for variable in [
        "A3S_KIMI_DESKTOP_HOME",
        "KIMI_DESKTOP_HOME",
        "WORKBUDDY_CONFIG_DIR",
        "CODEBUDDY_CONFIG_DIR",
    ] {
        extend_configured_secret(&mut paths, variable, None);
    }
    paths
}

fn read_denied_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        // The Windows provider runs commands under its dedicated sandbox user.
        // Broad parent-directory DENY ACEs would override the narrower
        // workspace grant, so rely on that account boundary plus the explicit
        // credential denies above.
        Vec::new()
    }
    #[cfg(not(windows))]
    {
        let mut roots = Vec::new();
        if let Some(home) = dirs::home_dir() {
            roots.push(home);
        }
        let temp = std::env::temp_dir();
        roots.push(temp.canonicalize().unwrap_or(temp));
        roots
    }
}

fn readable_tool_paths(workspace: &Path, scratch: &Path) -> Vec<PathBuf> {
    const TOOLCHAIN_ROOTS: &[&str] = &[
        "CARGO_HOME",
        "RUSTUP_HOME",
        "GOPATH",
        "GOROOT",
        "GOMODCACHE",
        "NVM_DIR",
        "FNM_DIR",
        "VOLTA_HOME",
        "BUN_INSTALL",
        "DENO_DIR",
        "PNPM_HOME",
        "JAVA_HOME",
        "GRADLE_USER_HOME",
        "MAVEN_HOME",
        "SDKROOT",
        "DEVELOPER_DIR",
    ];

    let mut paths = vec![workspace.to_path_buf(), scratch.to_path_buf()];
    for variable in TOOLCHAIN_ROOTS {
        let Some(path) = std::env::var_os(variable).filter(|value| !value.is_empty()) else {
            continue;
        };
        let path = PathBuf::from(path);
        if path.is_absolute() && path.exists() {
            paths.push(path.canonicalize().unwrap_or(path));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path).filter_map(|path| {
            if !path.is_absolute() || !path.exists() {
                return None;
            }
            path.canonicalize().ok()
        }));
    }
    paths
}

fn default_sensitive_paths(home: &Path) -> Vec<PathBuf> {
    [
        ".ssh",
        ".gnupg",
        ".aws",
        ".azure",
        ".kube",
        ".docker",
        ".config/gcloud",
        ".config/gh",
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".cargo/credentials",
        ".cargo/credentials.toml",
        ".codex/auth.json",
        ".claude/.credentials.json",
        ".claude.json",
        ".git-credentials",
        ".config/git/credentials",
        ".workbuddy",
        "credentials/kimi-code.json",
        ".kimi-code/credentials/kimi-code.json",
        ".kimi/credentials/kimi-code.json",
        ".config/kimi-desktop/daimon-share",
        "Library/Application Support/kimi-desktop/daimon-share",
        ".config/opencode/auth.json",
        ".local/share/opencode/auth.json",
        ".gemini/oauth_creds.json",
        ".terraform.d/credentials.tfrc.json",
        ".local/share/keyrings",
        ".password-store",
        ".a3s/os-auth.json",
        "Library/Keychains",
    ]
    .into_iter()
    .map(|path| home.join(path))
    .collect()
}

pub(crate) fn workspace_sensitive_paths(workspace: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = [
        ".env",
        ".env.local",
        ".env.development",
        ".env.production",
        ".env.test",
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".git-credentials",
        ".a3s/os-auth.json",
        ".codex/auth.json",
        ".claude/.credentials.json",
        ".claude.json",
    ]
    .into_iter()
    .map(|path| workspace.join(path))
    .collect::<Vec<_>>();
    paths.extend(workspace_nested_env_paths(workspace)?);
    Ok(paths)
}

fn workspace_nested_env_paths(workspace: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![(workspace.to_path_buf(), 0usize)];
    let mut scanned = 0usize;
    let mut paths = Vec::new();

    while let Some((directory, depth)) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .with_context(|| format!("failed to scan SRT workspace {}", directory.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| {
                format!("failed to enumerate SRT workspace {}", directory.display())
            })?;
            scanned = next_workspace_scan_entry(scanned)?;
            let path = entry.path();
            let file_type = entry.file_type().with_context(|| {
                format!("failed to inspect SRT workspace path {}", path.display())
            })?;
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".env"))
            {
                paths.push(path);
            } else if file_type.is_dir() {
                if should_skip_workspace_scan_directory(&entry.file_name()) {
                    continue;
                }
                ensure_workspace_scan_depth(depth, &path)?;
                pending.push((path, depth + 1));
            }
        }
    }

    Ok(paths)
}

pub(crate) fn workspace_hardlink_paths(workspace: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![(workspace.to_path_buf(), 0usize)];
    let mut scanned = 0usize;
    let mut hardlinks = Vec::new();

    while let Some((directory, depth)) = pending.pop() {
        let entries = std::fs::read_dir(&directory)
            .with_context(|| format!("failed to scan SRT workspace {}", directory.display()))?;
        for entry in entries {
            let entry = entry.with_context(|| {
                format!("failed to enumerate SRT workspace {}", directory.display())
            })?;
            scanned = next_workspace_scan_entry(scanned)?;

            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).with_context(|| {
                format!("failed to inspect SRT workspace path {}", path.display())
            })?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if should_skip_workspace_scan_directory(&entry.file_name()) {
                    continue;
                }
                ensure_workspace_scan_depth(depth, &path)?;
                pending.push((path, depth + 1));
                continue;
            }
            if metadata.is_file() && hard_link_count(&metadata) > 1 {
                hardlinks.push(path);
                if hardlinks.len() > MAX_WORKSPACE_HARDLINK_DENIES {
                    bail!(
                        "SRT workspace contains more than {MAX_WORKSPACE_HARDLINK_DENIES} multi-link files"
                    );
                }
            }
        }
    }

    hardlinks.sort();
    hardlinks.dedup();
    Ok(hardlinks)
}

fn next_workspace_scan_entry(scanned: usize) -> Result<usize> {
    let scanned = scanned
        .checked_add(1)
        .context("SRT workspace scan entry count overflowed")?;
    if scanned > MAX_WORKSPACE_SCAN_ENTRIES {
        bail!("SRT workspace exceeds the {MAX_WORKSPACE_SCAN_ENTRIES} entry scan limit");
    }
    Ok(scanned)
}

fn ensure_workspace_scan_depth(depth: usize, path: &Path) -> Result<()> {
    if depth >= MAX_WORKSPACE_SCAN_DEPTH {
        bail!(
            "SRT workspace exceeds the {MAX_WORKSPACE_SCAN_DEPTH}-level scan depth at {}",
            path.display()
        );
    }
    Ok(())
}

pub(crate) fn should_skip_workspace_scan_directory(name: &OsStr) -> bool {
    // Control metadata is already write-protected, while dependency and build
    // trees commonly contain legitimate package-store hardlinks and can be
    // extremely large. The local boundary governs source-tree work; hostile
    // dependency/build execution belongs in the stronger Box/Runtime boundary.
    matches!(name.to_str(), Some(".git" | "node_modules" | "target"))
}

#[cfg(unix)]
pub(crate) fn hard_link_count(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.nlink()
}

#[cfg(windows)]
pub(crate) fn hard_link_count(metadata: &std::fs::Metadata) -> u64 {
    use std::os::windows::fs::MetadataExt;
    u64::from(metadata.number_of_links().unwrap_or(1))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn hard_link_count(_metadata: &std::fs::Metadata) -> u64 {
    1
}

fn extend_configured_secret(paths: &mut Vec<PathBuf>, variable: &str, suffix: Option<&str>) {
    let Some(root) = std::env::var_os(variable).filter(|value| !value.is_empty()) else {
        return;
    };
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        return;
    }
    paths.push(match suffix {
        Some(suffix) => root.join(suffix),
        None => root,
    });
}

fn protected_workspace_paths(workspace: &Path) -> Vec<PathBuf> {
    PROTECTED_WORKSPACE_DIRECTORIES
        .iter()
        .chain(PROTECTED_WORKSPACE_FILES)
        .copied()
        .map(|path| workspace.join(path))
        .collect()
}

fn resolved_git_dir(workspace: &Path) -> Option<PathBuf> {
    let dot_git = workspace.join(".git");
    if dot_git.is_dir() {
        return dot_git.canonicalize().ok();
    }
    let source = std::fs::read_to_string(dot_git).ok()?;
    let relative = source.trim().strip_prefix("gitdir:")?.trim();
    let path = Path::new(relative);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    };
    path.canonicalize().ok()
}

fn deduplicate_paths(paths: &mut Vec<PathBuf>) {
    paths.sort();
    paths.dedup();
}

fn path_strings<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<String> {
    paths
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests;
