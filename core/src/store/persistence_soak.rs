//! S-PE-01: forty snapshot generations, five of them killed after the WAL
//! intent and before the atomic rename. Resume must return the last durable
//! generation, never a mix of the temp file and the previous snapshot.

use super::tests::create_test_snapshot;
use super::{
    snapshot_content_digest, FileSessionStore, FileSessionStoreWal, SessionStore,
    SessionStoreWalEntryV1, SessionStoreWalPhaseV1,
};
use base64::Engine as _;

fn session_file(root: &std::path::Path, id: &str) -> std::path::PathBuf {
    let key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(id.as_bytes());
    root.join("v1")
        .join("sessions")
        .join(format!("id_{key}.json"))
}

#[tokio::test]
#[ignore = "S-PE-01 soak: 40 snapshot generations with 5 crash-before-rename injections"]
async fn soak_snapshot_resume_keeps_one_generation_across_rename_crashes() {
    let dir = tempfile::tempdir().expect("store");
    for cycle in 0..40 {
        let generation = format!("GEN-{cycle:02}");
        let prompt = format!("PROMPT-{cycle:02}");
        let store = FileSessionStore::new(dir.path()).await.expect("open");
        let mut snapshot = create_test_snapshot().await;
        snapshot.session.config.name = generation.clone();
        snapshot.session.config.system_prompt = Some(prompt.clone());
        store.save_snapshot(&snapshot).await.expect("save");
        let durable = snapshot_content_digest(&snapshot).expect("digest");
        drop(store);

        if cycle % 8 == 0 {
            let mut torn = create_test_snapshot().await;
            torn.session.config.name = format!("TORN-{cycle:02}");
            torn.session.config.system_prompt = Some(prompt.clone());
            let torn_digest = snapshot_content_digest(&torn).expect("torn digest");
            let wal = FileSessionStoreWal::new(dir.path());
            let (_, next) = wal.load_entries().await.expect("wal");
            let intent = SessionStoreWalEntryV1::new(
                next,
                "test-session-1",
                torn_digest,
                SessionStoreWalPhaseV1::Intent,
                1,
            )
            .expect("intent");
            wal.append(&intent).await.expect("append intent");
            let temp = session_file(dir.path(), "test-session-1")
                .with_extension(format!("json.crash-{cycle}.tmp"));
            tokio::fs::write(&temp, serde_json::to_vec(&torn).expect("torn json"))
                .await
                .expect("temp");
        }

        let resumed = FileSessionStore::new(dir.path())
            .await
            .unwrap_or_else(|error| panic!("cycle {cycle} reopen panicked: {error}"));
        let loaded = resumed
            .load_snapshot("test-session-1")
            .await
            .unwrap_or_else(|error| panic!("cycle {cycle} load: {error}"))
            .unwrap_or_else(|| panic!("cycle {cycle} missing snapshot"));
        assert_eq!(
            snapshot_content_digest(&loaded).expect("loaded digest"),
            durable,
            "cycle {cycle} resumed a different generation"
        );
        assert_eq!(loaded.session.config.name, generation);
        assert_eq!(
            loaded.session.config.system_prompt.as_deref(),
            Some(prompt.as_str())
        );
        assert!(
            !loaded.session.config.name.starts_with("TORN"),
            "cycle {cycle} mixed a crash temp into the durable snapshot"
        );
    }
}
