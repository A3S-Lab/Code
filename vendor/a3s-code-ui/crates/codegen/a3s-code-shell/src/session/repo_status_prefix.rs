use std::path::PathBuf;

/// Discover the VCS root for templated prefixes (`vcs_root` / "Is directory a git repo").
pub(crate) fn discover_vcs_root(cwd: &std::path::Path) -> Option<PathBuf> {
    use a3s_code_workspace::session::git::{GitDiscoveryResult, discover_git_root};
    match discover_git_root(cwd) {
        GitDiscoveryResult::Found(r) => {
            Some(PathBuf::from(r.to_string_lossy().trim_end_matches('/')))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_vcs_root_none_outside_a_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(discover_vcs_root(tmp.path()), None);
    }
}
