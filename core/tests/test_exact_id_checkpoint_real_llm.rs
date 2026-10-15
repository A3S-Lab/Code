//! The exact-id run rail persists a loop checkpoint at every tool round —
//! the boundary a hard process crash between rounds recovers from.
//! Requires the repo `.a3s/config.acl` flash provider.

use a3s_code_core::{Agent, AgentEvent, SessionOptions};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a real provider configured in .a3s/config.acl"]
async fn exact_id_run_persists_loop_checkpoint_mid_run() {
    let config_path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.a3s/config.acl");
    let config = a3s_code_core::CodeConfig::from_file(&config_path).unwrap();
    let agent = Agent::from_config(config).await.unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let store_dir = tempfile::tempdir().unwrap();
    let session = agent
        .session_async(
            workspace.path().to_string_lossy().to_string(),
            Some(
                SessionOptions::new()
                    .with_session_id("ckpt-probe")
                    .with_file_session_store(store_dir.path().to_path_buf()),
            ),
        )
        .await
        .unwrap();
    // seed
    tokio::time::timeout(Duration::from_secs(30), session.send("Reply ok.", None))
        .await
        .unwrap()
        .unwrap();
    session.save().await.unwrap();

    // Exact-id spawn: the checkpoint rail must persist per tool round even
    // though this run did not go through the owned-run path.
    let run_id = format!("probe-{}", uuid::Uuid::new_v4());
    let worker = match session
        .spawn_run_with_id(&run_id, "Run the shell command `echo hello` then stop.")
        .await
        .unwrap()
    {
        a3s_code_core::AgentRunSpawn::Started { worker, .. } => worker,
        a3s_code_core::AgentRunSpawn::Replayed { .. } => panic!("fresh id"),
    };
    let saw_tool = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let events = session.run_events(&run_id).await;
            if events
                .iter()
                .any(|record| matches!(record.event, AgentEvent::ToolEnd { .. }))
            {
                return true;
            }
            if worker.is_finished() {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    })
    .await
    .unwrap();
    assert!(saw_tool);
    // Poll for the on-disk checkpoint immediately (mid-run), before any
    // teardown: the durable boundary is the crash-recovery contract.
    let mut checkpoint_seen = false;
    for _ in 0..100 {
        if walk_has_checkpoint(store_dir.path()) {
            checkpoint_seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // The checkpoint must be ON DISK while the run is still in flight —
    // that is what a hard process crash between tool rounds recovers from.
    // Poll for it before tearing anything down (the run may otherwise
    // settle and clear its checkpoint).
    drop(session);
    agent.close().await;

    let mut found = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else {
                    out.push(p.display().to_string());
                }
            }
        }
    }
    walk(store_dir.path(), &mut found);
    eprintln!("PROBE all files: {found:#?}");
    let ckpts: Vec<_> = found
        .into_iter()
        .filter(|f| f.contains("loop_checkpoint"))
        .collect();
    assert!(
        checkpoint_seen || !ckpts.is_empty(),
        "exact-id run must persist a loop checkpoint mid-run"
    );
}

fn walk_has_checkpoint(root: &std::path::Path) -> bool {
    fn walk(dir: &std::path::Path) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    out.extend(walk(&p));
                } else {
                    out.push(p.display().to_string());
                }
            }
        }
        out
    }
    walk(root).iter().any(|f| f.contains("loop_checkpoint"))
}
