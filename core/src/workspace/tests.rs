use super::*;

#[test]
fn virtual_resolver_normalizes_relative_paths() {
    let resolver = VirtualPathResolver;
    let path = resolver.normalize("./src/../README.md").unwrap();
    assert_eq!(path.as_str(), "README.md");
}

#[test]
fn virtual_resolver_normalizes_backslash_separators() {
    let resolver = VirtualPathResolver;
    let path = resolver.normalize(r"src\main.rs").unwrap();
    assert_eq!(path.as_str(), "src/main.rs");
}

#[test]
fn virtual_resolver_rejects_escape() {
    let resolver = VirtualPathResolver;
    let err = resolver.normalize("../secret.txt").unwrap_err();
    assert!(err.to_string().contains("escapes workspace"));
}

#[test]
fn virtual_resolver_rejects_backslash_escape() {
    let resolver = VirtualPathResolver;
    let err = resolver.normalize(r"..\secret.txt").unwrap_err();
    assert!(err.to_string().contains("escapes workspace"));
}

#[test]
fn virtual_resolver_rejects_absolute_paths() {
    let resolver = VirtualPathResolver;
    let err = resolver.normalize("/tmp/secret.txt").unwrap_err();
    assert!(err.to_string().contains("Absolute paths"));
}

#[test]
fn virtual_resolver_rejects_windows_absolute_paths() {
    let resolver = VirtualPathResolver;

    let drive_err = resolver.normalize(r"C:\Users\secret.txt").unwrap_err();
    assert!(drive_err.to_string().contains("Absolute paths"));

    let unc_err = resolver
        .normalize(r"\\server\share\secret.txt")
        .unwrap_err();
    assert!(unc_err.to_string().contains("Absolute paths"));
}

#[test]
fn workspace_services_disable_exec_without_runner() {
    struct EmptyFs;

    #[async_trait]
    impl WorkspaceFileSystem for EmptyFs {
        async fn read_text(&self, _path: &WorkspacePath) -> WorkspaceResult<String> {
            Err(WorkspaceError::Unsupported("not implemented".into()))
        }

        async fn write_text(
            &self,
            _path: &WorkspacePath,
            _content: &str,
        ) -> WorkspaceResult<WorkspaceWriteOutcome> {
            Err(WorkspaceError::Unsupported("not implemented".into()))
        }

        async fn list_dir(&self, _path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
            Err(WorkspaceError::Unsupported("not implemented".into()))
        }
    }

    let fs_backend: Arc<dyn WorkspaceFileSystem> = Arc::new(EmptyFs);
    let services = WorkspaceServices::builder(
        WorkspaceRef::new("virtual", "virtual://workspace"),
        fs_backend,
    )
    .capabilities(WorkspaceCapabilities {
        exec: true,
        ..WorkspaceCapabilities::read_write()
    })
    .build();

    assert!(!services.capabilities().exec);
    assert!(services.command_runner().is_none());
}

// --- helpers for fs_ext / read_for_edit / write_for_edit coverage ---
//
// The mock backend used here is `InMemoryFileSystem` from the
// `workspace::conformance` module — the same fixture the conformance
// suite runs against, so any drift between the test fixture and the
// contract documented for new backends shows up immediately. Tests
// capture the auto-generated version at read time rather than
// hard-coding it, which keeps assertions decoupled from the version
// scheme of the mock.

use super::conformance::InMemoryFileSystem;

fn versioned_services(fs: Arc<InMemoryFileSystem>) -> Arc<WorkspaceServices> {
    let fs_ws: Arc<dyn WorkspaceFileSystem> = fs.clone();
    let fs_ext: Arc<dyn WorkspaceFileSystemExt> = fs;
    WorkspaceServices::builder(WorkspaceRef::new("mem", "mem://ws"), fs_ws)
        .file_system_ext(fs_ext)
        .build()
}

/// Seed a file into the mock and return its current version, so callers
/// can use it as an expected value without hardcoding the version
/// scheme.
async fn seed(fs: &Arc<InMemoryFileSystem>, path: &str, content: &str) -> String {
    use super::WorkspaceFileSystemExt;
    let ws_path = WorkspacePath::from_normalized(path);
    (*fs).write_text(&ws_path, content).await.unwrap();
    let (_, version) = (*fs).read_text_with_version(&ws_path).await.unwrap();
    version
}

#[test]
fn version_conflict_is_downcastable_from_anyhow() {
    let e: anyhow::Error = anyhow::Error::new(WorkspaceVersionConflict {
        path: "a/b.txt".to_string(),
        expected: "etag-1".to_string(),
        actual: Some("etag-2".to_string()),
    });
    let c = e.downcast_ref::<WorkspaceVersionConflict>().unwrap();
    assert_eq!(c.path, "a/b.txt");
    assert_eq!(c.expected, "etag-1");
    assert_eq!(c.actual.as_deref(), Some("etag-2"));
    // Display must include the path and both versions so logs are useful.
    let msg = e.to_string();
    assert!(msg.contains("a/b.txt"), "msg: {msg}");
    assert!(msg.contains("etag-1"), "msg: {msg}");
}

#[tokio::test]
async fn read_for_edit_returns_version_when_ext_available() {
    let fs = Arc::new(InMemoryFileSystem::new());
    let seeded_version = seed(&fs, "notes.md", "hello").await;
    let services = versioned_services(fs);

    let path = WorkspacePath::from_normalized("notes.md");
    let (content, version) = services.read_for_edit(&path).await.unwrap();
    assert_eq!(content, "hello");
    assert_eq!(
        version.as_deref(),
        Some(seeded_version.as_str()),
        "read_for_edit must return the version produced by the prior write"
    );
}

#[tokio::test]
async fn read_for_edit_returns_no_version_when_ext_absent() {
    struct PlainFs;
    #[async_trait]
    impl WorkspaceFileSystem for PlainFs {
        async fn read_text(&self, _path: &WorkspacePath) -> WorkspaceResult<String> {
            Ok("plain".to_string())
        }
        async fn write_text(
            &self,
            _path: &WorkspacePath,
            content: &str,
        ) -> WorkspaceResult<WorkspaceWriteOutcome> {
            Ok(WorkspaceWriteOutcome {
                bytes: content.len(),
                lines: content.lines().count(),
            })
        }
        async fn list_dir(&self, _path: &WorkspacePath) -> WorkspaceResult<Vec<WorkspaceDirEntry>> {
            Ok(Vec::new())
        }
    }
    let fs: Arc<dyn WorkspaceFileSystem> = Arc::new(PlainFs);
    let services = WorkspaceServices::builder(WorkspaceRef::new("plain", "plain://ws"), fs).build();

    let path = WorkspacePath::from_normalized("any.txt");
    let (content, version) = services.read_for_edit(&path).await.unwrap();
    assert_eq!(content, "plain");
    assert!(version.is_none());
    assert!(services.fs_ext().is_none());
}

#[tokio::test]
async fn write_for_edit_succeeds_on_matching_version() {
    let fs = Arc::new(InMemoryFileSystem::new());
    seed(&fs, "doc.md", "alpha").await;
    let services = versioned_services(fs.clone());
    let path = WorkspacePath::from_normalized("doc.md");

    let (content, version) = services.read_for_edit(&path).await.unwrap();
    assert_eq!(content, "alpha");

    services
        .write_for_edit(&path, "beta", version.as_deref())
        .await
        .expect("write should succeed with matching version");

    let current = fs.read_text(&path).await.unwrap();
    assert_eq!(current, "beta");
}

struct TestCodeIntelligence {
    status: tokio::sync::watch::Sender<crate::code_intelligence::CodeIntelligenceStatus>,
}

impl TestCodeIntelligence {
    fn ready() -> Self {
        use crate::code_intelligence::{
            CodeIntelligenceCapabilities, CodeIntelligenceState, CodeIntelligenceStatus,
        };

        let (status, _) = tokio::sync::watch::channel(CodeIntelligenceStatus {
            state: CodeIntelligenceState::Ready,
            capabilities: CodeIntelligenceCapabilities {
                document_symbols: true,
                ..CodeIntelligenceCapabilities::default()
            },
            languages: Vec::new(),
            message: None,
        });
        Self { status }
    }

    fn unavailable<T>() -> crate::code_intelligence::CodeIntelligenceResult<T> {
        Err(
            crate::code_intelligence::CodeIntelligenceError::Unavailable {
                message: "test provider has no runtime".to_string(),
            },
        )
    }
}

#[async_trait]
impl crate::code_intelligence::WorkspaceCodeIntelligence for TestCodeIntelligence {
    fn subscribe_status(
        &self,
    ) -> tokio::sync::watch::Receiver<crate::code_intelligence::CodeIntelligenceStatus> {
        self.status.subscribe()
    }

    async fn document_symbols(
        &self,
        _path: &WorkspacePath,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> crate::code_intelligence::CodeIntelligenceResult<
        crate::code_intelligence::CodeQueryResult<crate::code_intelligence::DocumentSymbol>,
    > {
        Self::unavailable()
    }

    async fn search_symbols(
        &self,
        _query: &str,
        _limit: usize,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> crate::code_intelligence::CodeIntelligenceResult<
        crate::code_intelligence::CodeQueryResult<crate::code_intelligence::SymbolInformation>,
    > {
        Self::unavailable()
    }

    async fn navigate(
        &self,
        _kind: crate::code_intelligence::NavigationKind,
        _path: &WorkspacePath,
        _position: crate::code_intelligence::CodePosition,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> crate::code_intelligence::CodeIntelligenceResult<
        crate::code_intelligence::CodeQueryResult<crate::code_intelligence::CodeLocation>,
    > {
        Self::unavailable()
    }

    async fn diagnostics(
        &self,
        _path: Option<&WorkspacePath>,
        _cancellation: tokio_util::sync::CancellationToken,
    ) -> crate::code_intelligence::CodeIntelligenceResult<
        crate::code_intelligence::CodeQueryResult<crate::code_intelligence::CodeDiagnostic>,
    > {
        Self::unavailable()
    }
}

#[test]
fn workspace_services_builder_attaches_code_intelligence() {
    use crate::code_intelligence::{CodeIntelligenceState, WorkspaceCodeIntelligence};

    let fs = Arc::new(InMemoryFileSystem::new());
    let fs_backend: Arc<dyn WorkspaceFileSystem> = fs;
    let provider: Arc<dyn WorkspaceCodeIntelligence> = Arc::new(TestCodeIntelligence::ready());
    let services =
        WorkspaceServices::builder(WorkspaceRef::new("semantic", "mem://semantic"), fs_backend)
            .code_intelligence(provider)
            .build();

    assert!(services.capabilities().code_intelligence);
    assert!(services.code_intelligence().is_some());
    assert_eq!(
        services.code_intelligence().unwrap().status().state,
        CodeIntelligenceState::Ready
    );
}

#[test]
fn code_intelligence_decorator_preserves_existing_services() {
    use crate::code_intelligence::WorkspaceCodeIntelligence;

    let fs = Arc::new(InMemoryFileSystem::new());
    let fs_backend: Arc<dyn WorkspaceFileSystem> = fs;
    let services = WorkspaceServices::builder(
        WorkspaceRef::new("decorated", "mem://decorated"),
        fs_backend,
    )
    .build();
    let provider: Arc<dyn WorkspaceCodeIntelligence> = Arc::new(TestCodeIntelligence::ready());
    let decorated = services.with_code_intelligence(provider);

    assert!(decorated.capabilities().read);
    assert!(decorated.capabilities().write);
    assert!(decorated.capabilities().code_intelligence);
    assert!(decorated.code_intelligence().is_some());
    assert_eq!(decorated.workspace_ref(), services.workspace_ref());
}

#[tokio::test]
async fn write_for_edit_surfaces_conflict_when_version_changed() {
    let fs = Arc::new(InMemoryFileSystem::new());
    let seeded_version = seed(&fs, "doc.md", "alpha").await;
    let services = versioned_services(fs.clone());
    let path = WorkspacePath::from_normalized("doc.md");

    let (_, version) = services.read_for_edit(&path).await.unwrap();
    // Simulate a concurrent overwrite — a real "second writer" doing
    // exactly what the conflict protection is meant to catch.
    fs.write_text(&path, "from-concurrent-writer")
        .await
        .unwrap();

    let err = services
        .write_for_edit(&path, "beta", version.as_deref())
        .await
        .expect_err("write should reject with conflict");
    let WorkspaceError::VersionConflict(conflict) = err else {
        panic!("expected WorkspaceError::VersionConflict, got {err:?}");
    };
    assert_eq!(conflict.path, "doc.md");
    assert_eq!(conflict.expected, seeded_version);
    // We don't pin the actual version's exact value — only that the
    // backend supplies one and that it differs from what we expected.
    let actual = conflict
        .actual
        .as_deref()
        .expect("conflict must report the current version");
    assert_ne!(actual, seeded_version);
}

#[tokio::test]
async fn write_for_edit_falls_back_to_plain_write_when_version_is_none() {
    // Even with fs_ext present, passing version=None must route through
    // unconditional write_text (e.g. for fresh-create paths).
    let fs = Arc::new(InMemoryFileSystem::new());
    seed(&fs, "doc.md", "alpha").await;
    let services = versioned_services(fs.clone());
    let path = WorkspacePath::from_normalized("doc.md");

    // Concurrent overwriter, but caller did not request CAS semantics:
    fs.write_text(&path, "from-concurrent-writer")
        .await
        .unwrap();

    services
        .write_for_edit(&path, "beta", None)
        .await
        .expect("plain write should not check version");
    let current = fs.read_text(&path).await.unwrap();
    assert_eq!(current, "beta");
}
