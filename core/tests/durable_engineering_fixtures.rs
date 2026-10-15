#![cfg(test)]

#[path = "fixtures/durable-engineering-v1/fixtures.rs"]
mod fixtures;

use fixtures::{Fixture, FIXTURES};
use std::process::Command;

fn passes(fixture: &Fixture, root: &std::path::Path) -> bool {
    if !fixture.protected_bytes_match(root) {
        return false;
    }
    Command::new("node")
        .arg("test.mjs")
        .current_dir(root)
        .output()
        .expect("Node is required to calibrate these acceptance fixtures")
        .status
        .success()
}

fn calibrate(fixture: &Fixture) {
    let workspace = tempfile::tempdir().unwrap();
    fixture.seed(workspace.path());
    assert!(
        !passes(fixture, workspace.path()),
        "initial fixture must fail"
    );

    fixture.apply(workspace.path(), fixture.correct);
    assert!(
        passes(fixture, workspace.path()),
        "correct reference rejected"
    );

    fixture.seed(workspace.path());
    fixture.apply(workspace.path(), fixture.incorrect);
    assert!(
        !passes(fixture, workspace.path()),
        "incorrect reference accepted"
    );
    assert!(!fixture.request.is_empty());
}

#[test]
fn existing_implementation_reuse_has_a_behavioral_oracle() {
    calibrate(&fixtures::REUSE);
}

#[test]
fn caller_closure_rejects_a_partial_fix() {
    calibrate(&fixtures::CALLERS);
}

#[test]
fn tenant_boundary_and_default_denial_survive_the_extension() {
    calibrate(&fixtures::BOUNDARY);
}

#[test]
fn scope_fixture_checks_numeric_behavior() {
    calibrate(&fixtures::SCOPE);
}

#[test]
fn test_tampering_cannot_turn_an_incorrect_reference_green() {
    for fixture in FIXTURES {
        let workspace = tempfile::tempdir().unwrap();
        fixture.seed(workspace.path());
        fixture.apply(workspace.path(), fixture.incorrect);
        std::fs::write(workspace.path().join("test.mjs"), "process.exit(0);\n").unwrap();
        assert!(
            !passes(fixture, workspace.path()),
            "{} accepted modified checks",
            fixture.id
        );
    }
}

#[test]
fn unrelated_document_changes_fail_even_when_behavior_passes() {
    let fixture = &fixtures::SCOPE;
    let workspace = tempfile::tempdir().unwrap();
    fixture.seed(workspace.path());
    fixture.apply(workspace.path(), fixture.correct);
    std::fs::write(
        workspace.path().join("docs/note.md"),
        "unrequested rewrite\n",
    )
    .unwrap();
    assert!(!passes(fixture, workspace.path()));
}

#[test]
fn durable_reference_does_not_close_after_only_one_caller_is_fixed() {
    assert!(!fixtures::DURABLE_REQUEST.is_empty());
    let fixture = &fixtures::CALLERS;
    let workspace = tempfile::tempdir().unwrap();
    fixture.seed(workspace.path());
    assert!(!passes(fixture, workspace.path()));
    fixture.apply(workspace.path(), &fixture.correct[..1]);
    assert!(!passes(fixture, workspace.path()));
    fixture.apply(workspace.path(), &fixture.correct[1..]);
    assert!(passes(fixture, workspace.path()));
}
