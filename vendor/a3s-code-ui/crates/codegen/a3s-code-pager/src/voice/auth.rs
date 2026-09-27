//! Bridge the shell's `AuthManager` onto the voice crate's bearer provider.
//!
//! voice-api accepts an A3S API key or an A3S OAuth2 token at `api.a3s` and attributes per-user billing for OAuth.
//! The bearer comes from the shell's side-call resolver, the same one the Imagine tools use, so a login issued by a
//! foreign authority is refused here and no socket opens for it.
//!
//! Resolved per request: the agent's refreshing manager in direct-spawn mode.
//! In leader mode, a non-refreshing one adopts the agent's rotated `auth.json` token under the file lock (see [`crate::acp`]).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use a3s_code_tools::types::SharedApiKeyProvider;
use a3s_code_tools::types::api_key_provider::SideCallBearerError;
use a3s_code_voice::{SharedVoiceAuth, VoiceAuthError, VoiceAuthProvider};

/// Adapts the shell's `ApiKeyProvider` onto [`VoiceAuthProvider`].
///
/// Resolves a token per request (never a static snapshot), so a long session follows the `AuthManager` instead of pinning a token that 401s.
struct AuthManagerVoiceAuth(SharedApiKeyProvider);

impl std::fmt::Debug for AuthManagerVoiceAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthManagerVoiceAuth")
    }
}

fn voice_auth_error(error: SideCallBearerError) -> VoiceAuthError {
    match error {
        SideCallBearerError::ForeignSession => VoiceAuthError::ForeignSession,
        SideCallBearerError::Missing => VoiceAuthError::NotSignedIn,
    }
}

impl VoiceAuthProvider for AuthManagerVoiceAuth {
    fn bearer(&self) -> Pin<Box<dyn Future<Output = Result<String, VoiceAuthError>> + Send + '_>> {
        let provider = self.0.clone();
        Box::pin(async move { provider.side_call_bearer().await.map_err(voice_auth_error) })
    }
}

/// Build the voice bearer provider from the connection's `AuthManager`.
///
/// Serves A3S logins and `A3S_API_KEY` / per-model BYOK keys. A foreign-issuer login resolves to
/// [`VoiceAuthError::ForeignSession`] instead of a bearer.
pub fn build_voice_auth(auth_manager: Arc<a3s_code_login::AuthManager>) -> SharedVoiceAuth {
    Arc::new(AuthManagerVoiceAuth(
        a3s_code_login::shared_api_key_provider(auth_manager),
    ))
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
