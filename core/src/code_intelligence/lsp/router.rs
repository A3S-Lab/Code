use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use url::Url;

use super::message::{JsonRpcRequest, JsonRpcResponse};

const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// One workspace folder exposed to a language server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceFolder {
    uri: Url,
    name: String,
}

impl WorkspaceFolder {
    pub(crate) fn new(uri: Url, name: impl Into<String>) -> Self {
        Self {
            uri,
            name: name.into(),
        }
    }

    pub(crate) fn uri(&self) -> &Url {
        &self.uri
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    fn to_value(&self) -> Value {
        json!({
            "uri": self.uri.as_str(),
            "name": self.name,
        })
    }
}

/// Immutable workspace-wide settings returned to server configuration queries.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct WorkspaceSettings {
    sections: BTreeMap<String, Value>,
}

impl WorkspaceSettings {
    pub(crate) fn new(sections: BTreeMap<String, Value>) -> Self {
        Self { sections }
    }

    #[cfg(test)]
    pub(crate) fn with_section(mut self, section: impl Into<String>, value: Value) -> Self {
        self.sections.insert(section.into(), value);
        self
    }

    fn resolve(&self, section: Option<&str>) -> Value {
        let Some(section) = section else {
            return Value::Object(
                self.sections
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect::<Map<_, _>>(),
            );
        };

        if let Some(value) = self.sections.get(section) {
            return value.clone();
        }

        // Servers commonly ask for a nested section even when the client was
        // configured with one object at the language root.
        for (index, _) in section.rmatch_indices('.') {
            let prefix = &section[..index];
            let Some(mut value) = self.sections.get(prefix) else {
                continue;
            };
            let mut found = true;
            for part in section[index + 1..].split('.') {
                let Some(next) = value.as_object().and_then(|object| object.get(part)) else {
                    found = false;
                    break;
                };
                value = next;
            }
            if found {
                return value.clone();
            }
        }

        Value::Null
    }
}

/// Static data available to server-initiated requests.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ServerRequestRouterConfig {
    workspace_folders: Vec<WorkspaceFolder>,
    settings: WorkspaceSettings,
}

impl ServerRequestRouterConfig {
    pub(crate) fn new(
        workspace_folders: Vec<WorkspaceFolder>,
        settings: WorkspaceSettings,
    ) -> Self {
        Self {
            workspace_folders,
            settings,
        }
    }
}

/// Handles the small, explicitly safe set of language-server requests.
#[derive(Debug, Clone, Default)]
pub(crate) struct ServerRequestRouter {
    config: ServerRequestRouterConfig,
}

impl ServerRequestRouter {
    pub(crate) fn new(config: ServerRequestRouterConfig) -> Self {
        Self { config }
    }

    pub(crate) fn route(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        match request.method.as_str() {
            "workspace/configuration" => self.configuration(request),
            "workspace/workspaceFolders" => JsonRpcResponse::success(
                request.id.clone(),
                Value::Array(
                    self.config
                        .workspace_folders
                        .iter()
                        .map(WorkspaceFolder::to_value)
                        .collect(),
                ),
            ),
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create" => {
                JsonRpcResponse::success(request.id.clone(), Value::Null)
            }
            "workspace/applyEdit" => JsonRpcResponse::success(
                request.id.clone(),
                json!({
                    "applied": false,
                    "failureReason": "workspace edits must be initiated by the client",
                }),
            ),
            "window/showDocument" => {
                JsonRpcResponse::success(request.id.clone(), json!({"success": false}))
            }
            _ => JsonRpcResponse::error(
                request.id.clone(),
                METHOD_NOT_FOUND,
                format!("method not found: {}", request.method),
                None,
            ),
        }
    }

    fn configuration(&self, request: &JsonRpcRequest) -> JsonRpcResponse {
        let Some(items) = request
            .params
            .as_ref()
            .and_then(Value::as_object)
            .and_then(|params| params.get("items"))
            .and_then(Value::as_array)
        else {
            return JsonRpcResponse::error(
                request.id.clone(),
                INVALID_PARAMS,
                "workspace/configuration requires an items array",
                None,
            );
        };

        let mut values = Vec::with_capacity(items.len());
        for item in items {
            let Some(item) = item.as_object() else {
                return JsonRpcResponse::error(
                    request.id.clone(),
                    INVALID_PARAMS,
                    "workspace/configuration items must be objects",
                    None,
                );
            };
            let section = match item.get("section") {
                None | Some(Value::Null) => None,
                Some(Value::String(section)) => Some(section.as_str()),
                Some(_) => {
                    return JsonRpcResponse::error(
                        request.id.clone(),
                        INVALID_PARAMS,
                        "workspace/configuration section must be a string or null",
                        None,
                    );
                }
            };
            values.push(self.config.settings.resolve(section));
        }

        JsonRpcResponse::success(request.id.clone(), Value::Array(values))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::code_intelligence::lsp::message::JsonRpcResponsePayload;

    fn router() -> ServerRequestRouter {
        let folder =
            WorkspaceFolder::new(Url::parse("file:///workspace/project").unwrap(), "project");
        let settings = WorkspaceSettings::default()
            .with_section("rust", json!({"check": {"command": "clippy"}}));
        ServerRequestRouter::new(ServerRequestRouterConfig::new(vec![folder], settings))
    }

    fn request(method: &str, params: Option<Value>) -> JsonRpcRequest {
        JsonRpcRequest::new("server-request", method, params)
    }

    fn result(response: JsonRpcResponse) -> Value {
        match response.payload {
            JsonRpcResponsePayload::Result(value) => value,
            JsonRpcResponsePayload::Error(error) => panic!("unexpected error: {error:?}"),
        }
    }

    #[test]
    fn returns_typed_workspace_folders_and_settings() {
        assert_eq!(
            result(router().route(&request("workspace/workspaceFolders", None))),
            json!([{"uri": "file:///workspace/project", "name": "project"}])
        );
        assert_eq!(
            result(router().route(&request(
                "workspace/configuration",
                Some(json!({
                    "items": [
                        {"section": "rust"},
                        {"section": "rust.check.command"},
                        {"section": "missing"}
                    ]
                })),
            ))),
            json!([{"check": {"command": "clippy"}}, "clippy", null])
        );
    }

    #[test]
    fn acknowledges_registration_and_progress_requests() {
        for method in [
            "client/registerCapability",
            "client/unregisterCapability",
            "window/workDoneProgress/create",
        ] {
            assert_eq!(result(router().route(&request(method, None))), Value::Null);
        }
    }

    #[test]
    fn refuses_server_initiated_user_actions() {
        assert_eq!(
            result(router().route(&request("workspace/applyEdit", None)))["applied"],
            false
        );
        assert_eq!(
            result(router().route(&request("window/showDocument", None)))["success"],
            false
        );
    }

    #[test]
    fn rejects_unknown_methods_and_invalid_configuration_params() {
        for response in [
            router().route(&request("unknown/method", None)),
            router().route(&request("workspace/configuration", Some(json!({})))),
        ] {
            let JsonRpcResponsePayload::Error(error) = response.payload else {
                panic!("expected error response");
            };
            assert!(matches!(error.code, METHOD_NOT_FOUND | INVALID_PARAMS));
        }
    }
}
