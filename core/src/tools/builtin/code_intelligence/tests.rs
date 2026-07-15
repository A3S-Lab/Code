use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{watch, Mutex};
use tokio_util::sync::CancellationToken;

use super::{
    diagnostics::CodeDiagnosticsTool, navigation::CodeNavigationTool, symbols::CodeSymbolsTool,
};
use crate::{
    code_intelligence::{
        CodeDiagnostic, CodeDiagnosticSeverity, CodeIntelligenceCapabilities,
        CodeIntelligenceError, CodeIntelligenceResult, CodeIntelligenceState,
        CodeIntelligenceStatus, CodeLocation, CodePosition, CodeQueryResult, CodeRange,
        CodeSymbolKind, DocumentRevision, DocumentSnapshot, DocumentSymbol, NavigationKind,
        SymbolInformation, WorkspaceCodeIntelligence,
    },
    tools::{Tool, ToolContext, ToolErrorKind, ToolOutputKind},
    workspace::{WorkspacePath, WorkspaceServices},
};

struct TestProvider {
    status: watch::Sender<CodeIntelligenceStatus>,
    calls: AtomicUsize,
    fail: bool,
    last_navigation: Mutex<Option<(NavigationKind, WorkspacePath, CodePosition)>>,
    last_search: Mutex<Option<(String, usize)>>,
}

impl TestProvider {
    fn new(fail: bool) -> Arc<Self> {
        let (status, _) = watch::channel(CodeIntelligenceStatus {
            state: CodeIntelligenceState::Ready,
            capabilities: CodeIntelligenceCapabilities {
                document_symbols: true,
                workspace_symbols: true,
                definition: true,
                declaration: true,
                references: true,
                implementations: true,
                diagnostics: true,
            },
            languages: Vec::new(),
            message: None,
        });
        Arc::new(Self {
            status,
            calls: AtomicUsize::new(0),
            fail,
            last_navigation: Mutex::new(None),
            last_search: Mutex::new(None),
        })
    }

    fn unavailable<T>(&self) -> CodeIntelligenceResult<T> {
        Err(CodeIntelligenceError::Unavailable {
            message: "test runtime unavailable".to_owned(),
        })
    }

    fn snapshot() -> DocumentSnapshot {
        DocumentSnapshot {
            revision: DocumentRevision::new(7),
            content_hash: "saved-content-hash".to_owned(),
            stale: false,
        }
    }

    fn result<T>(items: Vec<T>, document: Option<DocumentSnapshot>) -> CodeQueryResult<T> {
        CodeQueryResult {
            items,
            truncated: false,
            workspace_revision: 11,
            document,
        }
    }
}

#[async_trait]
impl WorkspaceCodeIntelligence for TestProvider {
    fn subscribe_status(&self) -> watch::Receiver<CodeIntelligenceStatus> {
        self.status.subscribe()
    }

    async fn document_symbols(
        &self,
        _path: &WorkspacePath,
        _cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<DocumentSymbol>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.fail {
            return self.unavailable();
        }
        Ok(Self::result(
            vec![DocumentSymbol {
                name: "WorkspaceClient".to_owned(),
                detail: Some("struct".to_owned()),
                kind: CodeSymbolKind::Struct,
                range: test_range(),
                selection_range: test_range(),
                children: vec![DocumentSymbol {
                    name: "navigate".to_owned(),
                    detail: None,
                    kind: CodeSymbolKind::Method,
                    range: test_range(),
                    selection_range: test_range(),
                    children: Vec::new(),
                }],
            }],
            Some(Self::snapshot()),
        ))
    }

    async fn search_symbols(
        &self,
        query: &str,
        limit: usize,
        _cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<SymbolInformation>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        *self.last_search.lock().await = Some((query.to_owned(), limit));
        if self.fail {
            return self.unavailable();
        }
        Ok(Self::result(
            vec![SymbolInformation {
                name: "WorkspaceClient".to_owned(),
                kind: CodeSymbolKind::Struct,
                location: CodeLocation {
                    path: WorkspacePath::from_normalized("src/client.rs"),
                    range: test_range(),
                },
                container_name: Some("runtime".to_owned()),
            }],
            None,
        ))
    }

    async fn navigate(
        &self,
        kind: NavigationKind,
        path: &WorkspacePath,
        position: CodePosition,
        _cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<CodeLocation>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        *self.last_navigation.lock().await = Some((kind, path.clone(), position));
        if self.fail {
            return self.unavailable();
        }
        Ok(Self::result(
            vec![CodeLocation {
                path: WorkspacePath::from_normalized("src/implementation.rs"),
                range: test_range(),
            }],
            Some(Self::snapshot()),
        ))
    }

    async fn diagnostics(
        &self,
        path: Option<&WorkspacePath>,
        _cancellation: CancellationToken,
    ) -> CodeIntelligenceResult<CodeQueryResult<CodeDiagnostic>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.fail {
            return self.unavailable();
        }
        let path = path
            .cloned()
            .unwrap_or_else(|| WorkspacePath::from_normalized("src/lib.rs"));
        Ok(Self::result(
            vec![CodeDiagnostic {
                location: CodeLocation {
                    path,
                    range: test_range(),
                },
                severity: Some(CodeDiagnosticSeverity::Warning),
                code: Some("unused".to_owned()),
                source: Some("compiler".to_owned()),
                message: "unused symbol".to_owned(),
            }],
            Some(Self::snapshot()),
        ))
    }
}

fn test_range() -> CodeRange {
    CodeRange::new(CodePosition::new(2, 4), CodePosition::new(2, 12))
}

fn context(provider: Arc<TestProvider>) -> (tempfile::TempDir, ToolContext) {
    let temp = tempfile::tempdir().unwrap();
    let provider: Arc<dyn WorkspaceCodeIntelligence> = provider;
    let services = WorkspaceServices::local(temp.path()).with_code_intelligence(provider);
    let context = ToolContext::new(temp.path().to_path_buf()).with_workspace_services(services);
    (temp, context)
}

fn output_json(output: &crate::tools::ToolOutput) -> Value {
    serde_json::from_str(&output.content).unwrap()
}

#[tokio::test]
async fn missing_and_failing_providers_return_typed_unavailable_errors() {
    let temp = tempfile::tempdir().unwrap();
    let missing = CodeSymbolsTool
        .execute(
            &json!({"operation": "search", "query": "Client"}),
            &ToolContext::new(temp.path().to_path_buf()),
        )
        .await
        .unwrap();
    assert!(!missing.success);
    assert!(missing.content.contains("Code Intelligence"));
    assert_eq!(
        missing.metadata.unwrap()["code"],
        "CODE_INTELLIGENCE_UNAVAILABLE"
    );

    let failing = TestProvider::new(true);
    let (_temp, context) = context(Arc::clone(&failing));
    let output = CodeDiagnosticsTool
        .execute(&json!({}), &context)
        .await
        .unwrap();
    assert!(!output.success);
    assert_eq!(
        output.metadata.unwrap()["code"],
        "CODE_INTELLIGENCE_UNAVAILABLE"
    );
    assert_eq!(failing.calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn invalid_arguments_and_escaping_paths_never_reach_provider() {
    let provider = TestProvider::new(false);
    let (_temp, context) = context(Arc::clone(&provider));

    for output in [
        CodeSymbolsTool
            .execute(&json!({"operation": "search", "query": "  "}), &context)
            .await
            .unwrap(),
        CodeSymbolsTool
            .execute(
                &json!({"operation": "outline", "path": "../outside.rs"}),
                &context,
            )
            .await
            .unwrap(),
        CodeNavigationTool
            .execute(
                &json!({
                    "operation": "definition",
                    "path": "src/lib.rs",
                    "line": -1,
                    "character": 0
                }),
                &context,
            )
            .await
            .unwrap(),
        CodeDiagnosticsTool
            .execute(&json!({"path": "../../outside.rs"}), &context)
            .await
            .unwrap(),
    ] {
        assert!(!output.success);
        assert!(matches!(
            output.error_kind,
            Some(ToolErrorKind::InvalidArgument { .. })
        ));
    }
    assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn symbols_tool_maps_outline_and_bounded_workspace_search() {
    let provider = TestProvider::new(false);
    let (_temp, context) = context(Arc::clone(&provider));

    let outline = CodeSymbolsTool
        .execute(
            &json!({"operation": "outline", "path": "src/lib.rs"}),
            &context,
        )
        .await
        .unwrap();
    assert!(outline.success);
    let outline = output_json(&outline);
    assert_eq!(outline["items"][0]["name"], "WorkspaceClient");
    assert_eq!(outline["items"][0]["kind"], "struct");
    assert_eq!(outline["items"][0]["children"][0]["kind"], "method");
    assert_eq!(outline["document"]["revision"], 7);

    let search = CodeSymbolsTool
        .execute(
            &json!({"operation": "search", "query": " Client ", "limit": 25}),
            &context,
        )
        .await
        .unwrap();
    assert!(search.success);
    let search = output_json(&search);
    assert_eq!(search["items"][0]["location"]["path"], "src/client.rs");
    assert_eq!(
        *provider.last_search.lock().await,
        Some(("Client".to_owned(), 25))
    );
}

#[tokio::test]
async fn navigation_tool_uses_typed_utf16_position_and_kind() {
    let provider = TestProvider::new(false);
    let (_temp, context) = context(Arc::clone(&provider));
    let output = CodeNavigationTool
        .execute(
            &json!({
                "operation": "implementations",
                "path": "src/lib.rs",
                "line": 3,
                "character": 9
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(output.success);
    assert_eq!(
        output_json(&output)["items"][0]["path"],
        "src/implementation.rs"
    );
    assert_eq!(
        *provider.last_navigation.lock().await,
        Some((
            NavigationKind::Implementations,
            WorkspacePath::from_normalized("src/lib.rs"),
            CodePosition::new(3, 9),
        ))
    );
}

#[tokio::test]
async fn diagnostics_tool_maps_typed_diagnostics_and_saved_snapshot() {
    let provider = TestProvider::new(false);
    let (_temp, context) = context(provider);
    let output = CodeDiagnosticsTool
        .execute(&json!({"path": "src/lib.rs"}), &context)
        .await
        .unwrap();

    assert!(output.success);
    let value = output_json(&output);
    assert_eq!(value["items"][0]["location"]["path"], "src/lib.rs");
    assert_eq!(value["items"][0]["severity"], "warning");
    assert_eq!(value["items"][0]["code"], "unused");
    assert_eq!(value["document"]["content_hash"], "saved-content-hash");
}

#[tokio::test]
async fn registration_is_capability_gated_and_tools_are_structured_query_reads() {
    let temp = tempfile::tempdir().unwrap();
    let plain_services = WorkspaceServices::local(temp.path());
    let plain = crate::tools::ToolExecutor::new_with_workspace_services(
        temp.path().display().to_string(),
        plain_services,
    );
    for name in ["code_symbols", "code_navigation", "code_diagnostics"] {
        assert!(!plain.registry().contains(name));
    }

    let test_provider = TestProvider::new(false);
    let provider: Arc<dyn WorkspaceCodeIntelligence> = test_provider.clone();
    let services = WorkspaceServices::local(temp.path()).with_code_intelligence(provider);
    let enabled = crate::tools::ToolExecutor::new_with_workspace_services(
        temp.path().display().to_string(),
        services,
    );
    for name in ["code_symbols", "code_navigation", "code_diagnostics"] {
        assert!(enabled.registry().contains(name));
        let capabilities = enabled.registry().capabilities(name, &json!({})).unwrap();
        assert!(capabilities.read_only);
        assert!(capabilities.idempotent);
        assert!(capabilities.cancellation_safe);
        assert_eq!(capabilities.output_kind, ToolOutputKind::Structured);
        assert_eq!(
            crate::queue::SessionLane::from_tool_name(name),
            crate::queue::SessionLane::Query
        );
    }
    assert!(enabled
        .registry()
        .validate_arguments(
            "code_symbols",
            &json!({"operation": "outline", "path": "src/lib.rs"}),
        )
        .is_ok());
    assert!(enabled
        .registry()
        .validate_arguments("code_symbols", &json!({"operation": "outline"}))
        .is_err());
    assert!(enabled
        .registry()
        .validate_arguments(
            "code_navigation",
            &json!({
                "operation": "definition",
                "path": "src/lib.rs",
                "line": 0,
                "character": 0
            }),
        )
        .is_ok());
    assert!(enabled
        .registry()
        .validate_arguments("code_diagnostics", &json!({}))
        .is_ok());

    let escaped = enabled
        .execute(
            "code_navigation",
            &json!({
                "operation": "definition",
                "path": "../outside.rs",
                "line": 0,
                "character": 0
            }),
        )
        .await
        .unwrap();
    assert_eq!(escaped.exit_code, 1);
    assert!(escaped.output.contains("Workspace boundary check failed"));
    assert_eq!(test_provider.calls.load(Ordering::Relaxed), 0);
}
