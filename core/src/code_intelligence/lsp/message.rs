use serde_json::{Map, Number, Value};
use std::hash::Hash;

const JSON_RPC_VERSION: &str = "2.0";

/// Identifier carried by a JSON-RPC request or response.
///
/// Although new client requests use monotonically increasing numeric IDs,
/// language servers are allowed to initiate requests with string or null IDs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum RequestId {
    Number(Number),
    String(String),
    Null,
}

impl RequestId {
    pub(crate) fn to_value(&self) -> Value {
        match self {
            Self::Number(value) => Value::Number(value.clone()),
            Self::String(value) => Value::String(value.clone()),
            Self::Null => Value::Null,
        }
    }

    fn from_value(value: &Value) -> Result<Self, MessageError> {
        match value {
            Value::Number(value) => Ok(Self::Number(value.clone())),
            Value::String(value) => Ok(Self::String(value.clone())),
            Value::Null => Ok(Self::Null),
            _ => Err(MessageError::InvalidRequestId),
        }
    }
}

impl From<u64> for RequestId {
    fn from(value: u64) -> Self {
        Self::Number(Number::from(value))
    }
}

impl From<i64> for RequestId {
    fn from(value: i64) -> Self {
        Self::Number(Number::from(value))
    }
}

impl From<String> for RequestId {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for RequestId {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JsonRpcRequest {
    pub(crate) id: RequestId,
    pub(crate) method: String,
    pub(crate) params: Option<Value>,
}

impl JsonRpcRequest {
    pub(crate) fn new(
        id: impl Into<RequestId>,
        method: impl Into<String>,
        params: Option<Value>,
    ) -> Self {
        Self {
            id: id.into(),
            method: method.into(),
            params,
        }
    }

    pub(crate) fn to_value(&self) -> Value {
        message_value(Some((&self.id, &self.method)), None, self.params.as_ref())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JsonRpcNotification {
    pub(crate) method: String,
    pub(crate) params: Option<Value>,
}

impl JsonRpcNotification {
    pub(crate) fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            method: method.into(),
            params,
        }
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert(
            "jsonrpc".to_owned(),
            Value::String(JSON_RPC_VERSION.to_owned()),
        );
        object.insert("method".to_owned(), Value::String(self.method.clone()));
        if let Some(params) = &self.params {
            object.insert("params".to_owned(), params.clone());
        }
        Value::Object(object)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JsonRpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
    pub(crate) data: Option<Value>,
}

impl JsonRpcError {
    pub(crate) fn new(code: i64, message: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            code,
            message: message.into(),
            data,
        }
    }

    fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert("code".to_owned(), Value::Number(Number::from(self.code)));
        object.insert("message".to_owned(), Value::String(self.message.clone()));
        if let Some(data) = &self.data {
            object.insert("data".to_owned(), data.clone());
        }
        Value::Object(object)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JsonRpcResponsePayload {
    Result(Value),
    Error(JsonRpcError),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JsonRpcResponse {
    pub(crate) id: RequestId,
    pub(crate) payload: JsonRpcResponsePayload,
}

impl JsonRpcResponse {
    pub(crate) fn success(id: impl Into<RequestId>, result: Value) -> Self {
        Self {
            id: id.into(),
            payload: JsonRpcResponsePayload::Result(result),
        }
    }

    pub(crate) fn error(
        id: impl Into<RequestId>,
        code: i64,
        message: impl Into<String>,
        data: Option<Value>,
    ) -> Self {
        Self {
            id: id.into(),
            payload: JsonRpcResponsePayload::Error(JsonRpcError::new(code, message, data)),
        }
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut object = Map::new();
        object.insert(
            "jsonrpc".to_owned(),
            Value::String(JSON_RPC_VERSION.to_owned()),
        );
        object.insert("id".to_owned(), self.id.to_value());
        match &self.payload {
            JsonRpcResponsePayload::Result(result) => {
                object.insert("result".to_owned(), result.clone());
            }
            JsonRpcResponsePayload::Error(error) => {
                object.insert("error".to_owned(), error.to_value());
            }
        }
        Value::Object(object)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum IncomingMessage {
    Request(JsonRpcRequest),
    Notification(JsonRpcNotification),
    Response(JsonRpcResponse),
}

impl TryFrom<Value> for IncomingMessage {
    type Error = MessageError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let object = value.as_object().ok_or(MessageError::ExpectedObject)?;
        validate_version(object)?;

        let has_id = object.contains_key("id");
        let has_method = object.contains_key("method");
        let has_result = object.contains_key("result");
        let has_error = object.contains_key("error");

        if has_method {
            if has_result || has_error {
                return Err(MessageError::AmbiguousShape);
            }

            let method = object
                .get("method")
                .and_then(Value::as_str)
                .ok_or(MessageError::InvalidMethod)?
                .to_owned();
            let params = parse_params(object)?;

            if has_id {
                let id = RequestId::from_value(&object["id"])?;
                Ok(Self::Request(JsonRpcRequest { id, method, params }))
            } else {
                Ok(Self::Notification(JsonRpcNotification { method, params }))
            }
        } else {
            if object.contains_key("params") || !has_id || has_result == has_error {
                return Err(MessageError::InvalidResponseShape);
            }

            let id = RequestId::from_value(&object["id"])?;
            let payload = if has_result {
                JsonRpcResponsePayload::Result(object["result"].clone())
            } else {
                JsonRpcResponsePayload::Error(parse_error(&object["error"])?)
            };
            Ok(Self::Response(JsonRpcResponse { id, payload }))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum MessageError {
    #[error("JSON-RPC message must be an object")]
    ExpectedObject,

    #[error("JSON-RPC message must declare jsonrpc=\"2.0\"")]
    InvalidVersion,

    #[error("JSON-RPC request ID must be a number, string, or null")]
    InvalidRequestId,

    #[error("JSON-RPC method must be a string")]
    InvalidMethod,

    #[error("JSON-RPC params must be an object, array, or null")]
    InvalidParams,

    #[error("JSON-RPC message mixes request and response fields")]
    AmbiguousShape,

    #[error("JSON-RPC response must contain an ID and exactly one of result or error")]
    InvalidResponseShape,

    #[error("JSON-RPC error must contain an integer code and string message")]
    InvalidError,
}

fn validate_version(object: &Map<String, Value>) -> Result<(), MessageError> {
    match object.get("jsonrpc") {
        Some(Value::String(version)) if version == JSON_RPC_VERSION => Ok(()),
        _ => Err(MessageError::InvalidVersion),
    }
}

fn parse_params(object: &Map<String, Value>) -> Result<Option<Value>, MessageError> {
    let Some(params) = object.get("params") else {
        return Ok(None);
    };
    if params.is_null() || params.is_array() || params.is_object() {
        Ok(Some(params.clone()))
    } else {
        Err(MessageError::InvalidParams)
    }
}

fn parse_error(value: &Value) -> Result<JsonRpcError, MessageError> {
    let object = value.as_object().ok_or(MessageError::InvalidError)?;
    let code = object
        .get("code")
        .and_then(Value::as_i64)
        .ok_or(MessageError::InvalidError)?;
    let message = object
        .get("message")
        .and_then(Value::as_str)
        .ok_or(MessageError::InvalidError)?
        .to_owned();
    Ok(JsonRpcError {
        code,
        message,
        data: object.get("data").cloned(),
    })
}

fn message_value(
    request: Option<(&RequestId, &str)>,
    notification_method: Option<&str>,
    params: Option<&Value>,
) -> Value {
    let mut object = Map::new();
    object.insert(
        "jsonrpc".to_owned(),
        Value::String(JSON_RPC_VERSION.to_owned()),
    );
    if let Some((id, method)) = request {
        object.insert("id".to_owned(), id.to_value());
        object.insert("method".to_owned(), Value::String(method.to_owned()));
    } else if let Some(method) = notification_method {
        object.insert("method".to_owned(), Value::String(method.to_owned()));
    }
    if let Some(params) = params {
        object.insert("params".to_owned(), params.clone());
    }
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_all_request_id_kinds() {
        for (id, expected) in [
            (json!(42), RequestId::from(42_u64)),
            (json!("server-1"), RequestId::from("server-1")),
            (Value::Null, RequestId::Null),
        ] {
            let message = IncomingMessage::try_from(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "server/request",
                "params": null
            }))
            .unwrap();
            let IncomingMessage::Request(request) = message else {
                panic!("expected request");
            };
            assert_eq!(request.id, expected);
        }
    }

    #[test]
    fn parses_result_error_and_notification() {
        assert!(matches!(
            IncomingMessage::try_from(json!({
                "jsonrpc": "2.0",
                "id": 7,
                "result": null
            })),
            Ok(IncomingMessage::Response(JsonRpcResponse {
                payload: JsonRpcResponsePayload::Result(Value::Null),
                ..
            }))
        ));
        assert!(matches!(
            IncomingMessage::try_from(json!({
                "jsonrpc": "2.0",
                "id": "seven",
                "error": {"code": -32000, "message": "failed", "data": {"retry": false}}
            })),
            Ok(IncomingMessage::Response(JsonRpcResponse {
                payload: JsonRpcResponsePayload::Error(JsonRpcError { code: -32000, .. }),
                ..
            }))
        ));
        assert!(matches!(
            IncomingMessage::try_from(json!({
                "jsonrpc": "2.0",
                "method": "textDocument/publishDiagnostics",
                "params": {"diagnostics": []}
            })),
            Ok(IncomingMessage::Notification(_))
        ));
    }

    #[test]
    fn rejects_invalid_versions_and_ambiguous_shapes() {
        for value in [
            json!({"id": 1, "result": null}),
            json!({"jsonrpc": "1.0", "id": 1, "result": null}),
            json!({"jsonrpc": "2.0", "id": 1, "result": null, "error": null}),
            json!({"jsonrpc": "2.0", "id": 1, "method": "x", "result": null}),
            json!({"jsonrpc": "2.0", "method": "x", "params": "scalar"}),
        ] {
            assert!(IncomingMessage::try_from(value).is_err());
        }
    }

    #[test]
    fn constructors_emit_unambiguous_messages() {
        assert_eq!(
            JsonRpcRequest::new(3_u64, "initialize", Some(json!({}))).to_value(),
            json!({"jsonrpc": "2.0", "id": 3, "method": "initialize", "params": {}})
        );
        assert_eq!(
            JsonRpcNotification::new("initialized", None).to_value(),
            json!({"jsonrpc": "2.0", "method": "initialized"})
        );
        assert_eq!(
            JsonRpcResponse::error(RequestId::Null, -32601, "not found", None).to_value(),
            json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32601, "message": "not found"}})
        );
    }
}
