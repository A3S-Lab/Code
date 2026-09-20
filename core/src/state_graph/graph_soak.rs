//! S-SG-01: repeated patches and forks keep a verifiable hash chain, an
//! unchanged parent head, and a disk footprint proportional to the events.

use super::{FileGraphEventStore, GraphEventStore, GraphPatch, GraphRuntime, PatchOperation};
use serde_json::json;
use std::path::Path;

const PATCHES: usize = 100;
const FORKS: usize = 10;

fn add_object(version: u64, id: &str) -> GraphPatch {
    GraphPatch::new(
        version,
        vec![PatchOperation::AddObject {
            id: id.to_string(),
            object_type: "task".to_string(),
            data: json!({ "n": id }),
        }],
    )
}

fn store_bytes(root: &Path) -> u64 {
    std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .map(|entry| entry.metadata().map(|meta| meta.len()).unwrap_or(0))
        .sum()
}

fn leftover_temp_files(root: &Path) -> Vec<String> {
    std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.contains("tmp").then_some(name)
        })
        .collect()
}

#[tokio::test]
#[ignore = "S-SG-01 state-graph patch and fork soak; run with --ignored"]
async fn soak_patches_and_forks_keep_parent_heads_and_bounded_disk() {
    let root = tempfile::tempdir().unwrap();
    let store = FileGraphEventStore::new(root.path());
    let mut parent = GraphRuntime::new();
    let mut forks = Vec::new();
    let mut fork_count = 0usize;

    for index in 0..PATCHES {
        let version = parent.graph().version();
        let applied = parent
            .propose_patch(add_object(version, &format!("obj-{index:03}")), None)
            .unwrap();
        assert!(applied, "patch {index} must extend the head");

        if (index + 1) % (PATCHES / FORKS) == 0 {
            let head = parent
                .events()
                .last()
                .expect("a patch appends an event")
                .record_hash
                .clone();
            let sequence = parent.events().len() as u64;
            let mut fork = parent.fork_at(sequence).unwrap();
            assert_eq!(
                parent.events().last().unwrap().record_hash,
                head,
                "forking must not rewrite the parent head"
            );
            let fork_version = fork.graph().version();
            let fork_id = format!("fork-only-{fork_count:02}");
            assert!(fork
                .propose_patch(add_object(fork_version, &fork_id), None)
                .unwrap());
            assert_eq!(parent.events().last().unwrap().record_hash, head);
            assert!(parent.graph().object(&fork_id).is_none());
            let replayed = GraphRuntime::strict_replay(fork.events()).unwrap();
            assert_eq!(&replayed, fork.graph());
            store.save(fork.branch_id(), fork.events()).await.unwrap();
            forks.push((fork.branch_id().to_string(), head));
            fork_count += 1;
        }
    }

    assert_eq!(forks.len(), FORKS);
    let parent_replay = GraphRuntime::strict_replay(parent.events()).unwrap();
    assert_eq!(&parent_replay, parent.graph());
    store
        .save(parent.branch_id(), parent.events())
        .await
        .unwrap();

    let event_count = parent.events().len();
    let bytes = store_bytes(root.path());
    assert!(
        bytes <= (event_count as u64) * 16 * 1024,
        "graph store {bytes} bytes exceeds the per-event bound for {event_count} events"
    );
    assert!(
        leftover_temp_files(root.path()).is_empty(),
        "atomic saves must not leave temp files"
    );

    for (branch_id, _) in forks.iter().take(FORKS / 2) {
        store.delete(branch_id).await.unwrap();
        assert!(store.load(branch_id).await.unwrap().is_none());
    }
    for (branch_id, _) in forks.iter().skip(FORKS / 2) {
        let loaded = store.load(branch_id).await.unwrap().expect("kept fork");
        GraphRuntime::strict_replay(&loaded).unwrap();
    }
    let restored = GraphRuntime::restore(
        store
            .load(parent.branch_id())
            .await
            .unwrap()
            .expect("parent branch"),
    )
    .unwrap();
    assert_eq!(restored.graph(), parent.graph());
    assert!(
        leftover_temp_files(root.path()).is_empty(),
        "deletes must not leave orphan temp files"
    );
}
