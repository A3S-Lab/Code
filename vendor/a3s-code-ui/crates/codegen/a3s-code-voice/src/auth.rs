//! Bearer resolution for voice STT requests.
//! The voice clients are long-lived: a single voice session opens many STT WebSocket connections over its lifetime.
//! An OAuth/session bearer rotates (~15 min), so capturing a token once at startup would 401 mid-session.
//! Instead of a static `String`, the clients hold a [`SharedVoiceAuth`] and resolve a fresh bearer at the point of each connection.
//!
//! This crate stays dependency-light: it defines its own minimal async trait instead of the shell's `AuthManager` or tools' `ApiKeyProvider`.
//! The pager adapts the shell's refreshing provider onto this trait.

use std::future::{Future, ready};
use std::pin::Pin;
use std::sync::Arc;

#[cfg(feature = "audio")]
use crate::error::VoiceError;

/// Why there is no bearer for `wss://api.a3s/v1/stt`; neither variant is a cue to try another credential.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VoiceAuthError {
    #[error(
        "voice needs an A3S credential for this account: sign in with an A3S login or set A3S_API_KEY"
    )]
    ForeignSession,
    #[error("not signed in — run `a3s login`, set A3S_API_KEY, or set a model api_key/env_key")]
    NotSignedIn,
}

pub trait VoiceAuthProvider: std::fmt::Debug + Send + Sync + 'static {
    fn bearer(&self) -> Pin<Box<dyn Future<Output = Result<String, VoiceAuthError>> + Send + '_>>;
}

/// Shared provider handed to the voice pipeline.
pub type SharedVoiceAuth = Arc<dyn VoiceAuthProvider>;

#[cfg(feature = "audio")]
pub(crate) async fn require_bearer(auth: &SharedVoiceAuth) -> Result<String, VoiceError> {
    auth.bearer()
        .await
        .map_err(|error| VoiceError::Auth(error.to_string()))
}

/// A fixed bearer that never refreshes.
/// Used by the standalone `voice-probe` binary and tests, where there is no `AuthManager`, only a raw `A3S_API_KEY`.
pub struct StaticVoiceAuth(pub String);

impl std::fmt::Debug for StaticVoiceAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("StaticVoiceAuth")
            .field(&"<redacted>")
            .finish()
    }
}

impl VoiceAuthProvider for StaticVoiceAuth {
    fn bearer(&self) -> Pin<Box<dyn Future<Output = Result<String, VoiceAuthError>> + Send + '_>> {
        Box::pin(ready(Ok(self.0.clone())))
    }
}

impl StaticVoiceAuth {
    /// Build a [`SharedVoiceAuth`] from a static key, trimming whitespace and rejecting an empty value.
    pub fn shared(key: impl Into<String>) -> Option<SharedVoiceAuth> {
        let key = key.into().trim().to_string();
        if key.is_empty() {
            return None;
        }
        Some(Arc::new(Self(key)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_provider_resolves() {
        let provider = StaticVoiceAuth::shared("  sk-test  ").unwrap();
        assert_eq!(provider.bearer().await.as_deref(), Ok("sk-test"));
    }

    #[test]
    fn static_provider_rejects_empty() {
        assert!(StaticVoiceAuth::shared("   ").is_none());
    }
}
