use std::path::{Component, Path};

/// Return whether a workspace-relative path is excluded from source egress.
///
/// This is shared by manifest admission and the read-time source boundary so
/// a symlink or rename cannot cross into a path category that the manifest
/// would have rejected.
pub(crate) fn path_is_denied(path: &Path) -> bool {
    has_sensitive_component(path) || has_sensitive_file_name(path)
}

fn has_sensitive_component(path: &Path) -> bool {
    for component in path.components() {
        let name = match component {
            Component::CurDir => continue,
            Component::Normal(name) => name,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return true,
        };
        if matches!(
            name.to_string_lossy().to_ascii_lowercase().as_str(),
            ".git"
                | ".a3s"
                | ".a3s-code"
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
        ) {
            return true;
        }
    }
    false
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
            | ".git-credentials"
            | "auth.json"
            | "service-account.json"
            | "service_account.json"
            | "secrets.json"
            | "id_rsa"
            | "id_dsa"
            | "id_ecdsa"
            | "id_ed25519"
    ) || ((name.starts_with(".env")
        || name.starts_with("credentials.")
        || name.starts_with("secrets."))
        && !name.ends_with(".example")
        && !name.ends_with(".sample"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_egress_paths_cover_control_credentials_and_generated_trees() {
        for path in [
            ".env",
            "apps/api/.env.local",
            "apps/api/.envrc",
            ".git-credentials",
            ".a3s/config.acl",
            ".a3s-code/index/CURRENT",
            ".git/config",
            "target/generated.rs",
            "node_modules/pkg/index.js",
            "keys/service.pem",
            "nested/secrets.json",
            "../outside.rs",
            "/absolute.rs",
        ] {
            assert!(path_is_denied(Path::new(path)), "{path} should be denied");
        }
        for path in [
            "src/env.rs",
            "src/config.rs",
            ".env.example",
            "fixtures/credentials.sample",
        ] {
            assert!(
                !path_is_denied(Path::new(path)),
                "{path} should be admitted"
            );
        }
    }
}
