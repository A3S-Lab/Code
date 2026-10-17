use super::{strip_windows_verbatim_prefix, WorkspacePersistentIndex, MANIFEST_FILE};
use crate::workspace::retrieval::{
    ChunkCatalogLimits, ChunkingConfig, LexicalSearchRequest, WorkspaceChunkCatalog,
    WorkspaceLexicalEngine,
};
use crate::workspace::WorkspacePath;
use std::path::PathBuf;

#[test]
fn strip_windows_verbatim_prefix_removes_extended_length_form() {
    let stripped = strip_windows_verbatim_prefix(PathBuf::from(
        r"\\?\C:\Users\runneradmin\AppData\Local\Temp\.tmpIndex\.a3s-code\index",
    ));
    #[cfg(windows)]
    assert_eq!(
        stripped,
        PathBuf::from(r"C:\Users\runneradmin\AppData\Local\Temp\.tmpIndex\.a3s-code\index")
    );
    #[cfg(not(windows))]
    assert_eq!(
        stripped,
        PathBuf::from(r"\\?\C:\Users\runneradmin\AppData\Local\Temp\.tmpIndex\.a3s-code\index")
    );
}

#[cfg(feature = "a3s-vec-fts")]
#[test]
fn persistent_index_opens_under_windows_verbatim_prefix() {
    let directory = tempfile::tempdir().expect("temporary index directory");
    let root = directory.path().join(".a3s-code").join("index");
    std::fs::create_dir_all(&root).expect("index root");
    let open_root = {
        #[cfg(windows)]
        {
            let canonical = std::fs::canonicalize(&root).expect("canonicalize index root");
            if canonical.to_string_lossy().starts_with(r"\\?\") {
                canonical
            } else {
                PathBuf::from(format!(r"\\?\{}", canonical.display()))
            }
        }
        #[cfg(not(windows))]
        {
            root.clone()
        }
    };
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("src/lib.rs"),
            Some("rust"),
            1,
            "verbatim path persistent sentinel\n",
        )
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open(open_root, WorkspaceLexicalEngine::A3sVec)
        .expect("persistent index under verbatim prefix");
    index
        .sync_snapshot(&catalog.snapshot().expect("snapshot"))
        .expect("generation write under verbatim prefix");
    assert!(index.is_ready());
    assert!(
        !index.root().to_string_lossy().starts_with(r"\\?\"),
        "persistent index root should strip the Windows verbatim prefix"
    );
}

#[test]
fn persistent_generation_survives_reopen_and_replaces_removed_content() {
    let directory = tempfile::tempdir().expect("temporary index directory");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("src/lib.rs"),
            Some("rust"),
            1,
            "persistent workspace search sentinel\n",
        )
        .expect("catalog replacement");

    let index = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("persistent index");
    index
        .sync_snapshot(&catalog.snapshot().expect("snapshot"))
        .expect("generation write");
    assert!(index.is_ready());

    let request = LexicalSearchRequest::new("workspace sentinel");
    let result = index.search(&request).expect("persistent query");
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.hits[0].chunk.path.as_ref(), "src/lib.rs");
    drop(index);

    let reopened = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("reopen persistent index");
    let result = reopened.search(&request).expect("reopened query");
    assert_eq!(result.hits.len(), 1);
    assert_eq!(reopened.status().indexed_chunks, 1);

    catalog
        .remove_file(&WorkspacePath::from_normalized("src/lib.rs"), 2)
        .expect("catalog removal");
    reopened
        .sync_snapshot(&catalog.snapshot().expect("empty snapshot"))
        .expect("replacement generation");
    assert!(reopened
        .search(&request)
        .expect("empty query")
        .hits
        .is_empty());
    reopened.drop_index().expect("drop persistent index");
    assert_eq!(
        reopened.status().phase,
        super::WorkspacePersistentIndexPhase::Absent
    );
}

#[cfg(feature = "a3s-vec-fts")]
#[test]
fn cross_epoch_reopen_accepts_fresher_content_below_the_persisted_counters() {
    // Epoch 1 publishes with high per-epoch counters, then exits.
    let directory = tempfile::tempdir().expect("temporary index directory");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/lib.rs");
    catalog
        .replace_file(&path, Some("rust"), 6, "epoch one payload sentinel\n")
        .expect("epoch one replacement");
    {
        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("epoch one persistent index");
        index
            .sync_snapshot(&catalog.snapshot().expect("epoch one snapshot"))
            .expect("epoch one generation");
        assert_eq!(index.status().source_revision, 6);
    }

    // Epoch 2 rescans the workspace with fresh counters and observes an
    // edit. The loaded generation's (6, 1) counters must not veto the
    // fresh epoch's (1, 1): content decides, so the durable projection
    // publishes instead of going permanently stale.
    let fresh = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("epoch two catalog");
    let index = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("epoch two persistent index");
    assert_eq!(index.status().source_revision, 6, "generation must load");
    fresh
        .replace_file(&path, Some("rust"), 1, "epoch two edited beacon payload\n")
        .expect("epoch two edit");
    index
        .sync_snapshot(&fresh.snapshot().expect("epoch two snapshot"))
        .expect("a fresh epoch content change must publish, not go stale");
    let status = index.status();
    assert_eq!(status.source_revision, 1);
    assert!(
        status.indexed_chunks > 0,
        "the edited content must be indexed"
    );
    let hits = index
        .search(&LexicalSearchRequest::new("edited beacon"))
        .expect("epoch two query")
        .hits;
    assert_eq!(hits.len(), 1, "the beacon must be retrievable durably");
    assert!(hits[0].chunk.text.contains("epoch two edited beacon"));

    // A second cross-epoch snapshot with identical content reuses the
    // generation instead of rebuilding it.
    let generation_before = status.generation.clone();
    fresh
        .replace_file(&path, Some("rust"), 2, "epoch two edited beacon payload\n")
        .expect("same-content replacement");
    index
        .sync_snapshot(&fresh.snapshot().expect("same-content snapshot"))
        .expect("cross-epoch identical content must reuse");
    assert_eq!(
        index.status().generation,
        generation_before,
        "identical content must not mint a new generation cross-epoch"
    );
}

#[cfg(feature = "a3s-vec-fts")]
#[test]
fn cross_epoch_identical_counters_with_different_content_still_publishes() {
    // Restart race: the fresh epoch's manifest scan completes after an
    // edit, so its first snapshot carries the same (1, 1) counters as the
    // loaded generation while the content differs. Tuple equality must
    // not silently pin the stale generation.
    let directory = tempfile::tempdir().expect("temporary index directory");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/lib.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "pre-edit payload sentinel\n")
        .expect("epoch one replacement");
    {
        let index =
            WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
                .expect("epoch one persistent index");
        index
            .sync_snapshot(&catalog.snapshot().expect("epoch one snapshot"))
            .expect("epoch one generation");
    }

    let raced = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("raced catalog");
    raced
        .replace_file(&path, Some("rust"), 1, "post-edit beacon payload\n")
        .expect("scanned-after-edit replacement");
    let index = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("reopened persistent index");
    assert_eq!(index.status().source_revision, 1);
    index
        .sync_snapshot(&raced.snapshot().expect("raced snapshot"))
        .expect("identical counters with different content must publish");
    let hits = index
        .search(&LexicalSearchRequest::new("post-edit beacon"))
        .expect("query after raced publish")
        .hits;
    assert_eq!(hits.len(), 1, "the raced edit must be retrievable durably");
    assert!(hits[0].chunk.text.contains("post-edit beacon"));
}

#[cfg(feature = "a3s-vec-fts")]
#[test]
fn incremental_delta_serves_edits_renames_and_reopens() {
    let directory = tempfile::tempdir().expect("temporary index directory");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    const KEEPMARKERS: [&str; 3] = ["maplefjord", "zebraquill", "harborpine"];
    for (position, name) in ["alpha", "beta", "gamma"].into_iter().enumerate() {
        catalog
            .replace_file(
                &WorkspacePath::from_normalized(format!("src/{name}.rs")),
                Some("rust"),
                1,
                &format!("pub fn {name}_{}() {{}}\n", KEEPMARKERS[position]),
            )
            .expect("initial admission");
    }
    let index = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("persistent index");
    index
        .sync_snapshot(&catalog.snapshot().expect("initial snapshot"))
        .expect("initial generation");
    let initial = index.status();

    // Single-file edit: one changed chunk → bounded delta path. The
    // stale sentinel token exists only in the replaced text, so an OR
    // FTS query against it must return nothing after the delta.
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("src/beta.rs"),
            Some("rust"),
            2,
            "pub fn pinotnoir() {}\n",
        )
        .expect("edit");
    index
        .sync_snapshot(&catalog.snapshot().expect("edited snapshot"))
        .expect("delta generation");
    let edited = index.status();
    assert_ne!(edited.generation, initial.generation);
    let hits = index
        .search(&LexicalSearchRequest::new("pinotnoir"))
        .expect("edited query")
        .hits;
    assert_eq!(hits.len(), 1, "the edited chunk must be retrievable");
    assert!(hits[0].chunk.text.contains("pinotnoir"));
    // The removed posting must be gone: the stale sentinel token
    // existed only in the pre-edit beta text (FTS queries are OR across
    // terms, so the probe must be token-unique).
    assert!(
        index
            .search(&LexicalSearchRequest::new("zebraquill"))
            .expect("stale query")
            .hits
            .is_empty(),
        "the replaced posting must not survive the delta"
    );
    // Untouched chunks keep serving.
    assert_eq!(
        index
            .search(&LexicalSearchRequest::new("maplefjord"))
            .expect("kept query")
            .hits
            .len(),
        1
    );

    // Rename shape: alpha.rs → renamed.rs with identical content is a
    // remove+append pair (chunk ids hash the path).
    catalog
        .remove_file(&WorkspacePath::from_normalized("src/alpha.rs"), 3)
        .expect("remove old path");
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("src/renamed.rs"),
            Some("rust"),
            3,
            "pub fn maplefjord() {}\n",
        )
        .expect("admit renamed path");
    index
        .sync_snapshot(&catalog.snapshot().expect("renamed snapshot"))
        .expect("rename delta generation");
    let renamed_hits = index
        .search(&LexicalSearchRequest::new("maplefjord"))
        .expect("renamed query")
        .hits;
    assert_eq!(renamed_hits.len(), 1, "exactly the renamed file remains");
    assert!(renamed_hits[0].chunk.path.ends_with("renamed.rs"));

    // Reopen: the sparse v3 primary keys must reconstruct the exact
    // mapping (this is the misalignment class the pk field exists for).
    drop(index);
    let reopened = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("reopen after delta generations");
    assert_eq!(reopened.status().indexed_chunks, 3);
    let reopened_hits = reopened
        .search(&LexicalSearchRequest::new("maplefjord"))
        .expect("reopened query")
        .hits;
    assert_eq!(reopened_hits.len(), 1);
    assert!(reopened_hits[0].chunk.path.ends_with("renamed.rs"));
    assert_eq!(
        reopened
            .search(&LexicalSearchRequest::new("pinotnoir"))
            .expect("reopened edited query")
            .hits
            .len(),
        1
    );
}

#[cfg(feature = "a3s-vec-fts")]
#[test]
fn oversized_delta_and_missing_source_fall_back_to_full_builds() {
    let directory = tempfile::tempdir().expect("temporary index directory");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    const STALE: [&str; 10] = [
        "stalequilla",
        "stalequillb",
        "stalequillc",
        "stalequilld",
        "stalequille",
        "stalequillf",
        "stalequillg",
        "stalequillh",
        "stalequilli",
        "stalequillj",
    ];
    const FRESH: [&str; 10] = [
        "freshferna",
        "freshfernb",
        "freshfernc",
        "freshfernd",
        "freshferne",
        "freshfernf",
        "freshferng",
        "freshfernh",
        "freshferni",
        "freshfern",
    ];
    for (index, stale) in STALE.into_iter().enumerate() {
        catalog
            .replace_file(
                &WorkspacePath::from_normalized(format!("src/m{index:02}.rs")),
                Some("rust"),
                1,
                &format!("pub fn {stale}() {{}}\n"),
            )
            .expect("initial admission");
    }
    let index = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("persistent index");
    index
        .sync_snapshot(&catalog.snapshot().expect("initial snapshot"))
        .expect("initial generation");

    // Churn beyond the cap (> max(64, 30%)): the full build path must
    // still serve the new content.
    for (index, fresh) in FRESH.into_iter().enumerate() {
        catalog
            .replace_file(
                &WorkspacePath::from_normalized(format!("src/m{index:02}.rs")),
                Some("rust"),
                2,
                &format!("pub fn {fresh}() {{}}\n"),
            )
            .expect("churn admission");
    }
    index
        .sync_snapshot(&catalog.snapshot().expect("churned snapshot"))
        .expect("full rebuild after oversized churn");
    assert_eq!(
        index
            .search(&LexicalSearchRequest::new("freshfernf"))
            .expect("churned query")
            .hits
            .len(),
        1
    );
    assert!(index
        .search(&LexicalSearchRequest::new("stalequillf"))
        .expect("stale query")
        .hits
        .is_empty());

    // A missing delta source (in-memory state references a collection
    // that disappeared) must fail closed to a full build, not error out.
    let generation_dir = index
        .status()
        .generation
        .expect("current generation")
        .to_owned();
    std::fs::remove_dir_all(directory.path().join(&generation_dir))
        .expect("remove source collection");
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("src/m00.rs"),
            Some("rust"),
            3,
            "pub fn recoveredmoss() {}\n",
        )
        .expect("recovery edit");
    index
        .sync_snapshot(&catalog.snapshot().expect("recovery snapshot"))
        .expect("full build recovery after delta source loss");
    assert_eq!(
        index
            .search(&LexicalSearchRequest::new("recoveredmoss"))
            .expect("recovery query")
            .hits
            .len(),
        1
    );
}

#[test]
fn metadata_only_revision_reuses_the_published_generation() {
    let directory = tempfile::tempdir().expect("temporary index directory");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/lib.rs");
    catalog
        .replace_file(&path, Some("rust"), 1, "stable content sentinel\n")
        .expect("first replacement");
    let index = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("persistent index");
    index
        .sync_snapshot(&catalog.snapshot().expect("first snapshot"))
        .expect("first generation");
    let first = index.status();

    // Rechunk the same bytes under a newer source revision. The chunk
    // identity and FTS postings are unchanged, so a full native rebuild
    // would be pure update amplification.
    catalog
        .replace_file(&path, Some("rust"), 2, "stable content sentinel\n")
        .expect("same-content replacement");
    index
        .sync_snapshot(&catalog.snapshot().expect("second snapshot"))
        .expect("metadata-only update");
    let second = index.status();
    assert_eq!(second.generation, first.generation);
    assert_eq!(second.source_revision, 2);
    assert_eq!(second.catalog_revision, first.catalog_revision + 1);
    assert_eq!(second.indexed_chunks, first.indexed_chunks);
    assert_eq!(
        index
            .search(&LexicalSearchRequest::new("stable sentinel"))
            .expect("reused query")
            .hits
            .len(),
        1
    );
}

#[test]
fn published_replacement_collects_old_generations() {
    let directory = tempfile::tempdir().expect("temporary index directory");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    let path = WorkspacePath::from_normalized("src/lib.rs");
    let index = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("persistent index");

    catalog
        .replace_file(&path, Some("rust"), 1, "generation one sentinel\n")
        .expect("first replacement");
    index
        .sync_snapshot(&catalog.snapshot().expect("first snapshot"))
        .expect("first generation");
    let first_generation = index.status().generation.expect("first generation name");

    catalog
        .replace_file(&path, Some("rust"), 2, "generation two sentinel\n")
        .expect("second replacement");
    index
        .sync_snapshot(&catalog.snapshot().expect("second snapshot"))
        .expect("second generation");
    let second_generation = index.status().generation.expect("second generation name");
    assert_ne!(first_generation, second_generation);

    let generations = std::fs::read_dir(directory.path())
        .expect("persistent index directory")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("generation-"))
        })
        .map(|entry| entry.file_name())
        .collect::<Vec<_>>();
    assert_eq!(
        generations,
        vec![std::ffi::OsString::from(&second_generation)]
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("CURRENT")).expect("CURRENT"),
        format!("{}\n", second_generation)
    );
}

#[test]
fn corrupted_generation_manifest_is_rejected_before_query() {
    let directory = tempfile::tempdir().expect("temporary index directory");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("src/lib.rs"),
            Some("rust"),
            1,
            "durable manifest integrity sentinel\n",
        )
        .expect("catalog replacement");

    let index = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("persistent index");
    let snapshot = catalog.snapshot().expect("snapshot");
    index.sync_snapshot(&snapshot).expect("generation write");
    let generation = index.status().generation.expect("generation");
    drop(index);

    let manifest_path = directory.path().join(generation).join(MANIFEST_FILE);
    let bytes = std::fs::read(&manifest_path).expect("manifest");
    let mut manifest: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    manifest["chunks"][0]["text"] = serde_json::Value::String("tampered\n".to_owned());
    std::fs::write(
        &manifest_path,
        serde_json::to_vec(&manifest).expect("manifest json"),
    )
    .expect("tampered manifest");

    let reopened = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("reopen should remain recoverable");
    assert!(!reopened.is_ready());
    assert_eq!(
        reopened.status().phase,
        super::WorkspacePersistentIndexPhase::Absent
    );

    reopened
        .sync_snapshot(&snapshot)
        .expect("catalog snapshot rebuilds corrupted generation");
    let result = reopened
        .search(&LexicalSearchRequest::new("manifest integrity sentinel"))
        .expect("rebuilt query");
    assert_eq!(result.hits.len(), 1);
    assert_eq!(
        result.hits[0].chunk.text.as_ref(),
        "durable manifest integrity sentinel\n"
    );
}

#[test]
fn status_reports_building_without_hiding_the_last_published_generation() {
    let directory = tempfile::tempdir().expect("temporary index directory");
    let catalog = WorkspaceChunkCatalog::new_with_engine(
        ChunkingConfig::default(),
        ChunkCatalogLimits::default(),
        WorkspaceLexicalEngine::A3sVec,
    )
    .expect("catalog");
    catalog
        .replace_file(
            &WorkspacePath::from_normalized("src/lib.rs"),
            Some("rust"),
            1,
            "published status sentinel\n",
        )
        .expect("catalog replacement");
    let index = WorkspacePersistentIndex::open(directory.path(), WorkspaceLexicalEngine::A3sVec)
        .expect("persistent index");
    index
        .sync_snapshot(&catalog.snapshot().expect("snapshot"))
        .expect("generation");
    let ready = index.status();
    index
        .building
        .store(true, std::sync::atomic::Ordering::Release);
    let building = index.status();
    assert_eq!(
        building.phase,
        super::WorkspacePersistentIndexPhase::Building
    );
    assert_eq!(building.generation, ready.generation);
    assert_eq!(building.indexed_chunks, ready.indexed_chunks);
    index
        .building
        .store(false, std::sync::atomic::Ordering::Release);
}
