//! Admission of an automatically discovered (untrusted) workspace layer.
//!
//! A repository's `.a3s/config.acl` may shape the agent, but it must not
//! start a host process or read host files merely because the user opened
//! the repository (SEC-WSTRUST1). Two rules apply on top of the
//! host-authoritative withhold:
//!
//! - each `mcp_servers` declaration from an untrusted layer stays inert
//!   until the user trusts its exact semantic digest. The digest binds the
//!   repository root and the whole declaration, so an edited command, an
//!   added argument or environment entry, or the same file in another
//!   repository is a new pending declaration;
//! - untrusted `skill_dirs` / `agent_dirs` entries must stay inside the
//!   repository root; an entry that escapes it is dropped.
//!
//! Trust lives in the user state root next to the command-hook trust
//! store, never inside the repository.

use std::collections::BTreeSet;
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{withhold_untrusted_host_authoritative_blocks, CodeConfig, LayerAcl};
use crate::mcp::{McpServerConfig, McpTransportConfig};

/// File name of the workspace MCP trust store inside `<state root>/code/`.
pub const WORKSPACE_MCP_TRUST_FILE: &str = "workspace-mcp-trust.json";
const MAX_TRUST_BYTES: u64 = 1024 * 1024;
const TRUST_STORE_VERSION: u32 = 1;

/// `<state root>/code/workspace-mcp-trust.json`. The state root must match
/// the CLI `ComponentPaths` state root: `A3S_STATE_HOME`, else the platform
/// state directory.
pub fn workspace_mcp_trust_path(directory: &Path) -> anyhow::Result<PathBuf> {
    Ok(code_state_dir(directory)?.join(WORKSPACE_MCP_TRUST_FILE))
}

/// `<state root>/code`: user-owned Code state that repository content must
/// never be able to write or pre-seed.
pub fn code_state_dir(directory: &Path) -> anyhow::Result<PathBuf> {
    let root = match nonempty_env("A3S_STATE_HOME") {
        Some(value) if value.is_absolute() => value,
        Some(value) => directory.join(value),
        None => platform_state_root(directory)?,
    };
    Ok(root.join("code"))
}

#[cfg(target_os = "macos")]
fn platform_state_root(_directory: &Path) -> anyhow::Result<PathBuf> {
    Ok(home()?.join("Library/Application Support/A3S/State"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_state_root(directory: &Path) -> anyhow::Result<PathBuf> {
    match nonempty_env("XDG_STATE_HOME") {
        Some(value) if value.is_absolute() => Ok(value.join("a3s")),
        Some(value) => Ok(directory.join(value).join("a3s")),
        None => Ok(home()?.join(".local/state/a3s")),
    }
}

#[cfg(windows)]
fn platform_state_root(_directory: &Path) -> anyhow::Result<PathBuf> {
    nonempty_env("LOCALAPPDATA")
        .map(|local| local.join("A3S/State"))
        .context("A3S_STATE_HOME is not set and LOCALAPPDATA is not available")
}

#[cfg(unix)]
fn home() -> anyhow::Result<PathBuf> {
    nonempty_env("HOME")
        .or_else(dirs::home_dir)
        .context("A3S_STATE_HOME is not set and no home directory is available")
}

fn nonempty_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The exact semantic identity of one repository MCP declaration.
pub fn workspace_mcp_server_id(
    repository_root: &Path,
    server: &McpServerConfig,
) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"a3s.code.workspace-mcp-trust.v1\0");
    hasher.update(repository_root.to_string_lossy().as_bytes());
    hasher.update(b"\0");
    hasher.update(serde_json::to_vec(&canonical_declaration(server)?)?);
    Ok(hex(&hasher.finalize()))
}

/// Serde over `McpServerConfig` iterates its `HashMap`s in random order;
/// the identity must not depend on that.
fn canonical_declaration(server: &McpServerConfig) -> anyhow::Result<serde_json::Value> {
    let mut value = serde_json::to_value(server)?;
    sort_objects(&mut value);
    Ok(value)
}

fn sort_objects(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = std::mem::take(map).into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (key, mut child) in entries {
                sort_objects(&mut child);
                map.insert(key, child);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(sort_objects),
        _ => {}
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// One repository MCP declaration and whether the user trusted it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceMcpDeclaration {
    pub id: String,
    pub name: String,
    /// What the declaration would start or contact, for the trust prompt.
    pub summary: String,
    pub trusted: bool,
}

impl WorkspaceMcpDeclaration {
    pub fn short_id(&self) -> &str {
        self.id.get(..12).unwrap_or(&self.id)
    }
}

fn summarize(server: &McpServerConfig) -> String {
    match &server.transport {
        McpTransportConfig::Stdio { command, args } => {
            let mut words = vec![command.clone()];
            words.extend(args.iter().cloned());
            shell_words::join(words)
        }
        McpTransportConfig::Http { url, .. } => format!("http {url}"),
        McpTransportConfig::StreamableHttp { url, .. } => format!("streamable-http {url}"),
    }
}

/// What admitting the untrusted layers withheld or left pending.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceLayerAdmission {
    /// Host-authoritative blocks an untrusted layer tried to set.
    pub withheld_blocks: Vec<&'static str>,
    /// Every untrusted MCP declaration with its trust state.
    pub workspace_mcp: Vec<WorkspaceMcpDeclaration>,
    /// Untrusted skill/agent directories that escaped the repository root.
    pub dropped_dirs: Vec<PathBuf>,
}

impl WorkspaceLayerAdmission {
    pub fn pending_mcp(&self) -> impl Iterator<Item = &WorkspaceMcpDeclaration> {
        self.workspace_mcp.iter().filter(|server| !server.trusted)
    }
}

/// The set of trusted repository MCP declaration ids.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMcpTrust {
    version: u32,
    trusted: BTreeSet<String>,
}

impl WorkspaceMcpTrust {
    /// Load the store; a missing file trusts nothing. A symlinked,
    /// oversized, or malformed store is an error, never an empty grant.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            bail!("workspace MCP trust store must be a regular non-symlink file");
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_TRUST_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_TRUST_BYTES {
            bail!("workspace MCP trust store exceeds {MAX_TRUST_BYTES} bytes");
        }
        let store: Self =
            serde_json::from_slice(&bytes).context("invalid workspace MCP trust store")?;
        if store.version != TRUST_STORE_VERSION {
            bail!(
                "workspace MCP trust store version {} is not {TRUST_STORE_VERSION}",
                store.version
            );
        }
        Ok(store)
    }

    pub fn is_trusted(&self, id: &str) -> bool {
        self.trusted.contains(id)
    }

    /// Add `ids` under an exclusive file lock, re-reading first so
    /// concurrent decisions do not overwrite one another.
    pub fn trust(path: &Path, ids: &[String]) -> anyhow::Result<Self> {
        let parent = path
            .parent()
            .context("workspace MCP trust path has no parent")?;
        std::fs::create_dir_all(parent)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(parent.join("workspace-mcp-trust.lock"))?;
        lock.lock_exclusive()?;
        let result = (|| {
            let mut store = Self::load(path)?;
            store.version = TRUST_STORE_VERSION;
            store.trusted.extend(ids.iter().cloned());
            write_atomic(path, &serde_json::to_vec_pretty(&store)?)?;
            Ok(store)
        })();
        let _ = FileExt::unlock(&lock);
        result
    }
}

fn write_atomic(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    let parent = path.parent().context("trust path has no parent")?;
    let temp = parent.join(format!(
        ".{WORKSPACE_MCP_TRUST_FILE}.{}.tmp",
        std::process::id()
    ));
    let written = (|| {
        #[cfg(unix)]
        let mut file = {
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&temp)?
        };
        #[cfg(not(unix))]
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Apply every untrusted-layer rule to a merged `config`.
///
/// `repository_root` is the directory that owns the untrusted layer's
/// `.a3s/`. Trusted layers keep full authority. Fail-closed: an untrusted
/// layer that does not parse on its own contributes no MCP server and no
/// directory.
pub fn admit_untrusted_layers(
    config: &mut CodeConfig,
    layers: &[LayerAcl],
    repository_root: &Path,
    trust: &WorkspaceMcpTrust,
) -> anyhow::Result<WorkspaceLayerAdmission> {
    let mut admission = WorkspaceLayerAdmission {
        withheld_blocks: withhold_untrusted_host_authoritative_blocks(config, layers),
        ..Default::default()
    };
    if layers.iter().all(|layer| layer.trusted) {
        return Ok(admission);
    }
    let root = repository_root
        .canonicalize()
        .unwrap_or_else(|_| repository_root.to_path_buf());

    let trusted_acls: Vec<&str> = layers
        .iter()
        .filter(|layer| layer.trusted)
        .map(|layer| layer.acl.as_str())
        .collect();
    let trusted = if trusted_acls.is_empty() {
        CodeConfig::default()
    } else {
        CodeConfig::from_acl_layers(&trusted_acls).unwrap_or_default()
    };
    let mut mcp_servers = trusted.mcp_servers;
    let mut skill_dirs = trusted.skill_dirs;
    let mut agent_dirs = trusted.agent_dirs;

    for layer in layers.iter().filter(|layer| !layer.trusted) {
        let Ok(parsed) = CodeConfig::from_acl(&layer.acl) else {
            continue;
        };
        for server in parsed.mcp_servers {
            let id = workspace_mcp_server_id(&root, &server)?;
            let trusted = trust.is_trusted(&id);
            admission.workspace_mcp.push(WorkspaceMcpDeclaration {
                id,
                name: server.name.clone(),
                summary: summarize(&server),
                trusted,
            });
            if trusted && !mcp_servers.iter().any(|known| known.name == server.name) {
                mcp_servers.push(server);
            }
        }
        for (target, entries) in [
            (&mut skill_dirs, parsed.skill_dirs),
            (&mut agent_dirs, parsed.agent_dirs),
        ] {
            for entry in entries {
                if !inside_root(&root, &entry) {
                    admission.dropped_dirs.push(entry);
                } else if !target.contains(&entry) {
                    target.push(entry);
                }
            }
        }
    }

    config.mcp_servers = mcp_servers;
    config.skill_dirs = skill_dirs;
    config.agent_dirs = agent_dirs;
    Ok(admission)
}

/// Whether a repository-declared directory stays under `root`. Relative
/// entries resolve against the root; `..` or a symlink that leaves the
/// root disqualifies the entry.
fn inside_root(root: &Path, entry: &Path) -> bool {
    let candidate = if entry.is_absolute() {
        entry.to_path_buf()
    } else {
        root.join(entry)
    };
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return false;
    }
    match candidate.canonicalize() {
        Ok(resolved) => resolved.starts_with(root),
        Err(_) => candidate.starts_with(root),
    }
}

#[cfg(test)]
mod tests;
