use std::sync::Arc;

use base64::Engine as _;
use chrono::{Duration, Utc};
use pretty_assertions::assert_eq;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use a3s_code_test_support::EnvGuard;
use a3s_code_tools::implementations::grok_build::image_gen::{ImageGenClient, ImageGenConfig};
use a3s_code_tools::implementations::grok_build::media_bearer::SIDE_CALL_BEARER_ERROR_CODE;
use a3s_code_tools::types::api_key_provider::SideCallBearerError;

use super::{SharedAuthKeyProvider, is_a3s_side_call_principal};
use crate::config::{GrokComConfig, A3S_OAUTH2_ISSUER};
use crate::{AuthManager, AuthMode, GrokAuth};

const FOREIGN_ISSUER: &str = "https://cursor.com";

fn unsigned_jwt(header: &str, payload: &str) -> String {
    let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    format!("{}.{}.sig", enc.encode(header), enc.encode(payload))
}

fn foreign_access_token() -> String {
    unsigned_jwt(
        r#"{"alg":"RS256","typ":"at+jwt"}"#,
        &format!(r#"{{"iss":"{FOREIGN_ISSUER}","sub":"user-1"}}"#),
    )
}

fn session(key: &str, mode: AuthMode, issuer: Option<&str>) -> GrokAuth {
    GrokAuth {
        key: key.to_owned(),
        auth_mode: mode,
        oidc_issuer: issuer.map(str::to_owned),
        refresh_token: Some("rt".to_owned()),
        expires_at: Some(Utc::now() + Duration::hours(1)),
        ..GrokAuth::test_default()
    }
}

fn api_key(key: &str) -> GrokAuth {
    GrokAuth {
        key: key.to_owned(),
        auth_mode: AuthMode::ApiKey,
        oidc_issuer: None,
        expires_at: None,
        ..GrokAuth::test_default()
    }
}

/// The process env may carry a developer's key; every test pins it out so the credential alone decides.
fn static_key_guards() -> [EnvGuard; 3] {
    [
        EnvGuard::unset("A3S_API_KEY"),
        EnvGuard::unset("GROK_CODE_A3S_API_KEY"),
        EnvGuard::unset("GROK_AUTH_PATH"),
    ]
}

fn manager(dir: &tempfile::TempDir, auth: Option<GrokAuth>) -> Arc<AuthManager> {
    let mgr = Arc::new(AuthManager::new(dir.path(), GrokComConfig::default()));
    if let Some(auth) = auth {
        mgr.hot_swap(auth);
    }
    mgr
}

/// Only a foreign-issuer credential is refused. Everything chat sends today keeps going to the
/// server, which decides: an enterprise IdP session, a bare external-provider token, a legacy web login.
#[test]
fn only_a_foreign_credential_is_refused() {
    assert!(is_a3s_side_call_principal(&api_key("a3s-plain-key")));
    assert!(is_a3s_side_call_principal(&session(
        "opaque",
        AuthMode::Oidc,
        Some(A3S_OAUTH2_ISSUER)
    )));
    assert!(is_a3s_side_call_principal(&session(
        "opaque",
        AuthMode::Oidc,
        Some("https://idp.acme.example")
    )));
    assert!(is_a3s_side_call_principal(&session(
        "bare-provider-token",
        AuthMode::External,
        None
    )));
    assert!(is_a3s_side_call_principal(&session(
        "legacy",
        AuthMode::WebLogin,
        None
    )));

    assert!(!is_a3s_side_call_principal(&session(
        &foreign_access_token(),
        AuthMode::Oidc,
        Some(FOREIGN_ISSUER)
    )));
    assert!(!is_a3s_side_call_principal(&session(
        &foreign_access_token(),
        AuthMode::Oidc,
        Some("https://api.cursor.com")
    )));
    assert!(is_a3s_side_call_principal(&session(
        "opaque",
        AuthMode::Oidc,
        Some("https://cursor.com.evil.test")
    )));
}

#[test]
#[serial_test::serial]
fn foreign_session_is_an_error_not_a_bearer() {
    let _guards = static_key_guards();
    let dir = tempfile::tempdir().unwrap();
    let mgr = manager(
        &dir,
        Some(session(
            &foreign_access_token(),
            AuthMode::Oidc,
            Some(FOREIGN_ISSUER),
        )),
    );

    assert_eq!(
        Err(SideCallBearerError::ForeignSession),
        mgr.side_call_bearer()
    );
}

#[tokio::test]
#[serial_test::serial]
async fn foreign_session_is_an_error_after_the_refresh_path_too() {
    let _guards = static_key_guards();
    let dir = tempfile::tempdir().unwrap();
    let mgr = manager(
        &dir,
        Some(session(
            &foreign_access_token(),
            AuthMode::Oidc,
            Some(FOREIGN_ISSUER),
        )),
    );

    assert_eq!(
        Err(SideCallBearerError::ForeignSession),
        mgr.side_call_bearer_async().await
    );
}

#[test]
#[serial_test::serial]
fn no_credential_is_missing_not_a_fallback() {
    let _guards = static_key_guards();
    let dir = tempfile::tempdir().unwrap();
    let mgr = manager(&dir, None);

    assert_eq!(Err(SideCallBearerError::Missing), mgr.side_call_bearer());
}

#[test]
#[serial_test::serial]
fn static_key_still_serves_a_foreign_login() {
    let _guards = static_key_guards();
    let dir = tempfile::tempdir().unwrap();
    let mgr = manager(
        &dir,
        Some(session(
            &foreign_access_token(),
            AuthMode::Oidc,
            Some(FOREIGN_ISSUER),
        )),
    );
    mgr.set_process_static_api_key(Some("a3s-static-key".to_owned()));

    assert_eq!(Ok("a3s-static-key".to_owned()), mgr.side_call_bearer());
}

#[test]
#[serial_test::serial]
fn kill_switch_blocks_the_static_key_for_side_calls_too() {
    let _guards = static_key_guards();
    let dir = tempfile::tempdir().unwrap();
    let mgr = Arc::new(AuthManager::new(
        dir.path(),
        GrokComConfig {
            disable_api_key_auth: Some(true),
            ..GrokComConfig::default()
        },
    ));
    mgr.hot_swap(session(
        &foreign_access_token(),
        AuthMode::Oidc,
        Some(FOREIGN_ISSUER),
    ));
    mgr.set_process_static_api_key(Some("a3s-static-key".to_owned()));

    assert_eq!(
        Err(SideCallBearerError::ForeignSession),
        mgr.side_call_bearer()
    );
}

#[test]
#[serial_test::serial]
fn a3s_principals_are_returned() {
    let _guards = static_key_guards();
    let dir = tempfile::tempdir().unwrap();

    let mgr = manager(&dir, Some(api_key("a3s-plain-key")));
    assert_eq!(Ok("a3s-plain-key".to_owned()), mgr.side_call_bearer());

    mgr.hot_swap(session(
        "a3s-session",
        AuthMode::Oidc,
        Some(A3S_OAUTH2_ISSUER),
    ));
    assert_eq!(Ok("a3s-session".to_owned()), mgr.side_call_bearer());
}

#[test]
#[serial_test::serial]
fn an_expired_a3s_login_is_missing_in_the_sync_path() {
    let _guards = static_key_guards();
    let dir = tempfile::tempdir().unwrap();
    let mgr = manager(
        &dir,
        Some(GrokAuth {
            expires_at: Some(Utc::now() - Duration::hours(1)),
            ..session("a3s-session", AuthMode::Oidc, Some(A3S_OAUTH2_ISSUER))
        }),
    );

    assert_eq!(Err(SideCallBearerError::Missing), mgr.side_call_bearer());
}

fn image_client(base_url: &str, mgr: &Arc<AuthManager>) -> ImageGenClient {
    let config = ImageGenConfig::Enabled {
        api_key: None,
        base_url: base_url.to_owned(),
        extra_headers: indexmap::IndexMap::new(),
        image_gen_enabled: true,
        image_edit_enabled: true,
        model_override: None,
        edit_model_override: None,
        tier_restricted: false,
    };
    ImageGenClient::new(&config, Some(Arc::new(SharedAuthKeyProvider(mgr.clone())))).unwrap()
}

fn image_response() -> ResponseTemplate {
    let b64 = base64::engine::general_purpose::STANDARD.encode(b"jpeg-bytes");
    ResponseTemplate::new(200).set_body_json(serde_json::json!({ "data": [{ "b64_json": b64 }] }))
}

#[tokio::test]
#[serial_test::serial]
async fn imagine_request_carries_only_an_a3s_bearer() {
    let _guards = static_key_guards();
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    let mgr = manager(
        &dir,
        Some(session(
            &foreign_access_token(),
            AuthMode::Oidc,
            Some(FOREIGN_ISSUER),
        )),
    );
    let client = image_client(&server.uri(), &mgr);

    let error = client
        .generate("a cat", "auto")
        .await
        .expect_err("a foreign session must not reach the imagine api");
    assert_eq!(
        Some(SIDE_CALL_BEARER_ERROR_CODE),
        error
            .details
            .as_ref()
            .and_then(|d| d.get("code"))
            .and_then(serde_json::Value::as_str)
    );
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "no HTTP request may leave on a foreign session"
    );

    Mock::given(method("POST"))
        .and(path("/images/generations"))
        .and(header("authorization", "Bearer a3s-plain-key"))
        .respond_with(image_response())
        .expect(1)
        .mount(&server)
        .await;
    mgr.hot_swap(api_key("a3s-plain-key"));
    assert_eq!(
        b"jpeg-bytes".to_vec(),
        client.generate("a cat", "auto").await.unwrap()
    );
    server.reset().await;

    Mock::given(method("POST"))
        .and(path("/images/generations"))
        .and(header("authorization", "Bearer a3s-session"))
        .respond_with(image_response())
        .expect(1)
        .mount(&server)
        .await;
    mgr.hot_swap(session(
        "a3s-session",
        AuthMode::Oidc,
        Some(A3S_OAUTH2_ISSUER),
    ));
    assert_eq!(
        b"jpeg-bytes".to_vec(),
        client.generate("a cat", "auto").await.unwrap()
    );
}
