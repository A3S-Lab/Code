//! Pure-Rust `a3s-vec` full-text adapter for workspace lexical retrieval.
//!
//! This module is gated by the `a3s-vec-fts` feature. Workspace admission,
//! chunk identity, and source verification remain owned by the surrounding
//! Code catalog. The adapter only indexes admitted chunk text and returns
//! bounded ranked ordinals.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use a3s_vec::{
    Collection, CollectionOptions, CollectionSchema, DataType, Doc, FieldSchema, Fts, IndexParams,
    SearchQuery,
};
use rayon::prelude::*;
use tempfile::TempDir;

const INSERT_BATCH_SIZE: usize = 256;
const ESTIMATED_DOCUMENT_OVERHEAD: usize = 64;
/// Bound how many hot read-only collections stay open across partitions.
const MAX_OPEN_COLLECTIONS: usize = 4;

fn strip_windows_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let value = path.to_string_lossy();
        if let Some(stripped) = value.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{stripped}"));
        }
        if let Some(stripped) = value.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
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

fn ensure_initialized() -> Result<(), String> {
    INITIALIZATION
        .get_or_init(|| {
            if a3s_vec::is_initialized() {
                return Ok(());
            }
            a3s_vec::initialize(None).map_err(|error| error.to_string())
        })
        .clone()
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
    pub(crate) fn build<I, K, T>(documents: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = (K, T)>,
        K: AsRef<str> + Send + Sync,
        T: AsRef<str> + Send + Sync,
    {
        let temp_dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let collection_path = temp_dir.path().join("collection");
        let mut index = Self::build_at_path(&collection_path, documents)?;
        index._temp_dir = Some(temp_dir);
        Ok(index)
    }

    /// Build an FTS collection at a caller-owned path.
    ///
    /// The caller publishes the containing generation atomically. The returned
    /// handle is closed between queries so the path can be renamed afterward.
    pub(crate) fn build_at_path<I, K, T>(
        collection_root: &std::path::Path,
        documents: I,
    ) -> Result<Self, String>
    where
        I: IntoIterator<Item = (K, T)>,
        K: AsRef<str> + Send + Sync,
        T: AsRef<str> + Send + Sync,
    {
        let prepared = prepare_documents(documents)?;
        let terms = collect_terms(&prepared);

        ensure_initialized()?;

        let mut body = FieldSchema::new("body", DataType::String, false, 0)
            .map_err(|error| error.to_string())?;
        let fts =
            IndexParams::fts(Some("whitespace"), None, None).map_err(|error| error.to_string())?;
        body.set_index_params(&fts)
            .map_err(|error| error.to_string())?;
        let schema = CollectionSchema::builder("workspace_lexical")
            .add_field(body)
            .build()
            .map_err(|error| error.to_string())?;

        if let Some(parent) = collection_root.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let collection_path = strip_windows_verbatim_prefix(collection_root.to_path_buf())
            .to_str()
            .ok_or_else(|| "a3s-vec lexical path is not UTF-8".to_owned())?
            .to_owned();
        let collection = Collection::create_and_open(&collection_path, &schema, None)
            .map_err(|error| error.to_string())?;

        let mut native_ordinals = HashMap::with_capacity(prepared.len());
        let mut next_ordinal = 0usize;
        for batch in prepared.chunks(INSERT_BATCH_SIZE) {
            let mut docs = Vec::with_capacity(batch.len());
            for prepared_document in batch {
                let ordinal = next_ordinal;
                next_ordinal = next_ordinal.saturating_add(1);
                let native_key = format!("d{ordinal}");
                native_ordinals.insert(native_key.clone(), ordinal);
                let mut native_document = Doc::new().map_err(|error| error.to_string())?;
                native_document.set_pk(&native_key);
                native_document
                    .add_string("body", &prepared_document.normalized)
                    .map_err(|error| error.to_string())?;
                docs.push(native_document);
            }
            let references = docs.iter().collect::<Vec<_>>();
            let result = collection
                .insert(&references)
                .map_err(|error| error.to_string())?;
            if result.error_count != 0 {
                return Err(format!(
                    "a3s-vec lexical insert rejected {} document(s)",
                    result.error_count
                ));
            }
        }

        collection.flush().map_err(|error| error.to_string())?;
        collection.close().map_err(|error| error.to_string())?;

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
    pub(crate) fn open_persistent<I, K, T>(
        collection_root: PathBuf,
        documents: I,
    ) -> Result<Self, String>
    where
        I: IntoIterator<Item = (K, T)>,
        K: AsRef<str> + Send + Sync,
        T: AsRef<str> + Send + Sync,
    {
        if !collection_root.is_dir() {
            return Err(format!(
                "a3s-vec lexical collection does not exist: {}",
                collection_root.display()
            ));
        }
        ensure_initialized()?;
        let mut native_ordinals = HashMap::new();
        let prepared = prepare_documents(documents)?;
        let terms = collect_terms(&prepared);
        for document_count in 0..prepared.len() {
            native_ordinals.insert(format!("d{document_count}"), document_count);
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
        let mut fts = Fts::new().map_err(|error| error.to_string())?;
        fts.set_match_string(&terms.join(" "))
            .map_err(|error| error.to_string())?;
        let topk =
            i32::try_from(limit).map_err(|_| "lexical result limit exceeds i32".to_owned())?;
        let mut query = SearchQuery::fts("body", &fts, topk).map_err(|error| error.to_string())?;
        query
            .set_output_fields(&[])
            .map_err(|error| error.to_string())?;
        let mut options = CollectionOptions::new().map_err(|error| error.to_string())?;
        options
            .set_read_only(true)
            .map_err(|error| error.to_string())?;
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
            (
                collection
                    .query(&query)
                    .map_err(|error| error.to_string())?,
                None,
            )
        } else {
            drop(cached_slot.take());
            let collection = Collection::open(collection_path, Some(&options))
                .map_err(|error| error.to_string())?;
            let documents = collection
                .query(&query)
                .map_err(|error| error.to_string())?;
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

fn prepare_documents<I, K, T>(documents: I) -> Result<Vec<PreparedLexicalDocument>, String>
where
    I: IntoIterator<Item = (K, T)>,
    K: AsRef<str> + Send + Sync,
    T: AsRef<str> + Send + Sync,
{
    let documents = documents.into_iter().collect::<Vec<_>>();
    let mut seen_keys = HashSet::with_capacity(documents.len());
    for (key, _) in &documents {
        let key = key.as_ref();
        if key.is_empty() || key.contains('\0') {
            return Err("lexical document key must be non-empty and contain no NUL byte".into());
        }
        if !seen_keys.insert(key) {
            return Err("lexical document keys must be unique".into());
        }
    }

    let parallel = super::lexical::should_parallelize_build(
        documents.len(),
        documents.iter().fold(0usize, |total, (_, text)| {
            total.saturating_add(text.as_ref().len())
        }),
    );
    if parallel {
        Ok(documents
            .par_iter()
            .filter_map(|(_, text)| prepare_document(text.as_ref()))
            .collect())
    } else {
        Ok(documents
            .iter()
            .filter_map(|(_, text)| prepare_document(text.as_ref()))
            .collect())
    }
}

fn prepare_document(text: &str) -> Option<PreparedLexicalDocument> {
    let tokens = super::lexical::tokenize(text);
    (!tokens.is_empty()).then(|| PreparedLexicalDocument {
        normalized: tokens.join(" "),
        tokens,
    })
}

fn collect_terms(prepared: &[PreparedLexicalDocument]) -> HashSet<String> {
    let parallel = super::lexical::should_parallelize_build(
        prepared.len(),
        prepared.iter().fold(0usize, |total, document| {
            total.saturating_add(document.normalized.len())
        }),
    );
    if parallel {
        prepared
            .par_iter()
            .flat_map_iter(|document| document.tokens.iter().cloned())
            .collect()
    } else {
        prepared
            .iter()
            .flat_map(|document| document.tokens.iter().cloned())
            .collect()
    }
}

fn reserve_open_collection() -> bool {
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
        let score = f64::from(document.get_score());
        if score.is_finite() {
            hits.push((ordinal, score));
        }
    }
    Ok(hits)
}

fn directory_size(root: &std::path::Path) -> Result<usize, String> {
    fn visit(path: &std::path::Path) -> Result<usize, String> {
        let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "a3s-vec lexical collection contains an unexpected symlink: {}",
                path.display()
            ));
        }
        if metadata.is_file() {
            return usize::try_from(metadata.len())
                .map_err(|_| "a3s-vec lexical file size exceeds usize".to_owned());
        }
        if !metadata.is_dir() {
            return Ok(0);
        }
        let mut total = 0usize;
        for entry in fs::read_dir(path).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            total = total
                .checked_add(visit(&entry.path())?)
                .ok_or_else(|| "a3s-vec lexical directory size overflows usize".to_owned())?;
        }
        Ok(total)
    }

    visit(root)
}

#[cfg(test)]
mod tests {
    use super::{prepare_documents, A3sVecLexicalIndex};

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
        let prepared = prepare_documents(documents).expect("documents must tokenize");
        assert_eq!(prepared.len(), 128);
        assert!(prepared[0].tokens.contains(&"workspace".to_owned()));
        assert!(prepared[127].tokens.contains(&"127".to_owned()));
    }

    #[test]
    fn rejects_invalid_or_duplicate_document_keys_before_engine_initialization() {
        assert!(matches!(
            A3sVecLexicalIndex::build([("", "text")]),
            Err(error) if error.contains("non-empty")
        ));
        assert!(matches!(
            A3sVecLexicalIndex::build([("bad\0key", "text")]),
            Err(error) if error.contains("NUL")
        ));
        assert!(matches!(
            A3sVecLexicalIndex::build([("same", "first"), ("same", "second")]),
            Err(error) if error.contains("unique")
        ));
    }

    #[test]
    fn builds_and_queries_a_multi_document_fts_partition() {
        let index = A3sVecLexicalIndex::build([
            ("first", "cache invalidation policy"),
            ("second", "cache expiry policy"),
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
}
