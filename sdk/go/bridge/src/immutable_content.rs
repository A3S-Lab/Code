//! Go callback-backed immutable content adapter (SDK-IMM1).

use super::*;
use a3s_code_core::{
    ImmutableContentAdapter, ImmutableContentAdapterBindingV1, ImmutableContentAdapterSession,
    ImmutableContentError, ImmutableContentReferenceV1, ImmutableContentResult,
    ImmutableContentWriteRequestV1, SdkImmutableContentWriteRequestV1,
};
use async_trait::async_trait;

const DEFAULT_IMMUTABLE_CONTENT_TIMEOUT_MS: u64 = 30_000;
const MAX_IMMUTABLE_CONTENT_TIMEOUT_MS: u64 = 300_000;

#[derive(Debug, Deserialize)]
pub(super) struct BridgeImmutableContentAdapterOptions {
    handler_id: String,
    authority_digest: String,
    maximum_bytes: u64,
    adapter_name: String,
    #[serde(default = "default_immutable_content_timeout_ms")]
    timeout_ms: u64,
}

impl BridgeImmutableContentAdapterOptions {
    pub(super) fn into_core(
        self,
        client: Arc<CallbackClient>,
    ) -> Result<ImmutableContentAdapterSession, BridgeFailure> {
        if self.handler_id.trim().is_empty() {
            return Err(invalid_immutable_content("handler_id must not be empty"));
        }
        if self.adapter_name.trim().is_empty() {
            return Err(invalid_immutable_content("adapter_name must not be empty"));
        }
        if self.timeout_ms == 0 || self.timeout_ms > MAX_IMMUTABLE_CONTENT_TIMEOUT_MS {
            return Err(invalid_immutable_content(format!(
                "timeout_ms must be from 1 to {MAX_IMMUTABLE_CONTENT_TIMEOUT_MS}"
            )));
        }
        let binding =
            ImmutableContentAdapterBindingV1::new(self.authority_digest, self.maximum_bytes)
                .map_err(|error| invalid_immutable_content(error.to_string()))?;
        let adapter: Arc<dyn ImmutableContentAdapter> = Arc::new(BridgeImmutableContentAdapter {
            client,
            handler_id: self.handler_id,
            name: self.adapter_name,
            timeout_ms: self.timeout_ms,
        });
        ImmutableContentAdapterSession::new(binding, adapter)
            .map_err(|error| invalid_immutable_content(error.to_string()))
    }
}

fn invalid_immutable_content(message: impl Into<String>) -> BridgeFailure {
    BridgeFailure::new(
        "INVALID_REQUEST",
        format!("immutable_content_adapter: {}", message.into()),
    )
}

struct BridgeImmutableContentAdapter {
    client: Arc<CallbackClient>,
    handler_id: String,
    name: String,
    timeout_ms: u64,
}

#[async_trait]
impl ImmutableContentAdapter for BridgeImmutableContentAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    async fn put(
        &self,
        request: &ImmutableContentWriteRequestV1<'_>,
    ) -> ImmutableContentResult<ImmutableContentReferenceV1> {
        let wire = SdkImmutableContentWriteRequestV1::from_request(request);
        let payload = serde_json::to_value(&wire).map_err(|error| {
            ImmutableContentError::Provider(format!(
                "failed to serialize immutable content write request: {error}"
            ))
        })?;
        let value = self
            .client
            .invoke(&self.handler_id, "put", payload, self.timeout_ms)
            .await
            .map_err(map_immutable_content_callback_failure)?;
        serde_json::from_value(value).map_err(|error| {
            ImmutableContentError::Provider(format!(
                "immutable content put reply is not a valid reference: {error}"
            ))
        })
    }
}

fn map_immutable_content_callback_failure(error: BridgeFailure) -> ImmutableContentError {
    match error.code.as_str() {
        "CALLBACK_TIMEOUT" => {
            ImmutableContentError::Provider("immutable content put timed out".into())
        }
        "BRIDGE_CLOSED" => {
            ImmutableContentError::Provider("immutable content callback transport closed".into())
        }
        _ => ImmutableContentError::Provider(error.message),
    }
}

const fn default_immutable_content_timeout_ms() -> u64 {
    DEFAULT_IMMUTABLE_CONTENT_TIMEOUT_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_handler_id() {
        let options = BridgeImmutableContentAdapterOptions {
            handler_id: " ".to_owned(),
            authority_digest: format!("sha256:{}", "a".repeat(64)),
            maximum_bytes: 1024,
            adapter_name: "go-fixture".to_owned(),
            timeout_ms: 1_000,
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let client = Arc::new(CallbackClient::new(tx));
        assert!(options.into_core(client).is_err());
    }
}
