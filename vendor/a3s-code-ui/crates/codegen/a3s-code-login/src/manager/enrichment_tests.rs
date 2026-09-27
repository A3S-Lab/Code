use super::*;
use crate::GrokComConfig;
use a3s_code_test_support::{MockCanAdministerTeam, MockInferenceServer};

const USER_A: &str = "a@acme.test";

fn team_auth() -> GrokAuth {
    GrokAuth {
        oidc_issuer: Some(crate::A3S_OAUTH2_ISSUER.to_owned()),
        email: Some(USER_A.to_owned()),
        principal_type: Some(crate::model::TEAM_PRINCIPAL_TYPE.to_owned()),
        team_id: Some("team-a".to_owned()),
        ..GrokAuth::test_default()
    }
}

async fn denying_server() -> (MockInferenceServer, AuthManager, tempfile::TempDir) {
    let server = MockInferenceServer::start().await.unwrap();
    server.set_user_can_administer_team(MockCanAdministerTeam::Denied);
    let dir = tempfile::tempdir().unwrap();
    let manager =
        AuthManager::new(dir.path(), GrokComConfig::default()).with_proxy_base_url(&server.url());
    manager.hot_swap(team_auth());
    (server, manager, dir)
}

/// The completion write is parked on the blocking pool: the runtime keeps serving meanwhile, and an account switch during the write is answered from the live credential rather than the pre-write snapshot.
/// The park is bounded and its expiry is a flag the test asserts, so a write that runs on the runtime thread fails here instead of hanging.
#[tokio::test]
async fn held_log_write_neither_stalls_the_runtime_nor_freezes_the_answer() {
    let (_server, manager, _home) = denying_server().await;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let entered_tx = std::sync::Mutex::new(Some(entered_tx));
    let release_rx = std::sync::Mutex::new(release_rx);
    let never_released = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook: Arc<dyn Fn() + Send + Sync> = {
        let never_released = never_released.clone();
        Arc::new(move || {
            if let Some(tx) = entered_tx.lock().unwrap().take() {
                let _ = tx.send(());
            }
            if release_rx
                .lock()
                .unwrap()
                .recv_timeout(StdDuration::from_secs(5))
                .is_err()
            {
                never_released.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        })
    };

    let hydration = TEST_LOG_HOOK.scope(
        hook,
        manager.hydrate_can_administer_team(Some(USER_A), Some("team-a")),
    );
    let switch_during_write = async {
        tokio::time::timeout(StdDuration::from_secs(5), entered_rx)
            .await
            .expect("the runtime must keep serving while the write is parked")
            .unwrap();
        assert_eq!(manager.current().unwrap().can_administer_team, Some(false));
        manager.hot_swap(GrokAuth {
            key: "key-b".to_owned(),
            email: Some("b@acme.test".to_owned()),
            ..team_auth()
        });
        release_tx.send(()).unwrap();
    };
    let (answer, ()) = tokio::join!(hydration, switch_during_write);
    assert!(
        !never_released.load(std::sync::atomic::Ordering::SeqCst),
        "the write waited out its release: it ran on the runtime thread"
    );
    assert_eq!(answer, None);
}

/// A logging task that dies changes nothing about the answer or the credential.
#[tokio::test]
async fn failed_log_write_leaves_the_answer_alone() {
    let (_server, manager, _home) = denying_server().await;
    let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(|| panic!("log sink down"));

    let answer = TEST_LOG_HOOK
        .scope(
            hook,
            manager.hydrate_can_administer_team(Some(USER_A), Some("team-a")),
        )
        .await;

    assert_eq!(answer, Some(false));
    assert_eq!(manager.current().unwrap().can_administer_team, Some(false));
}
