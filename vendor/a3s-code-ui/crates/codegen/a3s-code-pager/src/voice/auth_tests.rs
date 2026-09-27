use std::sync::Arc;

use a3s_code_login::{AuthManager, AuthMode, GrokAuth, GrokComConfig};
use a3s_code_test_support::EnvGuard;
use a3s_code_voice::VoiceAuthError;
use chrono::{Duration, Utc};
use pretty_assertions::assert_eq;

use super::build_voice_auth;

fn session(issuer: &str) -> GrokAuth {
    GrokAuth {
        key: "session-token".to_owned(),
        auth_mode: AuthMode::Oidc,
        oidc_issuer: Some(issuer.to_owned()),
        refresh_token: Some("rt".to_owned()),
        expires_at: Some(Utc::now() + Duration::hours(1)),
        ..GrokAuth::test_default()
    }
}

/// The positive case is a static key, which every build of the manager serves; the A3S-session case is pinned in
/// `a3s-code-login`.
#[tokio::test]
#[serial_test::serial]
async fn foreign_session_is_refused_and_a3s_credential_is_served() {
    let _a3s = EnvGuard::unset("A3S_API_KEY");
    let _legacy = EnvGuard::unset("GROK_CODE_A3S_API_KEY");
    let _auth_path = EnvGuard::unset("GROK_AUTH_PATH");
    let dir = tempfile::tempdir().unwrap();
    let mgr = Arc::new(AuthManager::new(dir.path(), GrokComConfig::default()));
    let auth = build_voice_auth(mgr.clone());

    mgr.hot_swap(session("https://cursor.com"));
    assert_eq!(Err(VoiceAuthError::ForeignSession), auth.bearer().await);

    mgr.set_process_static_api_key(Some("a3s-static-key".to_owned()));
    assert_eq!(Ok("a3s-static-key".to_owned()), auth.bearer().await);

    mgr.set_process_static_api_key(None);
    mgr.clear().unwrap();
    assert_eq!(Err(VoiceAuthError::NotSignedIn), auth.bearer().await);
}
