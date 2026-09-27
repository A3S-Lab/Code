//! The bearer for a call that goes straight to an A3S host outside a model turn: the Imagine
//! tools and pager voice dictation.
//!
//! An A3S host accepts two credential kinds and nothing else: an A3S API key, or an A3S OAuth2
//! access token (JWT header `typ` `at+jwt`, issuer `auth.a3s`). The login this manager holds may
//! have been issued by a foreign authority, and such a login is never a side-call bearer. The
//! model turn already refuses it for chat (`WireValidBearerResolver`); this module is the single
//! place that refuses it for everything else, so no tool client and no voice client reads the
//! manager's key on its own.
//!
//! Invariants:
//! - A session whose issuer is a foreign login authority is never returned.
//! - A missing bearer is an error, never a cue to fall back to a different credential.
//! - A configured static key (`A3S_API_KEY`, the process model key, or `a3s::api_key` on disk)
//!   still applies, under the same kill-switch and `preferred_method` rules as chat.
//! - Every other credential goes to the server as before, and the server decides, as it does
//!   today for enterprise IdP sessions and bare external-provider tokens.

use std::sync::Arc;

use a3s_code_tools::types::api_key_provider::SideCallBearerError;

use crate::AuthManager;
use crate::config::PreferredAuthMethod;
use crate::model::{AuthMode, GrokAuth};

impl AuthManager {
    /// The side-call bearer from the cached credential, without a refresh.
    ///
    /// # Errors
    ///
    /// [`SideCallBearerError::ForeignSession`] when the wire-valid credential belongs to another
    /// authority and no static key is configured; [`SideCallBearerError::Missing`] when there is
    /// no wire-valid credential and no static key.
    pub fn side_call_bearer(&self) -> Result<String, SideCallBearerError> {
        if prefers_static_api_key(self) {
            return resolve_static_api_key(self).ok_or(SideCallBearerError::Missing);
        }
        match self.current_wire_valid() {
            Some(auth) if is_a3s_side_call_principal(&auth) => Ok(auth.key),
            Some(_) => resolve_static_api_key(self).ok_or(SideCallBearerError::ForeignSession),
            None => resolve_static_api_key(self).ok_or(SideCallBearerError::Missing),
        }
    }

    /// The side-call bearer for the next request, refreshing an A3S login through the same
    /// chain chat uses. A foreign login is classified before any refresh runs: renewing it
    /// would not change who issued it.
    ///
    /// # Errors
    ///
    /// As [`Self::side_call_bearer`]. A refresh failure falls back to the still wire-valid
    /// cached token, then the static key, then `Missing`.
    pub async fn side_call_bearer_async(self: &Arc<Self>) -> Result<String, SideCallBearerError> {
        if prefers_static_api_key(self) {
            return resolve_static_api_key(self).ok_or(SideCallBearerError::Missing);
        }
        let Some(loaded) = self.current_or_expired() else {
            return resolve_static_api_key(self).ok_or(SideCallBearerError::Missing);
        };
        if !is_a3s_side_call_principal(&loaded) {
            return resolve_static_api_key(self).ok_or(SideCallBearerError::ForeignSession);
        }
        match self.auth().await {
            Ok(auth) if is_a3s_side_call_principal(&auth) => Ok(auth.key),
            // A refresh that swapped in another authority's credential is still not a result
            Ok(_) => resolve_static_api_key(self).ok_or(SideCallBearerError::ForeignSession),
            Err(error) => {
                // The typed error cannot carry `AuthError` across the tools boundary; this log is its only trace
                tracing::warn!(%error, "side-call bearer: refresh failed, serving the cached token if it is still wire-valid");
                self.current_wire_valid()
                    .filter(is_a3s_side_call_principal)
                    .map(|auth| auth.key)
                    .or_else(|| resolve_static_api_key(self))
                    .ok_or(SideCallBearerError::Missing)
            }
        }
    }
}

/// Whether the credential may be sent to an A3S host as this account's own credential.
///
/// An API key always may. A session may unless a foreign login authority issued it. Anything else
/// is sent, as chat sends it, and the server decides. The issuer is checked here rather than
/// through the active backend so that an injected foreign credential is refused on every build.
pub(crate) fn is_a3s_side_call_principal(auth: &GrokAuth) -> bool {
    match auth.auth_mode {
        AuthMode::ApiKey => true,
        AuthMode::Oidc | AuthMode::External | AuthMode::WebLogin => {
            !auth.oidc_issuer.as_deref().is_some_and(is_foreign_issuer)
        }
    }
}

/// Host match, not a prefix match: `cursor.com.evil.test` is another domain.
fn is_foreign_issuer(issuer: &str) -> bool {
    reqwest::Url::parse(issuer).is_ok_and(|url| {
        url.host_str()
            .is_some_and(|host| host == "cursor.com" || host.ends_with(".cursor.com"))
    })
}

/// Bearer for tools and pager voice; static-key rules are on [`resolve_static_api_key`].
pub struct SharedAuthKeyProvider(pub Arc<AuthManager>);

impl a3s_code_tools::types::ApiKeyProvider for SharedAuthKeyProvider {
    fn current_api_key(&self) -> Option<String> {
        if prefers_static_api_key(&self.0) {
            return resolve_static_api_key(&self.0);
        }
        // Hard expiry, not the refresh buffer: sync cannot refresh, so a buffered-but-valid token must still beat static
        self.0
            .current_wire_valid()
            .map(|a| a.key)
            .or_else(|| resolve_static_api_key(&self.0))
            .or_else(|| self.0.current_or_expired().map(|a| a.key))
    }

    fn current_api_key_async(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send + '_>> {
        let am = self.0.clone();
        Box::pin(async move {
            if prefers_static_api_key(&am) {
                return resolve_static_api_key(&am);
            }
            am.get_valid_token()
                .await
                .ok()
                .or_else(|| resolve_static_api_key(&am))
        })
    }

    fn side_call_bearer(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, SideCallBearerError>> + Send + '_>,
    > {
        Box::pin(self.0.side_call_bearer_async())
    }
}

fn prefers_static_api_key(am: &AuthManager) -> bool {
    matches!(
        am.grok_com_config().preferred_method,
        Some(PreferredAuthMethod::ApiKey)
    )
}

/// Precedence: env, then process model key, then disk. Off under kill-switch / oidc pin.
pub(crate) fn resolve_static_api_key(am: &AuthManager) -> Option<String> {
    if am.grok_com_config().api_key_auth_disabled() {
        return None;
    }
    if matches!(
        am.grok_com_config().preferred_method,
        Some(PreferredAuthMethod::Oidc)
    ) {
        return None;
    }
    non_empty_key(crate::auth_method::read_a3s_api_key_env().ok())
        .or_else(|| am.process_static_api_key())
        .or_else(|| am.cached_disk_api_key())
}

pub(crate) fn non_empty_key(key: Option<String>) -> Option<String> {
    key.map(|k| k.trim().to_owned()).filter(|k| !k.is_empty())
}

/// Per-request bearer for out-of-crate consumers (e.g. pager voice).
pub fn shared_api_key_provider(
    auth_manager: Arc<AuthManager>,
) -> a3s_code_tools::types::SharedApiKeyProvider {
    Arc::new(SharedAuthKeyProvider(auth_manager))
}

#[cfg(test)]
#[path = "side_call_bearer_tests.rs"]
mod tests;
