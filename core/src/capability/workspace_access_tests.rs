#![cfg(test)]

use super::*;
use crate::capability::{
    CapabilityCeiling, CapabilityExecutionCeiling, CapabilityScope, CapabilitySet,
    GovernanceCapabilityCeiling,
};
use crate::tools::ToolContext;

fn ceiling(set: &CapabilitySet, workspace: WorkspaceCapabilityCeiling) -> CapabilityCeiling {
    CapabilityCeiling::all(
        set,
        workspace,
        GovernanceCapabilityCeiling::none_required(),
        CapabilityExecutionCeiling::new(8, 2, None, None, None).unwrap(),
    )
    .unwrap()
}

#[test]
fn workspace_operation_closed_set_refuses_unknown_names_and_plain_contexts() {
    let ctx = ToolContext::new(std::env::temp_dir());
    for name in [
        "read",
        "write",
        "execute",
        "search",
        "git",
        "code_intelligence",
    ] {
        let operation = WorkspaceOperation::parse(name).unwrap();
        assert_eq!(operation.as_str(), name);
        assert_eq!(
            ctx.workspace_operation(operation).unwrap_err(),
            CapabilityScopeError::WorkspaceOperationUnavailable
        );
    }
    assert!(matches!(
        WorkspaceOperation::parse("admin"),
        Err(CapabilityScopeError::InvalidWorkspaceOperation { .. })
    ));
}

#[tokio::test]
async fn workspace_operation_access_honors_narrowing_and_parent_revocation() {
    let set = CapabilitySet::empty().unwrap();
    let full = ceiling(&set, WorkspaceCapabilityCeiling::all());
    let session =
        CapabilityScope::new_session("resource-proof", set.clone(), full.clone()).unwrap();
    let run = session.admit_run("run", full).unwrap();
    let narrow = run
        .turn(
            "readonly",
            ceiling(&set, WorkspaceCapabilityCeiling::none().with_read(true)),
        )
        .unwrap();
    let access = WorkspaceOperationAccess::new(narrow.handle(), WorkspaceOperation::Read).unwrap();
    let observation = access.check().unwrap();
    assert_eq!(observation.scope_id(), narrow.id());
    assert_eq!(observation.catalog_digest(), set.digest());
    assert_eq!(observation.catalog_generation(), set.generation());
    assert!(observation.use_generation().is_none());
    assert_eq!(
        WorkspaceOperationAccess::new(narrow.handle(), WorkspaceOperation::Write).unwrap_err(),
        CapabilityScopeError::WorkspaceOperationUnavailable
    );
    let cancellation = access.cancellation().unwrap();
    run.cancel();
    assert!(cancellation.is_cancelled());
    assert_eq!(
        access.check().unwrap_err(),
        CapabilityScopeError::WorkspaceOperationUnavailable
    );
    assert_eq!(
        access.cancellation().unwrap_err(),
        CapabilityScopeError::WorkspaceOperationUnavailable
    );
    assert!(run.close().await.unwrap().is_clean());
    assert!(session.close().await.unwrap().is_clean());
}

#[tokio::test]
async fn cloned_workspace_access_does_not_keep_a_dropped_owner_alive() {
    let set = CapabilitySet::empty().unwrap();
    let full = ceiling(&set, WorkspaceCapabilityCeiling::all());
    let session = CapabilityScope::new_session("weak-resource-proof", set, full.clone()).unwrap();
    let run = session.admit_run("run", full.clone()).unwrap();
    let turn = run.turn("turn", full).unwrap();
    let access = WorkspaceOperationAccess::new(turn.handle(), WorkspaceOperation::Execute).unwrap();
    let clone = access.clone();
    let cancellation = clone.cancellation().unwrap();
    drop(turn);
    assert!(cancellation.is_cancelled());
    assert_eq!(
        clone.check().unwrap_err(),
        CapabilityScopeError::WorkspaceOperationUnavailable
    );
    assert_eq!(
        access.check().unwrap_err(),
        CapabilityScopeError::WorkspaceOperationUnavailable
    );
    run.close().await.unwrap();
    session.close().await.unwrap();
}
