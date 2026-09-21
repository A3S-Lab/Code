use super::{prepare_documents, strip_windows_verbatim_prefix, A3sVecLexicalIndex};
use std::path::PathBuf;

#[test]
fn strip_windows_verbatim_prefix_covers_unc_and_drive_forms() {
    assert_eq!(
        strip_windows_verbatim_prefix(PathBuf::from(r"\\?\UNC\server\share\file.txt")),
        PathBuf::from(r"\\server\share\file.txt")
    );
    assert_eq!(
        strip_windows_verbatim_prefix(PathBuf::from(r"\\?\C:\Users\a3s\file.txt")),
        PathBuf::from(r"C:\Users\a3s\file.txt")
    );
    assert_eq!(
        strip_windows_verbatim_prefix(PathBuf::from("/tmp/plain")),
        PathBuf::from("/tmp/plain")
    );
}

#[test]
fn parallel_tokenization_preserves_document_order() {
    let documents = (0..128)
        .map(|index| {
            (
                format!("doc-{index:03}"),
                format!(
                    "workspace_parallel_marker_{index} {}",
                    "payload ".repeat(100)
                ),
            )
        })
        .collect::<Vec<_>>();
    let prepared = prepare_documents(&documents).expect("documents must tokenize");
    assert_eq!(prepared.len(), 128);
    assert!(prepared[0].tokens.contains(&"workspace".to_owned()));
    assert!(prepared[127].tokens.contains(&"127".to_owned()));
}

#[test]
fn rejects_invalid_or_duplicate_document_keys_before_engine_initialization() {
    assert!(matches!(
        A3sVecLexicalIndex::build(vec![("".to_owned(), "text".to_owned())]),
        Err(error) if error.contains("non-empty")
    ));
    assert!(matches!(
        A3sVecLexicalIndex::build(vec![("bad\0key".to_owned(), "text".to_owned())]),
        Err(error) if error.contains("NUL")
    ));
    assert!(matches!(
        A3sVecLexicalIndex::build(vec![("same".to_owned(), "first".to_owned()), ("same".to_owned(), "second".to_owned())]),
        Err(error) if error.contains("unique")
    ));
}

#[test]
fn builds_and_queries_a_multi_document_fts_partition() {
    let index = A3sVecLexicalIndex::build(vec![
        ("first".to_owned(), "cache invalidation policy".to_owned()),
        ("second".to_owned(), "cache expiry policy".to_owned()),
    ])
    .expect("a3s-vec FTS partition must build");
    let terms = ["cache".to_owned(), "invalidation".to_owned()];
    let hits = index
        .search(&terms, 2)
        .expect("a3s-vec FTS partition must query");
    assert!(!hits.is_empty());
    assert_eq!(hits[0].0, 0);
    assert!(hits[0].1.is_finite() && hits[0].1 > 0.0);
}

#[test]
fn ensure_initialized_short_circuits_when_engine_already_live() {
    // Prime the process-wide engine; bootstrap still calls initialize, which is
    // idempotent once the runtime bit is set.
    let _ = a3s_vec::initialize(None);
    super::bootstrap_a3s_vec_engine().expect("idempotent initialize must succeed");
    let index = A3sVecLexicalIndex::build(vec![(
        "doc".to_owned(),
        "already initialized engine".to_owned(),
    )])
    .expect("build after external initialize");
    assert_eq!(index.document_count(), 1);
}

#[test]
fn map_insert_write_result_rejects_nonzero_error_count() {
    let _ = a3s_vec::initialize(None);
    let temp = tempfile::tempdir().expect("temp");
    let collection_path = temp.path().join("dup-collection");
    std::fs::create_dir_all(&collection_path).expect("collection parent");
    let mut body =
        a3s_vec::FieldSchema::new("body", a3s_vec::DataType::String, false, 0).expect("body field");
    let fts = a3s_vec::IndexParams::fts(Some("whitespace"), None, None).expect("fts");
    body.set_index_params(&fts).expect("index params");
    let schema = a3s_vec::CollectionSchema::builder("workspace_lexical")
        .add_field(body)
        .build()
        .expect("schema");
    let path = collection_path.to_str().expect("utf-8 path");
    let collection =
        a3s_vec::Collection::create_and_open(path, &schema, None).expect("create collection");
    let mut doc = a3s_vec::Doc::new().expect("doc");
    doc.set_pk("d0");
    doc.add_string("body", "duplicate pk probe")
        .expect("body text");
    collection.insert(&[&doc]).expect("first insert");
    let rejected = collection
        .insert(&[&doc])
        .expect("duplicate insert returns a write result");
    assert!(rejected.error_count > 0);
    let error = super::map_insert_write_result(&rejected)
        .expect_err("nonzero error_count must fail closed");
    assert!(
        error.contains("insert rejected"),
        "unexpected insert mapping error: {error}"
    );
    let _ = collection.close();
}

#[test]
fn build_at_path_skips_parent_create_when_collection_root_has_no_parent() {
    // Path::new("") / PathBuf::new() report parent() == None, so the
    // create_dir_all branch is skipped before create_and_open fails.
    assert!(
        A3sVecLexicalIndex::build_at_path(
            std::path::Path::new(""),
            vec![("doc".to_owned(), "text".to_owned())]
        )
        .is_err(),
        "rootless collection path must fail closed"
    );
}

#[test]
fn empty_search_returns_no_hits_without_opening_the_collection() {
    let index = A3sVecLexicalIndex::build(vec![("doc".to_owned(), "cache policy".to_owned())])
        .expect("a3s-vec FTS partition must build");
    assert!(index
        .search(&[], 4)
        .expect("empty terms must succeed")
        .is_empty());
    assert!(index
        .search(&["cache".to_owned()], 0)
        .expect("zero limit must succeed")
        .is_empty());
}

#[test]
fn open_persistent_rejects_a_missing_collection_directory() {
    let missing = tempfile::tempdir()
        .expect("temp")
        .path()
        .join("does-not-exist");
    let result =
        A3sVecLexicalIndex::open_persistent(missing, vec![("doc".to_owned(), "text".to_owned())]);
    assert!(result.is_err(), "missing collection must fail");
    let error = result.err().unwrap();
    assert!(
        error.contains("does not exist"),
        "unexpected open_persistent error: {error}"
    );
}

#[cfg(unix)]
#[test]
fn open_persistent_rejects_an_unexpected_symlink_in_the_collection() {
    let temp = tempfile::tempdir().expect("temp");
    let collection = temp.path().join("collection");
    A3sVecLexicalIndex::build_at_path(
        &collection,
        vec![("doc".to_owned(), "symlink probe text".to_owned())],
    )
    .expect("build collection");
    let target = temp.path().join("outside.txt");
    std::fs::write(&target, b"outside").expect("outside file");
    std::os::unix::fs::symlink(&target, collection.join("evil-link")).expect("symlink");

    let result = A3sVecLexicalIndex::open_persistent(
        collection,
        vec![("doc".to_owned(), "symlink probe text".to_owned())],
    );
    assert!(
        result.is_err(),
        "symlink inside collection must fail closed"
    );
    let error = result.err().unwrap();
    assert!(
        error.contains("unexpected symlink"),
        "unexpected open_persistent error: {error}"
    );
}

#[cfg(unix)]
#[test]
fn open_persistent_tolerates_non_file_non_directory_collection_entries() {
    let temp = tempfile::tempdir().expect("temp");
    let collection = temp.path().join("collection");
    A3sVecLexicalIndex::build_at_path(
        &collection,
        vec![("doc".to_owned(), "fifo probe text".to_owned())],
    )
    .expect("build collection");
    let fifo = collection.join("odd.pipe");
    let fifo_c =
        std::ffi::CString::new(fifo.to_str().expect("utf-8 fifo path")).expect("fifo path cstring");
    // SAFETY: mkfifo only creates a named pipe under the temp collection.
    let status = unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o644) };
    assert_eq!(status, 0, "mkfifo must succeed for the fixture");

    let index = A3sVecLexicalIndex::open_persistent(
        collection,
        vec![("doc".to_owned(), "fifo probe text".to_owned())],
    )
    .expect("fifo entries contribute zero bytes and must not fail open");
    assert_eq!(index.document_count(), 1);
    assert!(index.estimated_bytes() > 0);
}

#[test]
fn large_corpus_build_indexes_many_documents() {
    let documents = (0..128)
        .map(|index| {
            (
                format!("doc-{index:03}"),
                format!(
                    "workspace_parallel_terms_{index} {}",
                    "payload ".repeat(128)
                ),
            )
        })
        .collect::<Vec<_>>();
    let index = A3sVecLexicalIndex::build(documents).expect("large parallel corpus must build");
    assert_eq!(index.document_count(), 128);
    assert!(index.has_any_term(&["workspace".to_owned()]));
    assert!(index.has_any_term(&["payload".to_owned()]));
}

#[test]
fn search_fails_closed_when_the_collection_directory_disappears() {
    let temp = tempfile::tempdir().expect("temp");
    let collection = temp.path().join("collection");
    let index = A3sVecLexicalIndex::build_at_path(
        &collection,
        vec![(
            "doc".to_owned(),
            "missing collection search probe".to_owned(),
        )],
    )
    .expect("build collection");
    std::fs::remove_dir_all(&collection).expect("remove collection");
    // Transient open uses `map_err(display_error)` rather than the cached-slot match.
    super::force_transient_collection_open_for_test(true);
    let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        index
            .search(&["missing".to_owned()], 2)
            .expect_err("search against a deleted collection must fail")
    }));
    super::force_transient_collection_open_for_test(false);
    let error = error.expect("search must not panic");
    assert!(!error.is_empty(), "open failure must surface a message");
}

#[test]
fn build_at_path_fails_when_the_parent_is_a_file() {
    let temp = tempfile::tempdir().expect("temp");
    let blocker = temp.path().join("not-a-directory");
    std::fs::write(&blocker, b"x").expect("blocker file");
    let result = A3sVecLexicalIndex::build_at_path(
        &blocker.join("collection"),
        vec![("doc".to_owned(), "parent is a file".to_owned())],
    );
    assert!(
        result.is_err(),
        "create_dir_all must fail when the parent path is a file"
    );
    assert!(
        !result.err().unwrap().is_empty(),
        "io failure must surface a message"
    );
}

#[test]
fn relocate_collection_path_updates_the_open_root() {
    let temp = tempfile::tempdir().expect("temp");
    let original = temp.path().join("original");
    let relocated = temp.path().join("relocated");
    let mut index = A3sVecLexicalIndex::build_at_path(
        &original,
        vec![("doc".to_owned(), "relocate probe text".to_owned())],
    )
    .expect("build collection");
    std::fs::rename(&original, &relocated).expect("rename collection");
    index.relocate_collection_path(relocated);
    let hits = index
        .search(&["relocate".to_owned()], 2)
        .expect("search after relocate");
    assert!(!hits.is_empty());
}

#[test]
fn search_opens_transiently_when_the_open_collection_cache_is_forced_closed() {
    let index =
        A3sVecLexicalIndex::build(vec![("doc".to_owned(), "transient open probe".to_owned())])
            .expect("build collection");
    super::force_transient_collection_open_for_test(true);
    let hits = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        index
            .search(&["transient".to_owned()], 2)
            .expect("transient search must succeed")
    }));
    super::force_transient_collection_open_for_test(false);
    let hits = hits.expect("transient search must not panic");
    assert!(!hits.is_empty());
    // Drop with a live cached collection after a normal (cached) search.
    let _ = index
        .search(&["transient".to_owned()], 2)
        .expect("cached search after transient");
}

#[test]
fn drop_recovers_from_a_poisoned_cached_collection_mutex() {
    let index = A3sVecLexicalIndex::build(vec![("doc".to_owned(), "poison drop probe".to_owned())])
        .expect("build collection");
    // Prime the cache so Drop has a live Collection to close after poison recovery.
    let _ = index
        .search(&["poison".to_owned()], 1)
        .expect("prime cached collection");
    index.poison_cached_collection_for_test();
    drop(index);
}

#[test]
fn search_rejects_limits_that_exceed_i32() {
    let index =
        A3sVecLexicalIndex::build(vec![("doc".to_owned(), "limit overflow probe".to_owned())])
            .expect("build collection");
    let error = index
        .search(&["limit".to_owned()], (i32::MAX as usize) + 1)
        .expect_err("limit above i32 must fail closed");
    assert!(error.contains("i32"), "unexpected limit error: {error}");
}

#[test]
fn map_query_documents_rejects_unknown_or_omitted_primary_keys() {
    let mut ordinals = std::collections::HashMap::new();
    ordinals.insert("d0".to_owned(), 0usize);
    let mut good = a3s_vec::Doc::new().expect("doc");
    good.set_pk("d0");
    good.set_score(1.25).expect("score");
    let mut unknown = a3s_vec::Doc::new().expect("doc");
    unknown.set_pk("missing");
    unknown.set_score(1.0).expect("score");
    let mut missing_pk = a3s_vec::Doc::new().expect("doc");
    missing_pk.set_score(1.0).expect("score");
    let hits = super::map_query_documents(&ordinals, &[good]).expect("mapped hits");
    assert_eq!(hits, vec![(0, 1.25)]);
    let error = super::map_query_documents(&ordinals, &[unknown])
        .expect_err("unknown primary key must fail");
    assert!(error.contains("unknown primary key"), "unexpected: {error}");
    let error = super::map_query_documents(&ordinals, &[missing_pk])
        .expect_err("omitted primary key must fail");
    assert!(error.contains("primary key"), "unexpected: {error}");
}

#[test]
fn has_any_term_is_false_for_missing_tokens() {
    let index = A3sVecLexicalIndex::build(vec![("doc".to_owned(), "alpha beta".to_owned())])
        .expect("build collection");
    assert!(!index.has_any_term(&["gamma".to_owned()]));
    assert_eq!(index.estimated_bytes(), index.estimated_bytes());
}
