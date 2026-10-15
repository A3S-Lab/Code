#![cfg(test)]
#![cfg(target_os = "macos")]

use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Observer(AtomicUsize);

#[async_trait]
impl CommandOutputObserver for Observer {
    async fn on_output_delta(&self, _delta: &str) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    async fn on_output_complete(&self, _summary: &CommandOutputSummary) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn assert_refusal<T>(result: Result<T>) {
    let error = match result {
        Ok(_) => panic!("unqualified native lifetime unexpectedly admitted execution"),
        Err(error) => error,
    };
    assert_eq!(
        error.to_string(),
        super::NATIVE_PROCESS_LIFETIME_UNQUALIFIED
    );
    assert!(
        super::is_native_process_lifetime_unqualified(&error),
        "pre-spawn refusal must stay a typed error, not only its display text: {error:#}"
    );
}

#[tokio::test]
async fn both_native_apis_refuse_uniformly_without_effects_or_output() {
    let workspace = tempfile::tempdir().unwrap();
    let observer = Arc::new(Observer(AtomicUsize::new(0)));
    let commands = [
        "true",
        "printf harmless-output",
        "printf invocation-entered > invocation-sentinel; /usr/bin/python3 -c 'import os; os.fork(); os.setsid()'",
        "exec ./opaque-executable --arbitrary-payload",
    ];
    for posture in [
        super::super::SandboxNetworkMode::Deny,
        super::super::SandboxNetworkMode::Mediated,
    ] {
        let origins = if posture == super::super::SandboxNetworkMode::Mediated {
            vec!["api.example.com:443".into()]
        } else {
            Vec::new()
        };
        let sandbox: Arc<dyn BashSandbox> = Arc::new(
            NativeBashSandbox::with_network_mode(workspace.path(), posture, &origins).unwrap(),
        );
        assert!(!sandbox.supports_background_execution());
        for command in commands {
            assert_refusal(sandbox.exec_command(command, "/workspace").await);
            for timeout_ms in [0, 5_000] {
                assert_refusal(
                    sandbox
                        .exec(SandboxCommandRequest {
                            command: command.into(),
                            guest_workspace: "/workspace".into(),
                            timeout_ms,
                            output_observer: Some(Arc::clone(&observer) as Arc<_>),
                            env: Some(Arc::new(HashMap::from([(
                                "LIFETIME_TEST_NONCE".into(),
                                uuid::Uuid::new_v4().to_string(),
                            )]))),
                        })
                        .await,
                );
            }
        }
    }
    assert_eq!(observer.0.load(Ordering::SeqCst), 0);
    assert!(!workspace.path().join("invocation-sentinel").exists());
    assert_eq!(std::fs::read_dir(workspace.path()).unwrap().count(), 0);
}
