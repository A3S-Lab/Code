#![cfg(test)]

use super::*;
use crate::sandbox::{BashSandbox, SandboxOutput};

struct UnavailableSandbox {
    ready: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait]
impl BashSandbox for UnavailableSandbox {
    fn validate_execution_available(&self) -> anyhow::Result<()> {
        if !self.ready.load(Ordering::Acquire) {
            anyhow::bail!("test_namespace_unavailable: no qualified command backend")
        }
        Ok(())
    }

    async fn exec_command(&self, _: &str, _: &str) -> anyhow::Result<SandboxOutput> {
        anyhow::bail!("the unavailable backend must not execute")
    }

    async fn shutdown(&self) {}
}

#[tokio::test]
async fn the_first_model_request_names_an_unavailable_command_backend() {
    let workspace = tempfile::tempdir().unwrap();
    let agent = durable_source::source_agent().await;
    let systems = Arc::new(std::sync::Mutex::new(Vec::new()));
    let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let session = agent
        .build_session(
            workspace.path().to_string_lossy().to_string(),
            Arc::new(SkillCapturingClient {
                systems: Arc::clone(&systems),
            }),
            &crate::SessionOptions::new()
                .with_session_id("unavailable-sandbox-context")
                .with_planning_mode(crate::prompts::PlanningMode::Disabled)
                .with_sandbox_handle(Arc::new(UnavailableSandbox {
                    ready: Arc::clone(&ready),
                }))
                .with_auto_delegation_enabled(false)
                .with_continuation(false),
        )
        .unwrap();
    session
        .send("Describe the execution environment.", None)
        .await
        .unwrap();
    let first = systems.lock().unwrap().first().cloned().unwrap();
    assert!(
        first.contains("test_namespace_unavailable: no qualified command backend"),
        "the model must know execution is unavailable before requesting Bash"
    );
    ready.store(true, Ordering::Release);
    session
        .send("Describe the updated execution environment.", None)
        .await
        .unwrap();
    let latest = systems.lock().unwrap().last().cloned().unwrap();
    assert!(
        !latest.contains("test_namespace_unavailable: no qualified command backend"),
        "a recovered backend must replace its stale unavailability context"
    );
    session.close().await;
    agent.close().await;
}
