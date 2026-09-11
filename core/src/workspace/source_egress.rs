use std::path::{Component, Path, PathBuf};

/// Return whether a workspace-relative path is excluded from source egress.
///
/// This is shared by manifest admission and the read-time source boundary so
/// a symlink or rename cannot cross into a path category that the manifest
/// would have rejected.
pub(crate) fn path_is_denied(path: &Path) -> bool {
    has_sensitive_component(path) || has_sensitive_file_name(path)
}

/// Personal knowledge vault under `.a3s/kb/` is shared agent/human substrate.
///
/// Control-plane siblings (config, memory, auth, sessions) stay denied; only
/// the vault subtree is admitted for catalog + retrieval.
pub(crate) fn is_personal_kb_path(path: &Path) -> bool {
    personal_kb_rest(path).is_some()
}

fn personal_kb_rest(path: &Path) -> Option<PathBuf> {
    let comps: Vec<_> = path.components().collect();
    for index in 0..comps.len().saturating_sub(1) {
        let (Component::Normal(first), Component::Normal(second)) =
            (comps[index], comps[index + 1])
        else {
            continue;
        };
        if first.to_string_lossy().eq_ignore_ascii_case(".a3s")
            && second.to_string_lossy().eq_ignore_ascii_case("kb")
        {
            return Some(comps[index + 2..].iter().collect());
        }
    }
    None
}

fn has_sensitive_component(path: &Path) -> bool {
    for component in path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return true,
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    if let Some(rest) = personal_kb_rest(path) {
        return has_sensitive_dir_name(&rest);
    }
    has_sensitive_dir_name(path)
}

fn has_sensitive_dir_name(path: &Path) -> bool {
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
            ".a3s/kb/sources/note.md",
            ".a3s/kb/wiki/concept.md",
            "nested/.a3s/kb/note.md",
        ] {
            assert!(
                !path_is_denied(Path::new(path)),
                "{path} should be admitted"
            );
        }
        assert!(
            path_is_denied(Path::new(".a3s/kb/.env")),
            "secret filenames under the personal KB stay denied"
        );
        assert!(is_personal_kb_path(Path::new(".a3s/kb/sources/a.md")));
        assert!(!is_personal_kb_path(Path::new(".a3s/config.acl")));
    }
}
