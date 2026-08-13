use crate::workspace::LocalWorkspaceFile;
use std::path::{Component, Path};

const DEFAULT_MAX_FILE_BYTES: u64 = 512 * 1024;

/// Conservative sensitive-path rules for files retained in the local catalog.
///
/// Remote embedding egress uses a separate, explicit admission policy; this
/// local policy keeps useful text formats while excluding known secret and
/// generated locations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceEligibilityPolicy {
    pub max_file_bytes: u64,
}

impl Default for WorkspaceEligibilityPolicy {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

impl WorkspaceEligibilityPolicy {
    pub fn admits(&self, file: &LocalWorkspaceFile) -> bool {
        if file.binary || file.generated || file.size > self.max_file_bytes {
            return false;
        }
        let path = Path::new(&file.path);
        !has_sensitive_component(path) && !has_sensitive_file_name(path)
    }
}

fn has_sensitive_component(path: &Path) -> bool {
    path.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        matches!(
            name.to_string_lossy().to_ascii_lowercase().as_str(),
            ".git"
                | ".a3s"
                | ".ssh"
                | ".aws"
                | ".azure"
                | ".claude"
                | ".codex"
                | ".docker"
                | ".gnupg"
                | ".kube"
                | "node_modules"
                | "target"
                | ".next"
                | "dist"
                | "build"
                | "coverage"
        )
    })
}

fn has_sensitive_file_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    matches!(
        extension.as_str(),
        "pem" | "key" | "ppk" | "p12" | "pfx" | "jks" | "keystore"
    ) || matches!(
        name.as_str(),
        ".env"
            | "credentials"
            | "credentials.json"
            | ".netrc"
            | ".npmrc"
            | ".pypirc"
            | "auth.json"
            | "service-account.json"
            | "service_account.json"
            | "secrets.json"
            | "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
    ) || ((name.starts_with(".env.")
        || name.starts_with("credentials.")
        || name.starts_with("secrets."))
        && !name.ends_with(".example")
        && !name.ends_with(".sample"))
}
