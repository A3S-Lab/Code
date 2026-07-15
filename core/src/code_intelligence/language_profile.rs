//! Built-in language server process profiles.

use super::project_layout::{ProjectLanguageProfile, ProjectLayout, ProjectMarkerKind};
use crate::language::LanguageCatalog;
use crate::workspace::WorkspacePath;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_INITIALIZATION_SETTLE_DELAY: Duration = Duration::from_millis(750);
const DEFAULT_NAVIGATION_SETTLE_DELAY: Duration = Duration::from_millis(250);

/// Process topology used by a language server profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ServerTopology {
    MultiFolder,
}

/// Typed command used to launch one language server process.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LanguageServerCommand {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
    pub(crate) env: BTreeMap<OsString, OsString>,
}

impl LanguageServerCommand {
    fn new(
        program: impl Into<PathBuf>,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            env: BTreeMap::new(),
        }
    }
}

/// A resolved, typed language server profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LanguageServerProfile {
    id: ProjectLanguageProfile,
    topology: ServerTopology,
    command: LanguageServerCommand,
    initialization_settle_delay: Duration,
    navigation_settle_delay: Duration,
}

impl LanguageServerProfile {
    pub(crate) fn rust(program: impl Into<PathBuf>) -> Self {
        Self {
            id: ProjectLanguageProfile::Rust,
            topology: ServerTopology::MultiFolder,
            command: LanguageServerCommand::new(program, std::iter::empty::<OsString>()),
            initialization_settle_delay: DEFAULT_INITIALIZATION_SETTLE_DELAY,
            navigation_settle_delay: DEFAULT_NAVIGATION_SETTLE_DELAY,
        }
    }

    pub(crate) fn typescript_javascript(program: impl Into<PathBuf>) -> Self {
        Self {
            id: ProjectLanguageProfile::TypeScriptJavaScript,
            topology: ServerTopology::MultiFolder,
            command: LanguageServerCommand::new(program, ["--stdio"]),
            initialization_settle_delay: DEFAULT_INITIALIZATION_SETTLE_DELAY,
            navigation_settle_delay: DEFAULT_NAVIGATION_SETTLE_DELAY,
        }
    }

    pub(crate) fn built_in_defaults() -> Vec<Self> {
        vec![
            Self::rust("rust-analyzer"),
            Self::typescript_javascript("typescript-language-server"),
        ]
    }

    pub(crate) fn id(&self) -> ProjectLanguageProfile {
        self.id
    }

    #[cfg(test)]
    pub(crate) fn topology(&self) -> ServerTopology {
        self.topology
    }

    pub(crate) fn command(&self) -> &LanguageServerCommand {
        &self.command
    }

    pub(crate) fn initialization_settle_delay(&self) -> Duration {
        self.initialization_settle_delay
    }

    pub(crate) fn navigation_settle_delay(&self) -> Duration {
        self.navigation_settle_delay
    }

    #[cfg(test)]
    pub(crate) fn with_settle_delays(
        mut self,
        initialization: Duration,
        navigation: Duration,
    ) -> Self {
        self.initialization_settle_delay = initialization;
        self.navigation_settle_delay = navigation;
        self
    }

    pub(crate) fn language_ids(&self) -> &'static [&'static str] {
        match self.id {
            ProjectLanguageProfile::Rust => &["rust"],
            ProjectLanguageProfile::TypeScriptJavaScript => &[
                "javascript",
                "javascript-react",
                "typescript",
                "typescript-react",
            ],
        }
    }

    pub(crate) fn supports_path(&self, path: &Path) -> bool {
        LanguageCatalog::id_for_path(path)
            .is_some_and(|language| self.language_ids().contains(&language))
    }

    /// Return stable, deduplicated workspace folders for this profile.
    pub(crate) fn project_roots(&self, layout: &ProjectLayout) -> Vec<WorkspacePath> {
        let roots = layout
            .markers
            .iter()
            .filter(|marker| marker.profile == self.id)
            .map(|marker| marker.root.as_str().to_string())
            .collect::<BTreeSet<_>>();

        if roots.is_empty() {
            vec![WorkspacePath::root()]
        } else {
            roots
                .into_iter()
                .map(WorkspacePath::from_normalized)
                .collect()
        }
    }

    /// Initialization options sent once when the process starts.
    pub(crate) fn initialization_options(
        &self,
        canonical_root: &Path,
        layout: &ProjectLayout,
    ) -> Value {
        match self.id {
            ProjectLanguageProfile::Rust => self.rust_configuration(canonical_root, layout),
            ProjectLanguageProfile::TypeScriptJavaScript => Value::Null,
        }
    }

    /// Section-addressable values returned from `workspace/configuration`.
    pub(crate) fn workspace_settings(
        &self,
        canonical_root: &Path,
        layout: &ProjectLayout,
    ) -> BTreeMap<String, Value> {
        match self.id {
            ProjectLanguageProfile::Rust => BTreeMap::from([(
                "rust-analyzer".to_string(),
                self.rust_configuration(canonical_root, layout),
            )]),
            ProjectLanguageProfile::TypeScriptJavaScript => BTreeMap::new(),
        }
    }

    fn rust_configuration(&self, canonical_root: &Path, layout: &ProjectLayout) -> Value {
        json!({
            "linkedProjects": self.rust_project_manifests(canonical_root, layout),
            "cargo": {
                "buildScripts": {
                    "enable": false,
                },
            },
            "procMacro": {
                "enable": false,
            },
            "checkOnSave": false,
        })
    }

    fn rust_project_manifests(&self, canonical_root: &Path, layout: &ProjectLayout) -> Vec<String> {
        layout
            .markers
            .iter()
            .filter(|marker| {
                marker.profile == ProjectLanguageProfile::Rust
                    && marker.kind == ProjectMarkerKind::CargoManifest
            })
            .map(|marker| {
                canonical_root
                    .join(marker.path.as_str())
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{LanguageServerProfile, ServerTopology};
    use crate::code_intelligence::project_layout::ProjectLayoutResolver;
    use crate::workspace::{
        LocalWorkspaceFile, LocalWorkspaceFileStatus, LocalWorkspaceManifestSnapshot,
    };
    use serde_json::json;
    use std::path::{Path, PathBuf};

    fn file(path: &str) -> LocalWorkspaceFile {
        LocalWorkspaceFile {
            path: path.to_string(),
            size: 1,
            modified_ms: Some(1),
            language: None,
            status: LocalWorkspaceFileStatus::Tracked,
            binary: false,
            generated: false,
        }
    }

    fn layout(paths: &[&str]) -> super::ProjectLayout {
        ProjectLayoutResolver::resolve(&LocalWorkspaceManifestSnapshot {
            version: 1,
            root: PathBuf::from("/workspace"),
            files: paths.iter().map(|path| file(path)).collect(),
            scanned_at_ms: 1,
        })
    }

    #[test]
    fn built_in_commands_and_languages_are_explicit() {
        let profiles = LanguageServerProfile::built_in_defaults();
        let rust = &profiles[0];
        let web = &profiles[1];

        assert_eq!(rust.command().program, PathBuf::from("rust-analyzer"));
        assert!(rust.command().args.is_empty());
        assert_eq!(rust.topology(), ServerTopology::MultiFolder);
        assert!(rust.supports_path(Path::new("src/lib.rs")));
        assert!(!rust.supports_path(Path::new("src/main.ts")));

        assert_eq!(
            web.command().program,
            PathBuf::from("typescript-language-server")
        );
        assert_eq!(web.command().args, ["--stdio"]);
        assert!(web.supports_path(Path::new("src/main.tsx")));
        assert!(web.supports_path(Path::new("src/main.jsx")));
    }

    #[test]
    fn project_roots_are_stable_and_deduplicated() {
        let profile = LanguageServerProfile::typescript_javascript("server");
        let layout = layout(&[
            "apps/web/package.json",
            "apps/web/tsconfig.json",
            "packages/ui/package.json",
        ]);

        assert_eq!(
            profile
                .project_roots(&layout)
                .into_iter()
                .map(|path| path.as_str().to_string())
                .collect::<Vec<_>>(),
            ["apps/web", "packages/ui"]
        );
        assert_eq!(
            LanguageServerProfile::rust("server").project_roots(&layout),
            [crate::workspace::WorkspacePath::root()]
        );
    }

    #[test]
    fn rust_settings_use_discovered_manifests_and_disable_workspace_code_execution() {
        let profile = LanguageServerProfile::rust("server");
        let layout = layout(&["crates/a/Cargo.toml", "crates/b/Cargo.toml"]);
        let root = Path::new("/workspace");
        let linked_projects = ["crates/a/Cargo.toml", "crates/b/Cargo.toml"]
            .map(|manifest| root.join(manifest).to_string_lossy().into_owned());
        let expected = json!({
            "linkedProjects": linked_projects,
            "cargo": {
                "buildScripts": {
                    "enable": false,
                },
            },
            "procMacro": {
                "enable": false,
            },
            "checkOnSave": false,
        });

        assert_eq!(profile.initialization_options(root, &layout), expected);
        assert_eq!(
            profile.workspace_settings(root, &layout)["rust-analyzer"],
            profile.initialization_options(root, &layout)
        );
    }
}
