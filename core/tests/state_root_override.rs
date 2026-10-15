//! The process-wide state root override is set at most once, so its whole
//! contract lives in this one test binary and one test.

use std::path::Path;

use a3s_code_core::{session_folder, state_root};
use a3s_effect::{FileLog, LogStore, NewFact};

const ID: &str = "33333333-3333-4333-8333-333333333333";

#[test]
fn an_override_keeps_harness_state_out_of_the_workspace() {
    let workspace = tempfile::tempdir().unwrap();
    let workspace = workspace.path();
    let state = tempfile::tempdir().unwrap();
    let state = state.path().canonicalize().unwrap();
    let other = tempfile::tempdir().unwrap();

    assert!(state_root::state_root_override().is_none());
    assert_eq!(
        session_folder::sessions_root(workspace),
        workspace.join(".a3s").join("sessions")
    );
    assert!(state_root::set_state_root_override(Path::new("relative/state")).is_err());
    assert!(state_root::set_state_root_override(&state.join("missing")).is_err());
    #[cfg(unix)]
    {
        let link = other.path().join("link");
        std::os::unix::fs::symlink(&state, &link).unwrap();
        assert!(state_root::set_state_root_override(&link).is_err());
    }
    assert!(state_root::state_root_override().is_none());

    state_root::set_state_root_override(&state).unwrap();
    state_root::set_state_root_override(&state).unwrap();
    assert!(state_root::set_state_root_override(other.path()).is_err());
    assert_eq!(state_root::state_root_override(), Some(state.as_path()));

    let home = session_folder::prepare_session_home(workspace, ID)
        .unwrap()
        .unwrap();
    assert!(home.starts_with(state.join("sessions")));
    assert_eq!(session_folder::effect_log_dir(workspace, ID), home);
    assert_eq!(
        session_folder::legacy_effect_log_dir(workspace),
        state.join("effect-log")
    );
    FileLog::open(&home)
        .unwrap()
        .append(
            &a3s_code_core::fact_control::thread_for_session(ID),
            &[NewFact {
                kind: "user.message".into(),
                key: "user:1".into(),
                payload: serde_json::json!({ "text": "graded task" }),
            }],
            None,
        )
        .unwrap();

    let resolved = session_folder::resolve_workspace_session(workspace, ID)
        .unwrap()
        .expect("override session resolves");
    assert_eq!(resolved.id, ID);
    let page = session_folder::list_workspace_sessions(workspace, 10, None).unwrap();
    assert!(page.sessions.iter().any(|entry| entry.id == ID));

    std::fs::write(workspace.join("main.py"), "print('graded')\n").unwrap();
    assert!(
        a3s_code_core::workspace::TrigramGrepCandidateIndex::default_index_dir(workspace)
            .starts_with(state.join("indexes"))
    );
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _entered = runtime.enter();
    let services =
        a3s_code_core::workspace::WorkspaceServices::local_with_indexed_retrieval(workspace)
            .unwrap();
    assert!(services.persistent_index().is_some());
    assert!(
        std::fs::read_dir(state.join("indexes")).unwrap().count() == 1,
        "the a3s-vec index lands under the state root"
    );

    assert!(
        !workspace.join(".a3s").exists(),
        "the graded workspace holds no harness state"
    );
    assert!(
        !workspace.join(".a3s-code").exists(),
        "the graded workspace holds no index"
    );
}
