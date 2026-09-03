use super::catalog::ChunkCatalogSnapshot;
use super::types::{WorkspaceChunk, WorkspaceIndexError, WorkspaceIndexResult};
use crate::workspace::WorkspacePath;
use a3s_vec::{
    Collection, CollectionOptions, CollectionSchema, DataType, Doc, Durability, FieldSchema, Fts,
    IndexParams, SearchQuery,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tempfile::TempDir;

const DEFAULT_QUERY_TERM_LIMIT: usize = 32;
const DEFAULT_CANDIDATE_FILE_LIMIT: usize = 256;
const DEFAULT_RESULT_LIMIT: usize = 10;
const MAX_RESULT_LIMIT: usize = 25;
const MAX_QUERY_BYTES: usize = 2_048;
const DEFAULT_RESULTS_PER_FILE: usize = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalSearchRequest {
    pub query: String,
    pub path: WorkspacePath,
    pub glob: Option<String>,
    pub limit: usize,
    pub max_candidate_files: usize,
    pub max_results_per_file: usize,
}

impl LexicalSearchRequest {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            path: WorkspacePath::root(),
            glob: None,
            limit: DEFAULT_RESULT_LIMIT,
            max_candidate_files: DEFAULT_CANDIDATE_FILE_LIMIT,
            max_results_per_file: DEFAULT_RESULTS_PER_FILE,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LexicalSearchHit {
    pub chunk: Arc<WorkspaceChunk>,
    pub score: f64,
}

#[derive(Clone, Debug)]
pub struct LexicalSearchResult {
    pub catalog_revision: u64,
    pub source_revision: u64,
    pub query_terms: Vec<String>,
    pub matching_files: usize,
    pub selected_files: usize,
    pub scored_chunks: usize,
    pub candidate_truncated: bool,
    pub hits: Vec<LexicalSearchHit>,
}

/// One session-local A3S Vec FTS projection.
///
/// The caller owns admission, chunking, and source verification. This helper
/// owns only the token postings and BM25 score calculation. It is shared by
/// the incremental catalog and the bounded query-time compatibility path so
/// there is no second Code-local BM25 implementation.
pub(crate) struct VecLexicalIndex {
    collection: Collection,
    // Keep the temporary directory alive for the collection handle. The
    // collection field is declared first so it is released before the
    // directory during normal Rust drop order.
    _temp_dir: TempDir,
    terms: HashSet<String>,
    ordinals: HashMap<String, usize>,
    document_count: usize,
    estimated_bytes: usize,
}

impl VecLexicalIndex {
    /// Build an index from stable keys and source text.
    ///
    /// `K` and `T` deliberately accept both borrowed and owned values. The
    /// collection copies normalized tokens into its own documents, while the
    /// caller can retain its source text without coupling it to the index.
    pub(crate) fn build<I, K, T>(documents: I) -> Result<Self, a3s_vec::Error>
    where
        I: IntoIterator<Item = (K, T)>,
        K: AsRef<str>,
        T: AsRef<str>,
    {
        let mut body = FieldSchema::new("body", DataType::String, false, 0)?;
        let fts = IndexParams::fts(Some("whitespace"), None, None)?;
        body.set_index_params(&fts)?;
        let schema = CollectionSchema::builder("workspace_lexical")
            .add_field(body)
            .build()?;
        let temp_dir = tempfile::tempdir()?;
        let collection_path = temp_dir
            .path()
            .join("collection")
            .to_str()
            .ok_or_else(|| a3s_vec::Error::invalid_argument("lexical path is not UTF-8"))?
            .to_owned();
        let mut options = CollectionOptions::new()?;
        options.set_durability(Durability::Manual)?;
        let collection = Collection::create(&collection_path, &schema, Some(&options))?;

        let mut docs = Vec::new();
        let mut terms = HashSet::new();
        let mut ordinals = HashMap::new();
        for (key, text) in documents {
            let key = key.as_ref();
            if key.is_empty() || key.contains('\0') {
                return Err(a3s_vec::Error::invalid_argument(
                    "lexical document key must be non-empty and contain no NUL byte",
                ));
            }
            if ordinals.contains_key(key) {
                return Err(a3s_vec::Error::invalid_argument(
                    "lexical document keys must be unique",
                ));
            }
            let tokens = tokenize(text.as_ref());
            if tokens.is_empty() {
                continue;
            }
            // Keep the caller ordinal dense over indexed documents. Empty
            // source chunks are intentionally omitted from the FTS
            // collection, so using the input position here would make a
            // later non-empty chunk resolve to the wrong source chunk.
            let indexed_ordinal = ordinals.len();
            ordinals.insert(key.to_owned(), indexed_ordinal);
            terms.extend(tokens.iter().cloned());
            let mut document = Doc::with_pk(key)?;
            document.add_string("body", &tokens.join(" "))?;
            docs.push(document);
        }
        if !docs.is_empty() {
            let references = docs.iter().collect::<Vec<_>>();
            let result = collection.insert(&references)?;
            if result.error_count != 0 {
                return Err(a3s_vec::Error::failed_precondition(format!(
                    "lexical document insert rejected {} document(s)",
                    result.error_count
                )));
            }
        }
        let estimated_bytes =
            usize::try_from(collection.stats()?.accounted_bytes).unwrap_or(usize::MAX);
        Ok(Self {
            collection,
            _temp_dir: temp_dir,
            terms,
            document_count: ordinals.len(),
            ordinals,
            estimated_bytes,
        })
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

    /// Search and return `(caller_ordinal, score)` pairs.
    pub(crate) fn search(
        &self,
        terms: &[String],
        limit: usize,
    ) -> Result<Vec<(usize, f64)>, a3s_vec::Error> {
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut fts = Fts::new()?;
        fts.set_match_string(&terms.join(" "))?;
        let topk = i32::try_from(limit)
            .map_err(|_| a3s_vec::Error::invalid_argument("lexical result limit exceeds i32"))?;
        let mut query = SearchQuery::fts("body", &fts, topk)?;
        query.set_output_fields(&[])?;
        let documents = self.collection.query(&query)?;
        Ok(documents
            .into_iter()
            .filter_map(|document| {
                let key = document.get_pk()?;
                let ordinal = *self.ordinals.get(key)?;
                Some((ordinal, f64::from(document.get_score())))
            })
            .collect())
    }
}

pub(crate) struct LexicalPartition {
    index: VecLexicalIndex,
    chunks: Arc<[Arc<WorkspaceChunk>]>,
    pub(crate) document_count: usize,
}

impl LexicalPartition {
    /// Build the catalog's lexical partition through the A3S Vec FTS API.
    ///
    /// Code still owns chunk admission and path policy, while tokenization,
    /// postings, BM25 statistics, and deterministic score ordering are owned
    /// by the same engine used by the standalone Vec crate. The collection is
    /// deliberately temporary: workspace source remains authoritative and no
    /// durable SQLite/sqlite-vec path is introduced by lexical search.
    pub(crate) fn build(chunks: Arc<[Arc<WorkspaceChunk>]>) -> WorkspaceIndexResult<Self> {
        let indexed_chunks: Arc<[Arc<WorkspaceChunk>]> = Arc::from(
            chunks
                .iter()
                .filter(|chunk| !tokenize(chunk.text.as_ref()).is_empty())
                .cloned()
                .collect::<Vec<_>>(),
        );
        let index = VecLexicalIndex::build(
            indexed_chunks
                .iter()
                .map(|chunk| (chunk.id.as_str(), chunk.text.as_ref())),
        )
        .map_err(|error| lexical_build_error("index", error))?;
        let document_count = index.document_count();
        Ok(Self {
            index,
            chunks: indexed_chunks,
            document_count,
        })
    }

    fn has_any_term(&self, terms: &[String]) -> bool {
        self.index.has_any_term(terms)
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        self.index.estimated_bytes()
    }

    fn search(
        &self,
        terms: &[String],
        limit: usize,
    ) -> WorkspaceIndexResult<Vec<(Arc<WorkspaceChunk>, f64)>> {
        if terms.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        self.index
            .search(terms, limit)
            .map_err(|error| lexical_query_error("FTS search", error))
            .map(|hits| {
                hits.into_iter()
                    .filter_map(|(ordinal, score)| {
                        self.chunks
                            .get(ordinal)
                            .cloned()
                            .map(|chunk| (chunk, score))
                    })
                    .collect()
            })
    }
}

fn lexical_build_error(context: &str, error: a3s_vec::Error) -> WorkspaceIndexError {
    WorkspaceIndexError::InvalidConfig(format!("A3S Vec lexical {context} failed: {error}"))
}

fn lexical_query_error(context: &str, error: a3s_vec::Error) -> WorkspaceIndexError {
    WorkspaceIndexError::InvalidQuery(format!("A3S Vec lexical {context} failed: {error}"))
}

pub(crate) fn search_catalog(
    snapshot: &ChunkCatalogSnapshot,
    request: &LexicalSearchRequest,
) -> Result<LexicalSearchResult, WorkspaceIndexError> {
    validate_request(request)?;
    let terms = query_terms(request.query.trim(), DEFAULT_QUERY_TERM_LIMIT);
    if terms.is_empty() {
        return Err(WorkspaceIndexError::InvalidQuery(
            "query must contain a letter, number, underscore, or CJK character".to_owned(),
        ));
    }
    let glob = request
        .glob
        .as_deref()
        .map(glob::Pattern::new)
        .transpose()
        .map_err(|error| WorkspaceIndexError::InvalidQuery(error.to_string()))?;

    let matching = snapshot
        .state
        .files
        .iter()
        .filter(|(path, file)| {
            path_matches(path, &request.path, glob.as_ref()) && file.lexical.has_any_term(&terms)
        })
        .collect::<Vec<_>>();
    let matching_files = matching.len();
    let candidate_truncated = matching_files > request.max_candidate_files;
    let selected = matching
        .into_iter()
        .take(request.max_candidate_files)
        .collect::<Vec<_>>();
    let selected_files = selected.len();
    let document_count = selected
        .iter()
        .map(|(_, file)| file.lexical.document_count)
        .sum::<usize>();
    if document_count == 0 {
        return Ok(LexicalSearchResult {
            catalog_revision: snapshot.revision(),
            source_revision: snapshot.source_revision(),
            query_terms: terms,
            matching_files,
            selected_files: selected.len(),
            scored_chunks: 0,
            candidate_truncated,
            hits: Vec::new(),
        });
    }
    let mut ranked = Vec::new();
    for (_, file) in selected {
        for (chunk, score) in file.lexical.search(&terms, request.limit)? {
            if score.is_finite() && score > 0.0 {
                ranked.push(LexicalSearchHit { chunk, score });
            }
        }
    }
    ranked.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.chunk.path.cmp(&right.chunk.path))
            .then_with(|| left.chunk.start_byte.cmp(&right.chunk.start_byte))
            .then_with(|| left.chunk.id.cmp(&right.chunk.id))
    });

    let mut per_file = HashMap::<Arc<str>, usize>::new();
    let hits = ranked
        .into_iter()
        .filter(|hit| {
            let count = per_file.entry(Arc::clone(&hit.chunk.path)).or_default();
            if *count >= request.max_results_per_file {
                return false;
            }
            *count += 1;
            true
        })
        .take(request.limit)
        .collect();

    Ok(LexicalSearchResult {
        catalog_revision: snapshot.revision(),
        source_revision: snapshot.source_revision(),
        query_terms: terms,
        matching_files,
        selected_files,
        scored_chunks: document_count,
        candidate_truncated,
        hits,
    })
}

fn validate_request(request: &LexicalSearchRequest) -> Result<(), WorkspaceIndexError> {
    if request.query.trim().is_empty() {
        return Err(WorkspaceIndexError::InvalidQuery(
            "query must not be empty".to_owned(),
        ));
    }
    if request.query.len() > MAX_QUERY_BYTES {
        return Err(WorkspaceIndexError::InvalidQuery(format!(
            "query exceeds the {MAX_QUERY_BYTES}-byte limit"
        )));
    }
    if request.limit == 0 || request.limit > MAX_RESULT_LIMIT {
        return Err(WorkspaceIndexError::InvalidQuery(format!(
            "limit must be from 1 to {MAX_RESULT_LIMIT}"
        )));
    }
    if request.max_candidate_files == 0 || request.max_results_per_file == 0 {
        return Err(WorkspaceIndexError::InvalidQuery(
            "candidate and per-file limits must be greater than zero".to_owned(),
        ));
    }
    Ok(())
}

fn path_matches(path: &str, base: &WorkspacePath, glob: Option<&glob::Pattern>) -> bool {
    let relative = if base.is_root() {
        path
    } else if path == base.as_str() {
        path.rsplit('/').next().unwrap_or(path)
    } else {
        let Some(relative) = path
            .strip_prefix(base.as_str())
            .and_then(|path| path.strip_prefix('/'))
        else {
            return false;
        };
        relative
    };
    glob.is_none_or(|pattern| pattern.matches(relative) || pattern.matches(path))
}

pub(crate) fn query_terms(query: &str, limit: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    tokenize(query)
        .into_iter()
        .filter(|term| seen.insert(term.clone()))
        .take(limit)
        .collect()
}

pub(crate) fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut previous_cjk = None;

    for ch in text.chars() {
        if is_cjk(ch) {
            flush_word(&mut word, &mut tokens);
            tokens.push(ch.to_string());
            if let Some(previous) = previous_cjk {
                tokens.push(format!("{previous}{ch}"));
            }
            previous_cjk = Some(ch);
        } else {
            previous_cjk = None;
            if ch.is_alphanumeric() || ch == '_' {
                word.push(ch);
            } else {
                flush_word(&mut word, &mut tokens);
            }
        }
    }
    flush_word(&mut word, &mut tokens);
    tokens
}

fn flush_word(word: &mut String, tokens: &mut Vec<String>) {
    if word.is_empty() {
        return;
    }
    if !word.chars().any(char::is_alphanumeric) {
        word.clear();
        return;
    }
    let mut variants = vec![word.to_lowercase()];
    for segment in word.split('_').filter(|segment| !segment.is_empty()) {
        variants.push(segment.to_lowercase());
        variants.extend(split_identifier(segment));
    }
    let mut seen = HashSet::new();
    tokens.extend(
        variants
            .into_iter()
            .filter(|variant| !variant.is_empty() && seen.insert(variant.clone())),
    );
    word.clear();
}

fn split_identifier(identifier: &str) -> Vec<String> {
    let chars = identifier.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return Vec::new();
    }
    let mut parts = Vec::new();
    let mut start = 0usize;
    for index in 1..chars.len() {
        let previous = chars[index - 1];
        let current = chars[index];
        let next = chars.get(index + 1).copied();
        let at_case_boundary = previous.is_lowercase() && current.is_uppercase();
        let at_acronym_boundary = previous.is_uppercase()
            && current.is_uppercase()
            && next.is_some_and(char::is_lowercase);
        let at_numeric_boundary = previous.is_numeric() != current.is_numeric()
            && previous.is_alphanumeric()
            && current.is_alphanumeric();
        if at_case_boundary || at_acronym_boundary || at_numeric_boundary {
            parts.push(
                chars[start..index]
                    .iter()
                    .collect::<String>()
                    .to_lowercase(),
            );
            start = index;
        }
    }
    parts.push(chars[start..].iter().collect::<String>().to_lowercase());
    parts
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xf900..=0xfaff
            | 0x20000..=0x2fa1f
            | 0x3040..=0x30ff
            | 0xac00..=0xd7af
    )
}
