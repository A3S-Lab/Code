#![cfg(test)]

use super::*;

fn report(tree: bool, host_exit: bool) -> CapabilityReport {
    let mut capabilities = a3s_sandbox::BackendCapabilities::native_gate2();
    capabilities.process_tree_cleanup = tree;
    capabilities.host_exit_cleanup = host_exit;
    CapabilityReport {
        backend: "contract-backend",
        capabilities,
        policy_digest: "contract-policy".into(),
        unavailable: Vec::new(),
        runtime_probed: false,
    }
}

#[test]
fn foreground_and_background_require_both_lifetime_capabilities() {
    for (tree, host_exit, expected) in [
        (false, false, Some(ExecutionUnavailable::ProcessTreeCleanup)),
        (false, true, Some(ExecutionUnavailable::ProcessTreeCleanup)),
        (true, false, Some(ExecutionUnavailable::HostExitCleanup)),
        (true, true, None),
    ] {
        let result = ensure_lifetime_capabilities(&report(tree, host_exit));
        match expected {
            Some(reason) => {
                let error = result.unwrap_err().context("before user command dispatch");
                assert!(is_native_execution_unavailable(&error));
                assert_eq!(
                    error
                        .chain()
                        .find_map(|cause| cause.downcast_ref::<ExecutionUnavailable>()),
                    Some(&reason)
                );
            }
            None => result.unwrap(),
        }
    }
}

#[test]
fn only_adapter_bound_refusals_prove_that_the_user_command_did_not_start() {
    for failure in [
        a3s_sandbox::ExecutionFailure::Cancelled,
        a3s_sandbox::ExecutionFailure::SettlementUnknown,
        a3s_sandbox::ExecutionFailure::ObserverTimeout,
    ] {
        let error = anyhow::Error::from(failure).context("a command may already have started");
        assert!(!is_native_execution_unavailable(&error));
    }
    assert!(!is_native_execution_unavailable(&anyhow::anyhow!(
        "native_execution_unavailable: process_tree_cleanup_unavailable"
    )));
    assert!(!is_native_execution_unavailable(&anyhow::Error::from(
        ExecutionUnavailable::HostExitCleanup
    )));
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_real_launcher_probe_does_not_waive_missing_lifetime_capabilities() {
    let workspace = tempfile::tempdir().unwrap();
    let sandbox = NativeBashSandbox::new(workspace.path()).unwrap();
    let advertised = sandbox.capability_report();
    assert!(!advertised.runtime_probed);
    assert_eq!(
        advertised.require_lifetime_containment(),
        Err(ExecutionUnavailable::RuntimeProbeRequired)
    );
    let probed = sandbox.probe_capabilities().await.unwrap();
    assert!(probed.runtime_probed);
    assert_eq!(probed.policy_digest, advertised.policy_digest);
    assert_eq!(
        probed.require_lifetime_containment(),
        Err(ExecutionUnavailable::ProcessTreeCleanup)
    );
    assert!(is_native_execution_unavailable(
        &sandbox.validate_execution_available().unwrap_err()
    ));
    let error = sandbox
        .exec_command("printf unexpected > post-probe-sentinel", "/workspace")
        .await
        .err()
        .unwrap();
    assert!(is_native_execution_unavailable(&error));
    assert!(!workspace.path().join("post-probe-sentinel").exists());
    assert!(!sandbox.supports_background_execution());
}
