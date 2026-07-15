//! Project layout derived from an immutable workspace manifest snapshot.

use crate::workspace::{LocalWorkspaceFile, LocalWorkspaceManifestSnapshot, WorkspacePath};
use std::collections::BTreeMap;
use std::path::Path;

const LAYOUT_HASH_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const LAYOUT_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Language runtime profile selected by a project marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum ProjectLanguageProfile {
    Rust,
    TypeScriptJavaScript,
}

/// Supported project marker kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum ProjectMarkerKind {
    CargoManifest,
    PackageManifest,
    TypeScriptConfig,
}

/// One project marker found in the workspace manifest.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct ProjectMarker {
    pub(crate) path: WorkspacePath,
    pub(crate) root: WorkspacePath,
    pub(crate) profile: ProjectLanguageProfile,
    pub(crate) kind: ProjectMarkerKind,
}

/// Stable project topology for one workspace manifest revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectLayout {
    pub(crate) markers: Vec<ProjectMarker>,
    pub(crate) workspace_revision: u64,
    pub(crate) layout_hash: u64,
}

/// Resolves project topology without filesystem access.
pub(crate) struct ProjectLayoutResolver;

impl ProjectLayoutResolver {
    /// Resolve supported project markers from an immutable manifest snapshot.
    ///
    /// Only marker topology affects `layout_hash`; file metadata, source-file
    /// changes, and the manifest revision do not restart an unchanged layout.
    pub(crate) fn resolve(snapshot: &LocalWorkspaceManifestSnapshot) -> ProjectLayout {
        let mut markers = BTreeMap::<String, ProjectMarker>::new();

        for file in &snapshot.files {
            let Some(marker) = marker_from_file(file) else {
                continue;
            };

            markers
                .entry(marker.path.as_str().to_string())
                .or_insert(marker);
        }

        let mut hasher = StableLayoutHasher::new();
        let markers = markers
            .into_values()
            .inspect(|marker| hash_marker(marker, &mut hasher))
            .collect();

        ProjectLayout {
            markers,
            workspace_revision: snapshot.version,
            layout_hash: hasher.finish(),
        }
    }
}

fn marker_from_file(file: &LocalWorkspaceFile) -> Option<ProjectMarker> {
    if file.binary || file.generated {
        return None;
    }

    let path = Path::new(&file.path);
    let file_name = path.file_name()?.to_str()?;
    let (profile, kind) = marker_classification(file_name)?;
    let root = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| WorkspacePath::from_normalized(parent.to_string_lossy().into_owned()))
        .unwrap_or_else(WorkspacePath::root);

    Some(ProjectMarker {
        path: WorkspacePath::from_normalized(file.path.clone()),
        root,
        profile,
        kind,
    })
}

fn hash_marker(marker: &ProjectMarker, hasher: &mut StableLayoutHasher) {
    hasher.write_str(marker.path.as_str());
    hasher.write_str(marker.root.as_str());
    hasher.write_u8(match marker.profile {
        ProjectLanguageProfile::Rust => 1,
        ProjectLanguageProfile::TypeScriptJavaScript => 2,
    });
    hasher.write_u8(match marker.kind {
        ProjectMarkerKind::CargoManifest => 1,
        ProjectMarkerKind::PackageManifest => 2,
        ProjectMarkerKind::TypeScriptConfig => 3,
    });
}

fn marker_classification(file_name: &str) -> Option<(ProjectLanguageProfile, ProjectMarkerKind)> {
    match file_name {
        "Cargo.toml" => Some((
            ProjectLanguageProfile::Rust,
            ProjectMarkerKind::CargoManifest,
        )),
        "package.json" => Some((
            ProjectLanguageProfile::TypeScriptJavaScript,
            ProjectMarkerKind::PackageManifest,
        )),
        name if name.starts_with("tsconfig") && name.ends_with(".json") => Some((
            ProjectLanguageProfile::TypeScriptJavaScript,
            ProjectMarkerKind::TypeScriptConfig,
        )),
        _ => None,
    }
}

struct StableLayoutHasher(u64);

impl StableLayoutHasher {
    fn new() -> Self {
        let mut hasher = Self(LAYOUT_HASH_OFFSET_BASIS);
        hasher.write_str("a3s-code-project-layout-v1");
        hasher
    }

    fn write_str(&mut self, value: &str) {
        self.write_u64(value.len() as u64);
        self.write_bytes(value.as_bytes());
    }

    fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_le_bytes());
    }

    fn write_u8(&mut self, value: u8) {
        self.write_bytes(&[value]);
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(LAYOUT_HASH_PRIME);
        }
    }

    fn finish(self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ProjectLanguageProfile, ProjectLayoutResolver, ProjectMarkerKind, LAYOUT_HASH_OFFSET_BASIS,
    };
    use crate::workspace::{
        LocalWorkspaceFile, LocalWorkspaceFileStatus, LocalWorkspaceManifestSnapshot,
    };
    use std::path::PathBuf;

    fn workspace_file(path: &str, size: u64, modified_ms: u64) -> LocalWorkspaceFile {
        LocalWorkspaceFile {
            path: path.to_string(),
            size,
            modified_ms: Some(modified_ms),
            language: None,
            status: LocalWorkspaceFileStatus::Tracked,
            binary: false,
            generated: false,
        }
    }

    fn snapshot(version: u64, files: Vec<LocalWorkspaceFile>) -> LocalWorkspaceManifestSnapshot {
        LocalWorkspaceManifestSnapshot {
            version,
            root: PathBuf::from("/workspace"),
            files,
            scanned_at_ms: 1_000,
        }
    }

    #[test]
    fn resolves_mixed_nested_monorepo_with_marker_roots() {
        let layout = ProjectLayoutResolver::resolve(&snapshot(
            17,
            vec![
                workspace_file("crates/runtime/Cargo.toml", 220, 30),
                workspace_file("apps/web/tsconfig.build.json", 180, 20),
                workspace_file("apps/web/package.json", 140, 10),
                workspace_file("apps/web/src/main.ts", 900, 40),
                workspace_file("README.md", 500, 50),
            ],
        ));

        assert_eq!(layout.workspace_revision, 17);
        assert_eq!(layout.markers.len(), 3);
        assert_eq!(layout.markers[0].path.as_str(), "apps/web/package.json");
        assert_eq!(layout.markers[0].root.as_str(), "apps/web");
        assert_eq!(
            layout.markers[0].profile,
            ProjectLanguageProfile::TypeScriptJavaScript
        );
        assert_eq!(layout.markers[0].kind, ProjectMarkerKind::PackageManifest);
        assert_eq!(
            layout.markers[1].path.as_str(),
            "apps/web/tsconfig.build.json"
        );
        assert_eq!(layout.markers[1].root.as_str(), "apps/web");
        assert_eq!(layout.markers[1].kind, ProjectMarkerKind::TypeScriptConfig);
        assert_eq!(layout.markers[2].path.as_str(), "crates/runtime/Cargo.toml");
        assert_eq!(layout.markers[2].root.as_str(), "crates/runtime");
        assert_eq!(layout.markers[2].profile, ProjectLanguageProfile::Rust);
        assert_eq!(layout.markers[2].kind, ProjectMarkerKind::CargoManifest);
        assert_ne!(layout.layout_hash, LAYOUT_HASH_OFFSET_BASIS);
    }

    #[test]
    fn marker_order_and_duplicates_do_not_change_layout() {
        let cargo = workspace_file("services/api/Cargo.toml", 100, 10);
        let package = workspace_file("apps/web/package.json", 200, 20);
        let tsconfig = workspace_file("apps/web/tsconfig.json", 300, 30);

        let first = ProjectLayoutResolver::resolve(&snapshot(
            8,
            vec![cargo.clone(), package.clone(), tsconfig.clone()],
        ));
        let second = ProjectLayoutResolver::resolve(&snapshot(
            8,
            vec![tsconfig, package.clone(), cargo, package],
        ));

        assert_eq!(first, second);
        assert_eq!(first.markers.len(), 3);
    }

    #[test]
    fn source_changes_do_not_change_layout_hash() {
        let first = ProjectLayoutResolver::resolve(&snapshot(
            1,
            vec![
                workspace_file("Cargo.toml", 100, 10),
                workspace_file("src/lib.rs", 200, 20),
            ],
        ));
        let second = ProjectLayoutResolver::resolve(&snapshot(
            2,
            vec![
                workspace_file("Cargo.toml", 100, 10),
                workspace_file("src/lib.rs", 900, 90),
            ],
        ));

        assert_eq!(first.layout_hash, second.layout_hash);
        assert_eq!(first.markers, second.markers);
        assert_eq!(first.workspace_revision, 1);
        assert_eq!(second.workspace_revision, 2);
    }

    #[test]
    fn marker_metadata_changes_do_not_change_layout_hash() {
        let first = ProjectLayoutResolver::resolve(&snapshot(
            1,
            vec![workspace_file("apps/web/package.json", 100, 10)],
        ));
        let second = ProjectLayoutResolver::resolve(&snapshot(
            2,
            vec![workspace_file("apps/web/package.json", 101, 11)],
        ));

        assert_eq!(first.layout_hash, second.layout_hash);
        assert_eq!(first.markers, second.markers);
    }

    #[test]
    fn adding_or_removing_markers_changes_layout_hash() {
        let baseline = ProjectLayoutResolver::resolve(&snapshot(
            1,
            vec![workspace_file("apps/web/package.json", 100, 10)],
        ));
        let added = ProjectLayoutResolver::resolve(&snapshot(
            2,
            vec![
                workspace_file("apps/web/package.json", 100, 10),
                workspace_file("apps/web/tsconfig.json", 200, 20),
            ],
        ));
        let removed = ProjectLayoutResolver::resolve(&snapshot(3, Vec::new()));

        assert_ne!(baseline.layout_hash, added.layout_hash);
        assert_ne!(baseline.layout_hash, removed.layout_hash);
    }

    #[test]
    fn generated_and_binary_markers_are_ignored() {
        let baseline = ProjectLayoutResolver::resolve(&snapshot(
            1,
            vec![workspace_file("apps/web/package.json", 100, 10)],
        ));
        let mut generated = workspace_file("generated/client/tsconfig.json", 200, 20);
        generated.generated = true;
        let mut binary = workspace_file("fixtures/package.json", 300, 30);
        binary.binary = true;
        let with_ignored = ProjectLayoutResolver::resolve(&snapshot(
            2,
            vec![
                workspace_file("apps/web/package.json", 100, 10),
                generated,
                binary,
            ],
        ));

        assert_eq!(baseline.markers, with_ignored.markers);
        assert_eq!(baseline.layout_hash, with_ignored.layout_hash);
    }
}
