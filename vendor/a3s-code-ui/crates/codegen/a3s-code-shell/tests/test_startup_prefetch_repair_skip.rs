//! Its own binary: the a3s home resolves once per process.

mod common;

use a3s_code_shell::agent::remote_config::settings_get::{SettingsQuery, get_settings};

#[test]
fn getter_does_not_fetch_while_policy_repair_is_pending() {
    let home = common::isolated_home();
    common::block_on(async {
        let server = common::start_seeded_mock(home.path()).await;
        // A team principal with no serving managed policy: repair is pending,
        // so the getter must return ineligible without egress.
        let scope = a3s_code_login::GrokComConfig::default().auth_scope();
        let auth = serde_json::json!({
            scope: {
                "key": "team-session-token",
                "auth_mode": "oidc",
                "oidc_issuer": a3s_code_login::a3s_oauth2_issuer(),
                "create_time": "2026-01-01T00:00:00Z",
                "expires_at": "2099-01-01T00:00:00Z",
                "user_id": "test-user",
                "principal_type": "Team",
                "team_id": "team-1",
            }
        });
        std::fs::write(home.path().join("auth.json"), auth.to_string())
            .expect("write team auth.json");

        let outcome = get_settings(SettingsQuery::from_auth(None)).await;
        assert!(
            outcome.settings().is_none() && !outcome.attempted(),
            "a pending policy repair must skip the settings getter"
        );
        assert_eq!(
            server.request_count_for("/v1/settings"),
            0,
            "no authenticated request may leave before the policy repair"
        );
    });
}
