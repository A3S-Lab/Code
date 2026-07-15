use std::{str::FromStr, time::Duration};

use lsp_types::{
    ClientCapabilities, ClientInfo, DeclarationCapability, DiagnosticClientCapabilities,
    DocumentSymbolClientCapabilities, DynamicRegistrationClientCapabilities,
    GeneralClientCapabilities, GotoCapability, ImplementationProviderCapability, InitializeParams,
    InitializeResult, InitializedParams, OneOf, PositionEncodingKind,
    PublishDiagnosticsClientCapabilities, ServerCapabilities, ServerInfo,
    TextDocumentClientCapabilities, TextDocumentSyncCapability, TextDocumentSyncClientCapabilities,
    TextDocumentSyncKind, TextDocumentSyncSaveOptions, Uri, WorkDoneProgressParams,
    WorkspaceClientCapabilities, WorkspaceFolder as LspWorkspaceFolder,
    WorkspaceSymbolClientCapabilities,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{
    client::{LspClient, LspClientError},
    router::WorkspaceFolder,
};
use crate::code_intelligence::CodeIntelligenceCapabilities;

const INITIALIZE_METHOD: &str = "initialize";
const INITIALIZED_METHOD: &str = "initialized";

/// Immutable data advertised while starting one language server.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InitializeConfig {
    canonical_root: Url,
    workspace_folders: Vec<WorkspaceFolder>,
    initialization_options: Option<Value>,
    client_name: String,
    client_version: String,
}

impl InitializeConfig {
    pub(crate) fn new(
        canonical_root: Url,
        workspace_folders: Vec<WorkspaceFolder>,
        initialization_options: Option<Value>,
        client_name: impl Into<String>,
        client_version: impl Into<String>,
    ) -> Self {
        Self {
            canonical_root,
            workspace_folders,
            initialization_options,
            client_name: client_name.into(),
            client_version: client_version.into(),
        }
    }
}

/// Read-only capabilities negotiated with an initialized language server.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InitializedServer {
    pub(crate) capabilities: CodeIntelligenceCapabilities,
    pub(crate) supports_pull_diagnostics: bool,
    pub(crate) supports_publish_diagnostics: bool,
    pub(crate) text_sync_mode: ServerTextSyncMode,
    pub(crate) supports_open_close: bool,
    pub(crate) supports_did_save: bool,
    pub(crate) server_info: Option<ServerInfo>,
}

/// Text synchronization mode selected by a language server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerTextSyncMode {
    None,
    Full,
    /// Saved-only callers must close and reopen a changed document instead of
    /// sending a full-content change under an incremental contract.
    Incremental,
}

/// Complete the protocol handshake for one language-server connection.
///
/// The `initialized` notification is emitted only after the server has
/// returned and the client has validated a successful `initialize` result.
pub(crate) async fn initialize(
    client: &LspClient,
    config: &InitializeConfig,
    cancellation: CancellationToken,
    timeout: Duration,
) -> Result<InitializedServer, LspClientError> {
    let params = initialize_params(config)?;
    let params = serde_json::to_value(params).map_err(|error| LspClientError::Protocol {
        message: format!("failed to encode initialize parameters: {error}"),
    })?;
    let response = client
        .request(INITIALIZE_METHOD, Some(params), cancellation, timeout)
        .await?;
    let result: InitializeResult =
        serde_json::from_value(response).map_err(|error| LspClientError::Protocol {
            message: format!("invalid initialize result: {error}"),
        })?;

    let initialized = normalized_server(result)?;
    let params =
        serde_json::to_value(InitializedParams {}).map_err(|error| LspClientError::Protocol {
            message: format!("failed to encode initialized parameters: {error}"),
        })?;
    client.notify(INITIALIZED_METHOD, Some(params)).await?;

    Ok(initialized)
}

#[allow(deprecated)]
fn initialize_params(config: &InitializeConfig) -> Result<InitializeParams, LspClientError> {
    let root_uri = protocol_uri(&config.canonical_root)?;
    let workspace_folders = config
        .workspace_folders
        .iter()
        .map(|folder| {
            Ok(LspWorkspaceFolder {
                uri: protocol_uri(folder.uri())?,
                name: folder.name().to_owned(),
            })
        })
        .collect::<Result<Vec<_>, LspClientError>>()?;

    Ok(InitializeParams {
        process_id: Some(std::process::id()),
        root_path: None,
        root_uri: Some(root_uri),
        initialization_options: config.initialization_options.clone(),
        capabilities: client_capabilities(),
        trace: None,
        workspace_folders: Some(workspace_folders),
        client_info: Some(ClientInfo {
            name: config.client_name.clone(),
            version: Some(config.client_version.clone()),
        }),
        locale: None,
        work_done_progress_params: WorkDoneProgressParams::default(),
    })
}

fn protocol_uri(uri: &Url) -> Result<Uri, LspClientError> {
    Uri::from_str(uri.as_str()).map_err(|error| LspClientError::Protocol {
        message: format!("invalid language-server URI '{}': {error}", uri.as_str()),
    })
}

fn client_capabilities() -> ClientCapabilities {
    let goto = || GotoCapability {
        dynamic_registration: Some(false),
        link_support: Some(true),
    };

    ClientCapabilities {
        workspace: Some(WorkspaceClientCapabilities {
            apply_edit: Some(false),
            workspace_edit: None,
            symbol: Some(WorkspaceSymbolClientCapabilities {
                dynamic_registration: Some(false),
                ..WorkspaceSymbolClientCapabilities::default()
            }),
            workspace_folders: Some(true),
            configuration: Some(true),
            ..WorkspaceClientCapabilities::default()
        }),
        text_document: Some(TextDocumentClientCapabilities {
            synchronization: Some(TextDocumentSyncClientCapabilities {
                dynamic_registration: Some(false),
                will_save: Some(false),
                will_save_wait_until: Some(false),
                did_save: Some(true),
            }),
            references: Some(DynamicRegistrationClientCapabilities {
                dynamic_registration: Some(false),
            }),
            document_symbol: Some(DocumentSymbolClientCapabilities {
                dynamic_registration: Some(false),
                hierarchical_document_symbol_support: Some(true),
                ..DocumentSymbolClientCapabilities::default()
            }),
            declaration: Some(goto()),
            definition: Some(goto()),
            implementation: Some(goto()),
            rename: None,
            publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                related_information: Some(true),
                version_support: Some(true),
                code_description_support: Some(true),
                data_support: Some(false),
                ..PublishDiagnosticsClientCapabilities::default()
            }),
            diagnostic: Some(DiagnosticClientCapabilities {
                dynamic_registration: Some(false),
                related_document_support: Some(false),
            }),
            ..TextDocumentClientCapabilities::default()
        }),
        general: Some(GeneralClientCapabilities {
            position_encodings: Some(vec![PositionEncodingKind::UTF16]),
            ..GeneralClientCapabilities::default()
        }),
        ..ClientCapabilities::default()
    }
}

fn normalized_server(result: InitializeResult) -> Result<InitializedServer, LspClientError> {
    validate_position_encoding(&result.capabilities)?;
    let (text_sync_mode, supports_open_close, supports_did_save) =
        normalize_text_sync(result.capabilities.text_document_sync.as_ref())?;
    let supports_pull_diagnostics = result.capabilities.diagnostic_provider.is_some();
    let supports_publish_diagnostics = true;
    let capabilities = normalize_capabilities(&result.capabilities, supports_publish_diagnostics);

    Ok(InitializedServer {
        capabilities,
        supports_pull_diagnostics,
        supports_publish_diagnostics,
        text_sync_mode,
        supports_open_close,
        supports_did_save,
        server_info: result.server_info,
    })
}

fn validate_position_encoding(server: &ServerCapabilities) -> Result<(), LspClientError> {
    let Some(encoding) = server.position_encoding.as_ref() else {
        // UTF-16 is the protocol default when the server omits this field.
        return Ok(());
    };
    if encoding == &PositionEncodingKind::UTF16 {
        return Ok(());
    }

    Err(LspClientError::Protocol {
        message: format!(
            "language server selected unsupported position encoding '{}'; UTF-16 is required",
            encoding.as_str()
        ),
    })
}

fn normalize_text_sync(
    capability: Option<&TextDocumentSyncCapability>,
) -> Result<(ServerTextSyncMode, bool, bool), LspClientError> {
    match capability {
        None | Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::NONE)) => {
            Ok((ServerTextSyncMode::None, false, false))
        }
        Some(TextDocumentSyncCapability::Kind(kind)) => {
            let mode = text_sync_mode(*kind)?;
            Ok((mode, true, false))
        }
        Some(TextDocumentSyncCapability::Options(options)) => {
            let mode = match options.change {
                Some(kind) => text_sync_mode(kind)?,
                None => ServerTextSyncMode::None,
            };
            let supports_did_save = match options.save.as_ref() {
                Some(TextDocumentSyncSaveOptions::Supported(supported)) => *supported,
                Some(TextDocumentSyncSaveOptions::SaveOptions(_)) => true,
                None => false,
            };
            Ok((mode, options.open_close.unwrap_or(false), supports_did_save))
        }
    }
}

fn text_sync_mode(kind: TextDocumentSyncKind) -> Result<ServerTextSyncMode, LspClientError> {
    if kind == TextDocumentSyncKind::NONE {
        Ok(ServerTextSyncMode::None)
    } else if kind == TextDocumentSyncKind::FULL {
        Ok(ServerTextSyncMode::Full)
    } else if kind == TextDocumentSyncKind::INCREMENTAL {
        Ok(ServerTextSyncMode::Incremental)
    } else {
        Err(LspClientError::Protocol {
            message: format!("language server selected unsupported text sync kind {kind:?}"),
        })
    }
}

fn normalize_capabilities(
    server: &ServerCapabilities,
    supports_publish_diagnostics: bool,
) -> CodeIntelligenceCapabilities {
    let supports_pull_diagnostics = server.diagnostic_provider.is_some();
    CodeIntelligenceCapabilities {
        document_symbols: one_of_enabled(server.document_symbol_provider.as_ref()),
        workspace_symbols: one_of_enabled(server.workspace_symbol_provider.as_ref()),
        definition: one_of_enabled(server.definition_provider.as_ref()),
        declaration: declaration_enabled(server.declaration_provider.as_ref()),
        references: one_of_enabled(server.references_provider.as_ref()),
        implementations: implementation_enabled(server.implementation_provider.as_ref()),
        diagnostics: supports_pull_diagnostics || supports_publish_diagnostics,
    }
}

fn one_of_enabled<T>(capability: Option<&OneOf<bool, T>>) -> bool {
    match capability {
        Some(OneOf::Left(enabled)) => *enabled,
        Some(OneOf::Right(_)) => true,
        None => false,
    }
}

fn declaration_enabled(capability: Option<&DeclarationCapability>) -> bool {
    match capability {
        Some(DeclarationCapability::Simple(enabled)) => *enabled,
        Some(DeclarationCapability::RegistrationOptions(_) | DeclarationCapability::Options(_)) => {
            true
        }
        None => false,
    }
}

fn implementation_enabled(capability: Option<&ImplementationProviderCapability>) -> bool {
    match capability {
        Some(ImplementationProviderCapability::Simple(enabled)) => *enabled,
        Some(ImplementationProviderCapability::Options(_)) => true,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio::{io::DuplexStream, time};
    use tokio_util::codec::Framed;

    use super::*;
    use crate::code_intelligence::lsp::{
        codec::LspCodec,
        message::{IncomingMessage, JsonRpcResponse},
        router::{ServerRequestRouter, ServerRequestRouterConfig},
    };

    fn client_and_server() -> (LspClient, Framed<DuplexStream, LspCodec>) {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let client = LspClient::start(
            client_io,
            ServerRequestRouter::new(ServerRequestRouterConfig::default()),
        );
        (client, Framed::new(server_io, LspCodec::default()))
    }

    fn config() -> InitializeConfig {
        InitializeConfig::new(
            Url::parse("file:///workspace").unwrap(),
            vec![
                WorkspaceFolder::new(Url::parse("file:///workspace/core").unwrap(), "core"),
                WorkspaceFolder::new(Url::parse("file:///workspace/web").unwrap(), "web"),
            ],
            Some(json!({"check": {"command": "clippy"}})),
            "a3s-code",
            "5.2.4",
        )
    }

    async fn next_server_message(server: &mut Framed<DuplexStream, LspCodec>) -> IncomingMessage {
        let value = time::timeout(Duration::from_secs(1), server.next())
            .await
            .expect("server message timed out")
            .expect("client stream closed")
            .expect("client frame failed");
        IncomingMessage::try_from(value).expect("client sent invalid message")
    }

    async fn assert_no_server_message(server: &mut Framed<DuplexStream, LspCodec>) {
        assert!(
            time::timeout(Duration::from_millis(25), server.next())
                .await
                .is_err(),
            "client sent an unexpected protocol message"
        );
    }

    #[tokio::test]
    async fn initializes_in_order_and_normalizes_read_only_capabilities() {
        let (client, mut server) = client_and_server();
        let task = tokio::spawn({
            let client = client.clone();
            async move {
                initialize(
                    &client,
                    &config(),
                    CancellationToken::new(),
                    Duration::from_secs(1),
                )
                .await
            }
        });

        let IncomingMessage::Request(request) = next_server_message(&mut server).await else {
            panic!("expected initialize request");
        };
        assert_eq!(request.method, INITIALIZE_METHOD);
        let params = request.params.as_ref().expect("initialize params");
        assert_eq!(params["rootUri"], "file:///workspace");
        assert_eq!(
            params["workspaceFolders"],
            json!([
                {"uri": "file:///workspace/core", "name": "core"},
                {"uri": "file:///workspace/web", "name": "web"}
            ])
        );
        assert_eq!(
            params["initializationOptions"],
            json!({"check": {"command": "clippy"}})
        );
        assert_eq!(
            params["clientInfo"],
            json!({"name": "a3s-code", "version": "5.2.4"})
        );
        assert!(params["processId"].is_number());
        assert_eq!(params["capabilities"]["workspace"]["applyEdit"], false);
        assert!(params["capabilities"]["workspace"]
            .get("workspaceEdit")
            .is_none());
        assert!(params["capabilities"]["textDocument"]
            .get("rename")
            .is_none());
        assert_eq!(
            params["capabilities"]["textDocument"]["documentSymbol"]
                ["hierarchicalDocumentSymbolSupport"],
            true
        );
        for capability in ["declaration", "definition", "references", "implementation"] {
            assert!(params["capabilities"]["textDocument"]
                .get(capability)
                .is_some());
        }
        assert!(params["capabilities"]["workspace"].get("symbol").is_some());
        assert!(params["capabilities"]["textDocument"]
            .get("publishDiagnostics")
            .is_some());
        assert!(params["capabilities"]["textDocument"]
            .get("diagnostic")
            .is_some());
        assert_eq!(
            params["capabilities"]["general"]["positionEncodings"],
            json!(["utf-16"])
        );

        assert_no_server_message(&mut server).await;
        server
            .send(
                JsonRpcResponse::success(
                    request.id,
                    json!({
                        "capabilities": {
                            "positionEncoding": "utf-16",
                            "textDocumentSync": {
                                "openClose": true,
                                "change": 2,
                                "save": {"includeText": true}
                            },
                            "documentSymbolProvider": {"label": "outline"},
                            "workspaceSymbolProvider": true,
                            "definitionProvider": false,
                            "declarationProvider": {},
                            "referencesProvider": {},
                            "implementationProvider": {"documentSelector": null},
                            "diagnosticProvider": {
                                "identifier": "saved",
                                "interFileDependencies": true,
                                "workspaceDiagnostics": true
                            },
                            "renameProvider": true
                        },
                        "serverInfo": {"name": "test-server", "version": "1.2.3"}
                    }),
                )
                .to_value(),
            )
            .await
            .unwrap();

        let IncomingMessage::Notification(notification) = next_server_message(&mut server).await
        else {
            panic!("expected initialized notification");
        };
        assert_eq!(notification.method, INITIALIZED_METHOD);
        assert_eq!(notification.params, Some(json!({})));

        let initialized = task.await.unwrap().unwrap();
        assert_eq!(
            initialized.capabilities,
            CodeIntelligenceCapabilities {
                document_symbols: true,
                workspace_symbols: true,
                definition: false,
                declaration: true,
                references: true,
                implementations: true,
                diagnostics: true,
            }
        );
        assert!(initialized.supports_pull_diagnostics);
        assert!(initialized.supports_publish_diagnostics);
        assert_eq!(initialized.text_sync_mode, ServerTextSyncMode::Incremental);
        assert!(initialized.supports_open_close);
        assert!(initialized.supports_did_save);
        assert_eq!(
            initialized.server_info,
            Some(ServerInfo {
                name: "test-server".to_owned(),
                version: Some("1.2.3".to_owned()),
            })
        );
        client.close().await;
    }

    #[tokio::test]
    async fn publish_diagnostics_remain_available_without_pull_support() {
        let (client, mut server) = client_and_server();
        let task = tokio::spawn({
            let client = client.clone();
            async move {
                initialize(
                    &client,
                    &config(),
                    CancellationToken::new(),
                    Duration::from_secs(1),
                )
                .await
            }
        });
        let IncomingMessage::Request(request) = next_server_message(&mut server).await else {
            panic!("expected initialize request");
        };
        server
            .send(JsonRpcResponse::success(request.id, json!({"capabilities": {}})).to_value())
            .await
            .unwrap();
        let _ = next_server_message(&mut server).await;

        let initialized = task.await.unwrap().unwrap();
        assert!(!initialized.supports_pull_diagnostics);
        assert!(initialized.supports_publish_diagnostics);
        assert!(initialized.capabilities.diagnostics);
        assert_eq!(initialized.text_sync_mode, ServerTextSyncMode::None);
        assert!(!initialized.supports_open_close);
        assert!(!initialized.supports_did_save);
        client.close().await;
    }

    #[test]
    fn normalizes_legacy_and_options_text_sync_capabilities() {
        fn sync(value: Value) -> (ServerTextSyncMode, bool, bool) {
            let capability = serde_json::from_value(value).unwrap();
            normalize_text_sync(Some(&capability)).unwrap()
        }

        assert_eq!(sync(json!(0)), (ServerTextSyncMode::None, false, false));
        assert_eq!(sync(json!(1)), (ServerTextSyncMode::Full, true, false));
        assert_eq!(
            sync(json!(2)),
            (ServerTextSyncMode::Incremental, true, false)
        );
        assert_eq!(
            sync(json!({"openClose": false, "change": 1, "save": true})),
            (ServerTextSyncMode::Full, false, true)
        );
        assert_eq!(
            sync(json!({"openClose": true, "change": 0, "save": false})),
            (ServerTextSyncMode::None, true, false)
        );
        assert_eq!(
            sync(json!({"save": {"includeText": false}})),
            (ServerTextSyncMode::None, false, true)
        );
    }

    #[tokio::test]
    async fn rejects_non_utf16_server_encoding_before_initialized_notification() {
        let (client, mut server) = client_and_server();
        let task = tokio::spawn({
            let client = client.clone();
            async move {
                initialize(
                    &client,
                    &config(),
                    CancellationToken::new(),
                    Duration::from_secs(1),
                )
                .await
            }
        });
        let IncomingMessage::Request(request) = next_server_message(&mut server).await else {
            panic!("expected initialize request");
        };
        server
            .send(
                JsonRpcResponse::success(
                    request.id,
                    json!({"capabilities": {"positionEncoding": "utf-8"}}),
                )
                .to_value(),
            )
            .await
            .unwrap();

        let error = task.await.unwrap().unwrap_err();
        assert!(matches!(error, LspClientError::Protocol { .. }));
        assert!(error.to_string().contains("utf-8"));
        assert!(error.to_string().contains("UTF-16"));
        assert_no_server_message(&mut server).await;
        client.close().await;
    }

    #[tokio::test]
    async fn remote_error_does_not_send_initialized_notification() {
        let (client, mut server) = client_and_server();
        let task = tokio::spawn({
            let client = client.clone();
            async move {
                initialize(
                    &client,
                    &config(),
                    CancellationToken::new(),
                    Duration::from_secs(1),
                )
                .await
            }
        });
        let IncomingMessage::Request(request) = next_server_message(&mut server).await else {
            panic!("expected initialize request");
        };
        server
            .send(
                JsonRpcResponse::error(
                    request.id,
                    -32002,
                    "server is not ready",
                    Some(json!({"retry": false})),
                )
                .to_value(),
            )
            .await
            .unwrap();

        assert!(matches!(
            task.await.unwrap(),
            Err(LspClientError::RemoteError { code: -32002, .. })
        ));
        assert_no_server_message(&mut server).await;
        client.close().await;
    }

    #[tokio::test]
    async fn cancellation_aborts_initialize_without_initialized_notification() {
        let (client, mut server) = client_and_server();
        let cancellation = CancellationToken::new();
        let task = tokio::spawn({
            let client = client.clone();
            let cancellation = cancellation.clone();
            async move { initialize(&client, &config(), cancellation, Duration::from_secs(1)).await }
        });
        let IncomingMessage::Request(request) = next_server_message(&mut server).await else {
            panic!("expected initialize request");
        };
        cancellation.cancel();

        let IncomingMessage::Notification(notification) = next_server_message(&mut server).await
        else {
            panic!("expected cancellation notification");
        };
        assert_eq!(notification.method, "$/cancelRequest");
        assert_eq!(notification.params.unwrap()["id"], request.id.to_value());
        assert_eq!(task.await.unwrap(), Err(LspClientError::Cancelled));
        assert_no_server_message(&mut server).await;
        client.close().await;
    }
}
