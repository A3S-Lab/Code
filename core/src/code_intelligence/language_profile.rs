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

/// Process command and initialization payload resolved for one workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LanguageServerLaunch {
    pub(crate) command: LanguageServerCommand,
    pub(crate) initialization_options: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LaunchResolution {
    Static,
    WorkspaceTypeScript,
    /// Prefer basedpyright; fall back to stock pyright when only it is installed.
    PyrightFamily,
}

const BASEDPYRIGHT: &str = "basedpyright-langserver";
const PYRIGHT: &str = "pyright-langserver";

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
    launch_resolution: LaunchResolution,
    initialization_settle_delay: Duration,
    navigation_settle_delay: Duration,
}

impl LanguageServerProfile {
    pub(crate) fn rust(program: impl Into<PathBuf>) -> Self {
        Self {
            id: ProjectLanguageProfile::Rust,
            topology: ServerTopology::MultiFolder,
            command: LanguageServerCommand::new(program, std::iter::empty::<OsString>()),
            launch_resolution: LaunchResolution::Static,
            initialization_settle_delay: DEFAULT_INITIALIZATION_SETTLE_DELAY,
            navigation_settle_delay: DEFAULT_NAVIGATION_SETTLE_DELAY,
        }
    }

    pub(crate) fn typescript_javascript(program: impl Into<PathBuf>) -> Self {
        Self {
            id: ProjectLanguageProfile::TypeScriptJavaScript,
            topology: ServerTopology::MultiFolder,
            command: LanguageServerCommand::new(program, ["--stdio"]),
            launch_resolution: LaunchResolution::Static,
            initialization_settle_delay: DEFAULT_INITIALIZATION_SETTLE_DELAY,
            navigation_settle_delay: DEFAULT_NAVIGATION_SETTLE_DELAY,
        }
    }

    pub(crate) fn python(program: impl Into<PathBuf>) -> Self {
        Self {
            id: ProjectLanguageProfile::Python,
            topology: ServerTopology::MultiFolder,
            command: LanguageServerCommand::new(program, ["--stdio"]),
            launch_resolution: LaunchResolution::Static,
            initialization_settle_delay: DEFAULT_INITIALIZATION_SETTLE_DELAY,
            navigation_settle_delay: DEFAULT_NAVIGATION_SETTLE_DELAY,
        }
    }

    /// clangd's background index writes `.cache/clangd` into the project, so
    /// it stays off; queries index the opened translation units instead.
    pub(crate) fn c_cpp(program: impl Into<PathBuf>) -> Self {
        Self {
            id: ProjectLanguageProfile::CCpp,
            topology: ServerTopology::MultiFolder,
            command: LanguageServerCommand::new(program, ["--background-index=false"]),
            launch_resolution: LaunchResolution::Static,
            initialization_settle_delay: DEFAULT_INITIALIZATION_SETTLE_DELAY,
            navigation_settle_delay: DEFAULT_NAVIGATION_SETTLE_DELAY,
        }
    }

    pub(crate) fn built_in_defaults() -> Vec<Self> {
        let mut python = Self::python(BASEDPYRIGHT);
        python.launch_resolution = LaunchResolution::PyrightFamily;
        vec![
            Self::rust("rust-analyzer"),
            Self::typescript_javascript("typescript-language-server")
                .with_workspace_typescript_resolution(),
            python,
            Self::c_cpp("clangd"),
        ]
    }

    fn with_workspace_typescript_resolution(mut self) -> Self {
        self.launch_resolution = LaunchResolution::WorkspaceTypeScript;
        self
    }

    pub(crate) fn id(&self) -> ProjectLanguageProfile {
        self.id
    }

    #[cfg(test)]
    pub(crate) fn topology(&self) -> ServerTopology {
        self.topology
    }

    #[cfg(test)]
    pub(crate) fn command(&self) -> &LanguageServerCommand {
        &self.command
    }

    /// Resolve a project-local TypeScript runtime without assuming that a
    /// monorepo hoists its SDK to the served workspace root. Classic SDKs are
    /// pinned through `tsserver.path`; TypeScript 7+ uses its native LSP mode.
    pub(crate) async fn launch(
        &self,
        canonical_root: &Path,
        layout: &ProjectLayout,
    ) -> LanguageServerLaunch {
        if self.launch_resolution == LaunchResolution::WorkspaceTypeScript {
            if let Some(launch) = self
                .workspace_typescript_launch(canonical_root, layout)
                .await
            {
                return launch;
            }
        }
        if self.launch_resolution == LaunchResolution::PyrightFamily
            && !on_path(BASEDPYRIGHT)
            && on_path(PYRIGHT)
        {
            return LanguageServerLaunch {
                command: LanguageServerCommand::new(PYRIGHT, ["--stdio"]),
                initialization_options: Value::Null,
            };
        }

        LanguageServerLaunch {
            command: self.command.clone(),
            initialization_options: self.initialization_options(canonical_root, layout),
        }
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
            ProjectLanguageProfile::Python => &["python"],
            ProjectLanguageProfile::CCpp => &["c", "cpp"],
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
            ProjectLanguageProfile::TypeScriptJavaScript
            | ProjectLanguageProfile::Python
            | ProjectLanguageProfile::CCpp => Value::Null,
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
            ProjectLanguageProfile::TypeScriptJavaScript
            | ProjectLanguageProfile::Python
            | ProjectLanguageProfile::CCpp => BTreeMap::new(),
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
                join_workspace_path(canonical_root, &marker.path)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    async fn workspace_typescript_launch(
        &self,
        canonical_root: &Path,
        layout: &ProjectLayout,
    ) -> Option<LanguageServerLaunch> {
        for search_root in self.typescript_search_roots(canonical_root, layout) {
            for package_root in typescript_package_roots(&search_root) {
                let lib = package_root.join("lib");
                let tsserver = lib.join("tsserver.js");
                if is_file(&tsserver).await {
                    return Some(LanguageServerLaunch {
                        command: self.command.clone(),
                        initialization_options: json!({
                            "tsserver": {
                                "path": protocol_path(&tsserver),
                            },
                        }),
                    });
                }

                let native_entry = lib.join("tsc.js");
                if is_file(&native_entry).await
                    && typescript_major_version(&package_root)
                        .await
                        .is_some_and(|major| major >= 7)
                {
                    return Some(LanguageServerLaunch {
                        command: LanguageServerCommand::new(
                            "node",
                            [
                                native_entry.into_os_string(),
                                OsString::from("--lsp"),
                                OsString::from("--stdio"),
                            ],
                        ),
                        initialization_options: Value::Null,
                    });
                }
            }
        }
        None
    }

    fn typescript_search_roots(
        &self,
        canonical_root: &Path,
        layout: &ProjectLayout,
    ) -> Vec<PathBuf> {
        let project_directories = self
            .project_roots(layout)
            .into_iter()
            .map(|root| {
                if root.is_root() {
                    canonical_root.to_path_buf()
                } else {
                    join_workspace_path(canonical_root, &root)
                }
            })
            .collect::<Vec<_>>();
        let mut roots = Vec::new();
        let mut seen = BTreeSet::new();

        // Prefer each package's own SDK before considering any hoisted SDK.
        for directory in &project_directories {
            push_unique_path(&mut roots, &mut seen, directory.clone());
        }
        for directory in project_directories {
            let mut ancestor = directory.parent();
            while let Some(candidate) = ancestor.filter(|path| path.starts_with(canonical_root)) {
                push_unique_path(&mut roots, &mut seen, candidate.to_path_buf());
                if candidate == canonical_root {
                    break;
                }
                ancestor = candidate.parent();
            }
        }
        roots
    }
}

fn typescript_package_roots(search_root: &Path) -> [PathBuf; 3] {
    [
        search_root.join("node_modules").join("typescript"),
        search_root.join(".yarn").join("sdks").join("typescript"),
        search_root
            .join(".vscode")
            .join("pnpify")
            .join("typescript"),
    ]
}

fn join_workspace_path(root: &Path, path: &WorkspacePath) -> PathBuf {
    path.as_str()
        .split('/')
        .fold(root.to_path_buf(), |joined, component| {
            joined.join(component)
        })
}

fn push_unique_path(target: &mut Vec<PathBuf>, seen: &mut BTreeSet<PathBuf>, path: PathBuf) {
    if seen.insert(path.clone()) {
        target.push(path);
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|directory| directory.join(program).is_file())
    })
}

async fn is_file(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .is_ok_and(|metadata| metadata.is_file())
}

async fn typescript_major_version(package_root: &Path) -> Option<u64> {
    let package = tokio::fs::read_to_string(package_root.join("package.json"))
        .await
        .ok()?;
    let package: Value = serde_json::from_str(&package).ok()?;
    package
        .get("version")?
        .as_str()?
        .split('.')
        .next()?
        .parse()
        .ok()
}

fn protocol_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::{protocol_path, LanguageServerProfile, ServerTopology};
    use crate::code_intelligence::project_layout::{ProjectLanguageProfile, ProjectLayoutResolver};
    use crate::workspace::{
        LocalWorkspaceFile, LocalWorkspaceFileStatus, LocalWorkspaceManifestSnapshot,
    };
    use serde_json::{json, Value};
    use std::ffi::OsString;
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

    fn typescript_default() -> LanguageServerProfile {
        LanguageServerProfile::built_in_defaults()
            .into_iter()
            .find(|profile| profile.id() == ProjectLanguageProfile::TypeScriptJavaScript)
            .expect("TypeScript profile")
    }

    #[test]
    fn built_in_commands_and_languages_are_explicit() {
        let profiles = LanguageServerProfile::built_in_defaults();
        let [rust, web, python, native] = &profiles[..] else {
            panic!("four built-in profiles");
        };

        assert_eq!(
            python.command().program,
            PathBuf::from("basedpyright-langserver")
        );
        assert_eq!(python.command().args, ["--stdio"]);
        assert!(python.supports_path(Path::new("pkg/mod.py")));
        assert!(!python.supports_path(Path::new("src/lib.rs")));

        assert_eq!(native.command().program, PathBuf::from("clangd"));
        assert_eq!(native.command().args, ["--background-index=false"]);
        for path in [
            "src/main.c",
            "include/api.h",
            "src/main.cpp",
            "src/a.cc",
            "inc/a.hpp",
        ] {
            assert!(native.supports_path(Path::new(path)), "{path}");
        }
        assert!(!native.supports_path(Path::new("pkg/mod.py")));
        for profile in [python, native] {
            let layout = layout(&["Cargo.toml"]);
            assert_eq!(
                profile.project_roots(&layout),
                [crate::workspace::WorkspacePath::root()]
            );
            assert_eq!(
                profile.initialization_options(Path::new("/w"), &layout),
                Value::Null
            );
            assert!(profile
                .workspace_settings(Path::new("/w"), &layout)
                .is_empty());
        }

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

    #[tokio::test]
    async fn built_in_typescript_uses_nested_classic_sdk() {
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let package = workspace
            .path()
            .join("apps")
            .join("frontend")
            .join("node_modules")
            .join("typescript");
        tokio::fs::create_dir_all(package.join("lib"))
            .await
            .expect("create TypeScript SDK");
        tokio::fs::write(package.join("lib").join("tsserver.js"), "")
            .await
            .expect("write tsserver entry");
        tokio::fs::write(package.join("package.json"), r#"{"version":"5.9.3"}"#)
            .await
            .expect("write TypeScript package");
        let profile = typescript_default();

        let launch = profile
            .launch(
                workspace.path(),
                &layout(&["apps/frontend/package.json", "apps/frontend/tsconfig.json"]),
            )
            .await;

        assert_eq!(
            launch.command.program,
            PathBuf::from("typescript-language-server")
        );
        assert_eq!(launch.command.args, ["--stdio"]);
        assert_eq!(
            launch.initialization_options,
            json!({
                "tsserver": {
                    "path": protocol_path(&package.join("lib").join("tsserver.js")),
                },
            })
        );
    }

    #[tokio::test]
    async fn built_in_typescript_uses_nested_native_sdk() {
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let package = workspace
            .path()
            .join("apps")
            .join("frontend")
            .join("node_modules")
            .join("typescript");
        tokio::fs::create_dir_all(package.join("lib"))
            .await
            .expect("create TypeScript SDK");
        tokio::fs::write(package.join("lib").join("tsc.js"), "")
            .await
            .expect("write native TypeScript entry");
        tokio::fs::write(package.join("package.json"), r#"{"version":"7.0.2"}"#)
            .await
            .expect("write TypeScript package");
        let profile = typescript_default();

        let launch = profile
            .launch(
                workspace.path(),
                &layout(&["apps/frontend/package.json", "apps/frontend/tsconfig.json"]),
            )
            .await;

        assert_eq!(launch.command.program, PathBuf::from("node"));
        assert_eq!(
            launch.command.args,
            [
                package.join("lib").join("tsc.js").into_os_string(),
                OsString::from("--lsp"),
                OsString::from("--stdio"),
            ]
        );
        assert_eq!(launch.initialization_options, Value::Null);
    }

    #[tokio::test]
    async fn workspace_typescript_prefers_package_sdk_over_hoisted_sdk() {
        let workspace = tempfile::tempdir().expect("temporary workspace");
        let hoisted = workspace.path().join("node_modules").join("typescript");
        let local = workspace
            .path()
            .join("packages")
            .join("ui")
            .join("node_modules")
            .join("typescript");
        for package in [&hoisted, &local] {
            tokio::fs::create_dir_all(package.join("lib"))
                .await
                .expect("create TypeScript SDK");
            tokio::fs::write(package.join("lib").join("tsserver.js"), "")
                .await
                .expect("write tsserver entry");
            tokio::fs::write(package.join("package.json"), r#"{"version":"5.9.3"}"#)
                .await
                .expect("write TypeScript package");
        }
        let profile = typescript_default();

        let launch = profile
            .launch(
                workspace.path(),
                &layout(&["apps/frontend/package.json", "packages/ui/package.json"]),
            )
            .await;

        assert_eq!(
            launch.initialization_options["tsserver"]["path"],
            protocol_path(&local.join("lib").join("tsserver.js"))
        );
    }

    #[test]
    fn project_roots_are_stable_and_deduplicated() {
        let profile = LanguageServerProfile::typescript_javascript("server");
        let layout = layout(&[
            "apps/frontend/package.json",
            "apps/frontend/tsconfig.json",
            "packages/ui/package.json",
        ]);

        assert_eq!(
            profile
                .project_roots(&layout)
                .into_iter()
                .map(|path| path.as_str().to_string())
                .collect::<Vec<_>>(),
            ["apps/frontend", "packages/ui"]
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
            .map(|manifest| root.join(manifest).to_string_lossy().replace('\\', "/"));
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

    #[test]
    fn rust_linked_projects_use_protocol_path_separators() {
        let profile = LanguageServerProfile::rust("server");
        let layout = layout(&["crates/a/Cargo.toml"]);

        assert_eq!(
            profile.initialization_options(Path::new(r"C:\workspace"), &layout)["linkedProjects"],
            json!(["C:/workspace/crates/a/Cargo.toml"])
        );
    }
}
