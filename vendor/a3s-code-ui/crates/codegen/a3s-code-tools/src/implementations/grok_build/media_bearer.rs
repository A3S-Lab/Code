//! The one bearer decision the Imagine image and video clients share.
//!
//! Direct calls to `api.a3s` carry exactly one of: the per-request bearer an `ApiKeyProvider`
//! resolves, or the static key the host configured. A provider outranks the static key, its
//! typed refusal is final, and with neither source the request is refused rather than sent
//! unauthenticated. Nothing is baked into a client's default headers, so a host cannot leak a
//! token by configuring the client with the wrong one.

use a3s_tool_runtime::{ToolError, ToolErrorKind};

use crate::types::SharedApiKeyProvider;
use crate::types::api_key_provider::SideCallBearerError;

/// `details.code` on the tool error returned when no A3S bearer exists for a media request.
pub const SIDE_CALL_BEARER_ERROR_CODE: &str = "side_call_bearer";

fn side_call_bearer_error(error: SideCallBearerError) -> ToolError {
    ToolError::new(ToolErrorKind::Unauthorized, error.to_string())
        .with_details(serde_json::json!({ "code": SIDE_CALL_BEARER_ERROR_CODE }))
}

/// Where a media client's bearer comes from. Built once per client, read per request.
#[derive(Clone)]
pub(crate) struct MediaBearer {
    provider: Option<SharedApiKeyProvider>,
    static_key: Option<String>,
}

impl MediaBearer {
    pub(crate) fn new(provider: Option<SharedApiKeyProvider>, static_key: Option<String>) -> Self {
        Self {
            provider,
            static_key,
        }
    }

    /// The bearer for the next request. The provider decides when one is wired; its refusal is
    /// not softened by a static key here, because the provider already applied the static-key rules.
    pub(crate) async fn resolve(&self) -> Result<String, ToolError> {
        match &self.provider {
            Some(provider) => provider
                .side_call_bearer()
                .await
                .map_err(side_call_bearer_error),
            None => self
                .static_key
                .clone()
                .ok_or_else(|| side_call_bearer_error(SideCallBearerError::Missing)),
        }
    }
}
