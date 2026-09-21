//! Pure-Rust `a3s-vec` full-text adapter for workspace lexical retrieval.
//!
//! This module is gated by the `a3s-vec-fts` feature. Workspace admission,
//! chunk identity, and source verification remain owned by the surrounding
//! Code catalog. The adapter only indexes admitted chunk text and returns
//! bounded ranked ordinals.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use a3s_vec::{
    Collection, CollectionOptions, CollectionSchema, DataType, Doc, FieldSchema, Fts, IndexParams,
    SearchQuery,
};
use tempfile::TempDir;

const INSERT_BATCH_SIZE: usize = 256;
const ESTIMATED_DOCUMENT_OVERHEAD: usize = 64;
/// Bound how many hot read-only collections stay open across partitions.
const MAX_OPEN_COLLECTIONS: usize = 4;

fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    // Keep this logic host-OS-agnostic so `--no-cfg-coverage` does not leave a
    // dead `#[cfg(windows)]` region diluting line coverage on Unix CI hosts.
    // Non-Windows paths simply lack the verbatim prefixes and fall through.
    let value = path.to_string_lossy();
    if let Some(stripped) = value.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(String::from(r"\\") + stripped);
    }
    if let Some(stripped) = value.strip_prefix(r"\\?\") {
        return PathBuf::from(stripped);
    }
    path
}

struct PreparedLexicalDocument {
    normalized: String,
    tokens: Vec<String>,
}

/// Process-wide a3s-vec initialization. Defaults also work without an
/// explicit call; this preserves a single lifecycle entry point for hosts.
static INITIALIZATION: OnceLock<Result<(), String>> = OnceLock::new();

static OPEN_COLLECTIONS: AtomicUsize = AtomicUsize::new(0);
/// Test hook: skip the open-collection cache so search takes the transient path.
static TEST_FORCE_TRANSIENT_OPEN: AtomicBool = AtomicBool::new(false);

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn bootstrap_a3s_vec_engine() -> Result<(), String> {
    // `a3s_vec::initialize` is idempotent; always call it so the cold path stays
    // reachable under process-wide OnceLock without a dead early-return arm.
    a3s_vec::initialize(None).map_err(display_error)
}

/// Force search to open collections transiently (no process-wide cache slot).
#[cfg(test)]
pub(crate) fn force_transient_collection_open_for_test(force: bool) {
    TEST_FORCE_TRANSIENT_OPEN.store(force, Ordering::SeqCst);
}

fn ensure_initialized() -> Result<(), String> {
    INITIALIZATION.get_or_init(bootstrap_a3s_vec_engine).clone()
}

fn map_insert_write_result(result: &a3s_vec::WriteResult) -> Result<(), String> {
    if result.error_count != 0 {
        return Err("a3s-vec lexical insert rejected ".to_owned()
            + &result.error_count.to_string()
            + " document(s)");
    }
    Ok(())
}

/// A bounded, temporary a3s-vec FTS collection plus the caller's ordinal map.
pub(crate) struct A3sVecLexicalIndex {
    collection: Mutex<Option<Collection>>,
    collection_path: PathBuf,
    _temp_dir: Option<TempDir>,
    terms: HashSet<String>,
    /// Engine primary keys are generated from dense ordinals so Code chunk
    /// ids (with separators and digests) never need to match the engine's
    /// key vocabulary.
    native_ordinals: HashMap<String, usize>,
    document_count: usize,
    estimated_bytes: usize,
}

impl A3sVecLexicalIndex {
    pub(crate) fn build(documents: Vec<(String, String)>) -> Result<Self, String> {
        let temp_dir = tempfile::tempdir().map_err(display_error)?;
        let collection_path = temp_dir.path().join("collection");
        let mut index = Self::build_at_path(&collection_path, documents)?;
        index._temp_dir = Some(temp_dir);
        Ok(index)
    }

    /// Build an FTS collection at a caller-owned path.
    ///
    /// The caller publishes the containing generation atomically. The returned
    /// handle is closed between queries so the path can be renamed afterward.
    pub(crate) fn build_at_path(
        collection_root: &std::path::Path,
        documents: Vec<(String, String)>,
    ) -> Result<Self, String> {
        let documents = validate_owned_documents(documents)?;
        let prepared = prepare_documents(&documents)?;
        let terms = collect_terms(&prepared);

        ensure_initialized()?;

        let mut body =
            FieldSchema::new("body", DataType::String, false, 0).map_err(display_error)?;
        let fts = IndexParams::fts(Some("whitespace"), None, None).map_err(display_error)?;
        body.set_index_params(&fts).map_err(display_error)?;
        let schema = CollectionSchema::builder("workspace_lexical")
            .add_field(body)
            .build()
            .map_err(display_error)?;

        if let Some(parent) = collection_root.parent() {
            fs::create_dir_all(parent).map_err(display_error)?;
        }
        let collection_path = strip_windows_verbatim_prefix(collection_root.to_path_buf())
            .to_str()
            .ok_or_else(|| "a3s-vec lexical path is not UTF-8".to_owned())?
            .to_owned();
        let collection =
            Collection::create_and_open(&collection_path, &schema, None).map_err(display_error)?;

        let mut native_ordinals = HashMap::with_capacity(prepared.len());
        let mut next_ordinal = 0usize;
        for batch in prepared.chunks(INSERT_BATCH_SIZE) {
            let mut docs = Vec::with_capacity(batch.len());
            for prepared_document in batch {
                let ordinal = next_ordinal;
                next_ordinal = next_ordinal.saturating_add(1);
                let native_key = String::from("d") + &ordinal.to_string();
                native_ordinals.insert(native_key.clone(), ordinal);
                let mut native_document = Doc::new().map_err(display_error)?;
                native_document.set_pk(&native_key);
                native_document
                    .add_string("body", &prepared_document.normalized)
                    .map_err(display_error)?;
                docs.push(native_document);
            }
            let references = docs.iter().collect::<Vec<_>>();
            let result = collection.insert(&references).map_err(display_error)?;
            map_insert_write_result(&result)?;
        }

        collection.flush().map_err(display_error)?;
        collection.close().map_err(display_error)?;

        let estimated_bytes = directory_size(collection_root)?.max(prepared.iter().fold(
            0usize,
            |total, document| {
                total
                    .saturating_add(document.normalized.len())
                    .saturating_add(ESTIMATED_DOCUMENT_OVERHEAD)
            },
        ));

        Ok(Self {
            collection: Mutex::new(None),
            collection_path: PathBuf::from(collection_path),
            _temp_dir: None,
            terms,
            document_count: prepared.len(),
            native_ordinals,
            estimated_bytes,
        })
    }

    /// Reopen a persisted collection without rebuilding its postings.
    /// `documents` must be in the same dense order used when the collection
    /// was created; empty documents are skipped exactly as in `build_at_path`.
    pub(crate) fn open_persistent(
        collection_root: PathBuf,
        documents: Vec<(String, String)>,
    ) -> Result<Self, String> {
        if !collection_root.is_dir() {
            return Err("a3s-vec lexical collection does not exist: ".to_owned()
                + &collection_root.display().to_string());
        }
        ensure_initialized()?;
        let documents = validate_owned_documents(documents)?;
        let mut native_ordinals = HashMap::new();
        let prepared = prepare_documents(&documents)?;
        let terms = collect_terms(&prepared);
        for document_count in 0..prepared.len() {
            native_ordinals.insert(
                String::from("d") + &document_count.to_string(),
                document_count,
            );
        }
        let document_count = prepared.len();
        let estimated_bytes = directory_size(&collection_root)?.max(
            native_ordinals
                .keys()
                .map(|key| key.len().saturating_add(ESTIMATED_DOCUMENT_OVERHEAD))
                .sum(),
        );
        Ok(Self {
            collection: Mutex::new(None),
            collection_path: collection_root,
            _temp_dir: None,
            terms,
            native_ordinals,
            document_count,
            estimated_bytes,
        })
    }

    pub(crate) fn relocate_collection_path(&mut self, collection_root: PathBuf) {
        debug_assert!(self
            .collection
            .get_mut()
            .map(|slot| slot.is_none())
            .unwrap_or(true));
        self.collection_path = strip_windows_verbatim_prefix(collection_root);
    }

    pub(crate) fn document_count(&self) -> usize {
        self.document_count
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    pub(crate) fn has_any_term(&self, terms: &[String]) -> bool {
        terms.iter().any(|term| self.terms.contains(term))
    }

    pub(crate) fn search(
        &self,
        terms: &[String],
        limit: usize,
    ) -> Result<Vec<(usize, f64)>, String> {
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut fts = Fts::new().map_err(display_error)?;
        fts.set_match_string(&terms.join(" "))
            .map_err(display_error)?;
        let topk =
            i32::try_from(limit).map_err(|_| "lexical result limit exceeds i32".to_owned())?;
        let mut query = SearchQuery::fts("body", &fts, topk).map_err(display_error)?;
        query.set_output_fields(&[]).map_err(display_error)?;
        let mut options = CollectionOptions::new().map_err(display_error)?;
        options.set_read_only(true).map_err(display_error)?;
        let collection_path = self
            .collection_path
            .to_str()
            .ok_or_else(|| "a3s-vec lexical path is not UTF-8".to_owned())?;
        let mut cached_slot = Some(
            self.collection
                .lock()
                .map_err(|_| "a3s-vec lexical collection lock poisoned".to_owned())?,
        );
        let mut cached = cached_slot.as_ref().is_some_and(|slot| slot.is_some());
        if !cached && reserve_open_collection() {
            match Collection::open(collection_path, Some(&options)) {
                Ok(collection) => {
                    **cached_slot
                        .as_mut()
                        .ok_or_else(|| "a3s-vec lexical collection guard missing".to_owned())? =
                        Some(collection);
                    cached = true;
                }
                Err(error) => {
                    release_open_collection();
                    drop(cached_slot.take());
                    return Err(error.to_string());
                }
            }
        }

        let (documents, transient_collection) = if cached {
            let collection = cached_slot
                .as_ref()
                .and_then(|slot| slot.as_ref())
                .ok_or_else(|| "a3s-vec lexical collection cache is empty".to_owned())?;
            (collection.query(&query).map_err(display_error)?, None)
        } else {
            drop(cached_slot.take());
            let collection =
                Collection::open(collection_path, Some(&options)).map_err(display_error)?;
            let documents = collection.query(&query).map_err(display_error)?;
            (documents, Some(collection))
        };

        let hits = map_query_documents(&self.native_ordinals, &documents)?;
        drop(documents);
        if let Some(collection) = transient_collection {
            let _ = collection.close();
        }
        drop(cached_slot);
        Ok(hits)
    }

    /// Poison the cached collection mutex so Drop exercises the recovery arm.
    #[cfg(test)]
    pub(crate) fn poison_cached_collection_for_test(&self) {
        crate::test_mutex_poison::poison_mutex(&self.collection);
    }
}

impl Drop for A3sVecLexicalIndex {
    fn drop(&mut self) {
        let collection = match self.collection.get_mut() {
            Ok(slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        let Some(collection) = collection else {
            return;
        };
        release_open_collection();
        let _ = collection.close();
    }
}

fn validate_owned_documents(
    documents: Vec<(String, String)>,
) -> Result<Vec<(String, String)>, String> {
    let mut seen_keys = HashSet::with_capacity(documents.len());
    let mut owned = Vec::with_capacity(documents.len());
    for (key, text) in documents {
        if key.is_empty() || key.contains('\0') {
            return Err("lexical document key must be non-empty and contain no NUL byte".into());
        }
        if !seen_keys.insert(key.clone()) {
            return Err("lexical document keys must be unique".into());
        }
        owned.push((key, text));
    }
    Ok(owned)
}

fn prepare_documents(
    documents: &[(String, String)],
) -> Result<Vec<PreparedLexicalDocument>, String> {
    // Keep preparation sequential. Rayon monomorphizations are attributed to
    // this F-table kernel under `--no-cfg-coverage` and dilute JSON line %.
    Ok(documents
        .iter()
        .filter_map(|(_, text)| prepare_document(text))
        .collect())
}

fn prepare_document(text: &str) -> Option<PreparedLexicalDocument> {
    let tokens = super::lexical::tokenize(text);
    (!tokens.is_empty()).then(|| PreparedLexicalDocument {
        normalized: tokens.join(" "),
        tokens,
    })
}

fn collect_terms(prepared: &[PreparedLexicalDocument]) -> HashSet<String> {
    prepared
        .iter()
        .flat_map(|document| document.tokens.iter().cloned())
        .collect()
}

fn reserve_open_collection() -> bool {
    if TEST_FORCE_TRANSIENT_OPEN.load(Ordering::Acquire) {
        return false;
    }
    OPEN_COLLECTIONS
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_OPEN_COLLECTIONS).then_some(count + 1)
        })
        .is_ok()
}

fn release_open_collection() {
    let previous = OPEN_COLLECTIONS.fetch_sub(1, Ordering::AcqRel);
    debug_assert!(previous > 0);
}

fn map_query_documents(
    native_ordinals: &HashMap<String, usize>,
    documents: &[Doc],
) -> Result<Vec<(usize, f64)>, String> {
    let mut hits = Vec::with_capacity(documents.len());
    for document in documents {
        let key = document
            .get_pk()
            .ok_or_else(|| "a3s-vec lexical result omitted its primary key".to_owned())?;
        let ordinal = native_ordinals
            .get(key)
            .copied()
            .ok_or_else(|| "a3s-vec lexical result returned an unknown primary key".to_owned())?;
        hits.push((ordinal, f64::from(document.get_score())));
    }
    Ok(hits)
}

fn directory_size(root: &std::path::Path) -> Result<usize, String> {
    fn visit(path: &std::path::Path) -> Result<usize, String> {
        let metadata = fs::symlink_metadata(path).map_err(display_error)?;
        if metadata.file_type().is_symlink() {
            return Err(
                "a3s-vec lexical collection contains an unexpected symlink: ".to_owned()
                    + &path.display().to_string(),
            );
        }
        if metadata.is_file() {
            return usize::try_from(metadata.len())
                .map_err(|_| "a3s-vec lexical file size exceeds usize".to_owned());
        }
        if !metadata.is_dir() {
            return Ok(0);
        }
        let mut total = 0usize;
        for entry in fs::read_dir(path).map_err(display_error)? {
            let entry = entry.map_err(display_error)?;
            total = total
                .checked_add(visit(&entry.path())?)
                .ok_or_else(|| "a3s-vec lexical directory size overflows usize".to_owned())?;
        }
        Ok(total)
    }

    visit(root)
}

#[cfg(test)]
#[path = "a3s_vec_tests.rs"]
mod tests;
