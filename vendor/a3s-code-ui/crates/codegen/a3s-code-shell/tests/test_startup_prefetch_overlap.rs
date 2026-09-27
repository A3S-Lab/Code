//! Its own binary: the a3s home resolves once per process.

mod common;

use a3s_code_shell::agent::remote_config::settings_get::{SettingsQuery, get_settings};
use a3s_code_shell::util::config::RemoteSettings;

#[test]
fn mid_session_refresh_hits_the_network() {
    let home = common::isolated_home();
    common::block_on(async {
        let server = common::start_seeded_mock(home.path()).await;
        server.set_settings(RemoteSettings {
            tips: Some(vec!["from-server".into()]),
            ..RemoteSettings::default()
        });

        // Startup cell is unused here. Refresh calls get_settings directly.
        let outcome = get_settings(SettingsQuery::from_auth(None)).await;
        assert_eq!(
            outcome.settings().and_then(|s| s.tips.clone()),
            Some(vec!["from-server".to_string()]),
        );
        assert!(outcome.attempted(), "a cold refresh must hit the network");
        assert_eq!(server.request_count_for("/v1/settings"), 1);
    });
}
