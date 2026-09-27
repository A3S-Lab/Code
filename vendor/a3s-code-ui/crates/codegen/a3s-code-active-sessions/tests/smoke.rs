//! Runs the full session flow in one temp directory: register, clean unregister, then a register
//! that drops the entry whose PID is dead and keeps the live one.

use chrono::Utc;
use tempfile::TempDir;
use a3s_code_active_sessions::{ActiveSession, list_in, register_in, try_unregister_in};

fn session(id: &str, pid: u32) -> ActiveSession {
    ActiveSession {
        session_id: agent_client_protocol::SessionId::new(id),
        pid,
        cwd: "/tmp/test".into(),
        opened_at: Utc::now(),
    }
}

#[test]
fn full_lifecycle() {
    let dir = TempDir::new().unwrap();
    let r = dir.path();
    let pid = std::process::id();
    let sid = |s: &str| agent_client_protocol::SessionId::new(s);

    register_in(r, session("s1", pid)).unwrap();
    assert_eq!(1, list_in(r).unwrap().len());

    assert!(try_unregister_in(r, &sid("s1")).unwrap());
    assert!(list_in(r).unwrap().is_empty());

    register_in(r, session("crashed", 2_000_000_000)).unwrap();
    register_in(r, session("alive", pid)).unwrap();
    let ids: Vec<_> = list_in(r)
        .unwrap()
        .into_iter()
        .map(|s| s.session_id)
        .collect();
    assert_eq!(vec![sid("alive")], ids);
}
