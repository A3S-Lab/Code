use super::catalog::ChunkCatalogSnapshot;
use super::types::{
    WorkspaceChunk, WorkspaceIndexError, WorkspaceIndexResult, WorkspaceLexicalEngine,
};
use crate::workspace::WorkspacePath;
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::sync::Arc;

const DEFAULT_QUERY_TERM_LIMIT: usize = 32;
const DEFAULT_CANDIDATE_FILE_LIMIT: usize = 256;
const DEFAULT_RESULT_LIMIT: usize = 10;
const MAX_RESULT_LIMIT: usize = 25;
const MAX_QUERY_BYTES: usize = 2_048;
const DEFAULT_RESULTS_PER_FILE: usize = 2;
const PARALLEL_BUILD_MIN_DOCUMENTS: usize = 128;
const PARALLEL_BUILD_MIN_BYTES: usize = 64 * 1024;

pub(crate) fn should_parallelize_build(document_count: usize, text_bytes: usize) -> bool {
    document_count >= PARALLEL_BUILD_MIN_DOCUMENTS
        && text_bytes >= PARALLEL_BUILD_MIN_BYTES
        && std::thread::available_parallelism()
            .map(|workers| workers.get() > 1)
            .unwrap_or(true)
}

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
    pub lexical_engine: WorkspaceLexicalEngine,
    pub query_terms: Vec<String>,
    pub matching_files: usize,
    pub selected_files: usize,
    pub scored_chunks: usize,
    pub candidate_truncated: bool,
    pub hits: Vec<LexicalSearchHit>,
}

/// One dependency-free lexical document used by minimal builds.
#[derive(Debug, Clone)]
struct PortableDocument {
    term_frequencies: HashMap<String, u32>,
    length: usize,
}

impl PortableDocument {
    fn from_tokens(tokens: &[String]) -> Self {
        let mut term_frequencies = HashMap::new();
        for token in tokens {
            *term_frequencies.entry(token.clone()).or_insert(0) += 1;
        }
        Self {
            term_frequencies,
            length: tokens.len(),
        }
    }
}

#[derive(Debug, Clone)]
struct PortablePosting {
    document: usize,
    term_frequency: u32,
}

/// Small, deterministic scorer used by minimal builds that intentionally omit
/// the native zvec artifact. It shares the exact tokenizer and result contract
/// with the zvec path, so disabling native artifacts never removes BM25.
pub(crate) struct PortableLexicalIndex {
    documents: Vec<PortableDocument>,
    postings: HashMap<String, Vec<PortablePosting>>,
    terms: HashSet<String>,
    estimated_bytes: usize,
}

impl PortableLexicalIndex {
    fn build(documents: &[(String, String)]) -> WorkspaceIndexResult<Self> {
        let mut seen_keys = HashSet::new();
        for (key, _) in documents {
            if key.is_empty() || key.contains('\0') {
                return Err(WorkspaceIndexError::InvalidConfig(
                    "lexical document key must be non-empty and contain no NUL byte".to_owned(),
                ));
            }
            if !seen_keys.insert(key) {
                return Err(WorkspaceIndexError::InvalidConfig(
                    "lexical document keys must be unique".to_owned(),
                ));
            }
        }
        // Tokenization and per-document term-frequency construction are pure
        // CPU work. Rayon keeps the resulting vector in input order, which is
        // required for deterministic BM25 tie-breaking and chunk ordinals.
        // Tiny partitions stay serial because scheduling overhead would cost
        // more than the work; large rebuilds use every available worker.
        let parallel = should_parallelize_build(
            documents.len(),
            documents
                .iter()
                .fold(0usize, |total, (_, text)| total.saturating_add(text.len())),
        );
        let tokenized = if parallel {
            documents
                .par_iter()
                .filter_map(|(_, text)| {
                    let tokens = tokenize(text);
                    (!tokens.is_empty()).then_some(tokens)
                })
                .collect::<Vec<_>>()
        } else {
            documents
                .iter()
                .filter_map(|(_, text)| {
                    let tokens = tokenize(text);
                    (!tokens.is_empty()).then_some(tokens)
                })
                .collect::<Vec<_>>()
        };
        let indexed = if parallel {
            tokenized
                .par_iter()
                .map(|tokens| PortableDocument::from_tokens(tokens))
                .collect::<Vec<_>>()
        } else {
            tokenized
                .iter()
                .map(|tokens| PortableDocument::from_tokens(tokens))
                .collect::<Vec<_>>()
        };
        let terms = if parallel {
            tokenized
                .par_iter()
                .flat_map_iter(|tokens| tokens.iter().cloned())
                .collect::<HashSet<_>>()
        } else {
            tokenized
                .iter()
                .flat_map(|tokens| tokens.iter().cloned())
                .collect::<HashSet<_>>()
        };
        let mut postings = if parallel {
            indexed
                .par_iter()
                .enumerate()
                .fold(
                    HashMap::<String, Vec<PortablePosting>>::new,
                    |mut postings, (document, stats)| {
                        for (term, frequency) in &stats.term_frequencies {
                            postings
                                .entry(term.clone())
                                .or_default()
                                .push(PortablePosting {
                                    document,
                                    term_frequency: *frequency,
                                });
                        }
                        postings
                    },
                )
                .reduce(
                    HashMap::<String, Vec<PortablePosting>>::new,
                    |mut left, mut right| {
                        for (term, mut values) in right.drain() {
                            left.entry(term).or_default().append(&mut values);
                        }
                        left
                    },
                )
        } else {
            let mut postings = HashMap::<String, Vec<PortablePosting>>::new();
            for (document, stats) in indexed.iter().enumerate() {
                for (term, frequency) in &stats.term_frequencies {
                    postings
                        .entry(term.clone())
                        .or_default()
                        .push(PortablePosting {
                            document,
                            term_frequency: *frequency,
                        });
                }
            }
            postings
        };
        if parallel {
            // Rayon reduction order is intentionally unspecified. Restore
            // document order in each posting list before scoring so floating
            // point accumulation and deterministic ties remain stable.
            for values in postings.values_mut() {
                values.sort_unstable_by_key(|posting| posting.document);
            }
        }
        let estimated_bytes = size_of::<Self>()
            .saturating_add(indexed.len().saturating_mul(size_of::<PortableDocument>()))
            .saturating_add(
                indexed
                    .iter()
                    .flat_map(|document| document.term_frequencies.keys())
                    .map(|term| term.capacity())
                    .sum::<usize>(),
            )
            .saturating_add(
                postings
                    .iter()
                    .map(|(term, values)| {
                        term.capacity().saturating_add(
                            values.len().saturating_mul(size_of::<PortablePosting>()),
                        )
                    })
                    .sum::<usize>(),
            );
        Ok(Self {
            documents: indexed,
            postings,
            terms,
            estimated_bytes,
        })
    }

    fn document_count(&self) -> usize {
        self.documents.len()
    }

    fn estimated_bytes(&self) -> usize {
        self.estimated_bytes
    }

    fn has_any_term(&self, terms: &[String]) -> bool {
        terms.iter().any(|term| self.terms.contains(term))
    }

    fn search(&self, terms: &[String], limit: usize) -> WorkspaceIndexResult<Vec<(usize, f64)>> {
        if terms.is_empty() || limit == 0 || self.documents.is_empty() {
            return Ok(Vec::new());
        }
        const K1: f64 = 1.2;
        const B: f64 = 0.75;
        let document_count = self.documents.len() as f64;
        let average_length = (self
            .documents
            .iter()
            .map(|document| document.length)
            .sum::<usize>() as f64
            / document_count)
            .max(1.0);
        let mut scores = vec![0.0; self.documents.len()];
        let mut seen = HashSet::new();
        for term in terms {
            if !seen.insert(term.as_str()) {
                continue;
            }
            let Some(postings) = self.postings.get(term) else {
                continue;
            };
            let document_frequency = postings.len() as f64;
            let idf = (1.0
                + (document_count - document_frequency + 0.5) / (document_frequency + 0.5))
                .ln();
            for posting in postings {
                let length_ratio = self.documents[posting.document].length as f64 / average_length;
                let frequency = posting.term_frequency as f64;
                let denominator = frequency + K1 * (1.0 - B + B * length_ratio);
                scores[posting.document] +=
                    idf * (frequency * (K1 + 1.0) / denominator.max(f64::EPSILON));
            }
        }
        let mut hits = scores
            .into_iter()
            .enumerate()
            .filter(|(_, score)| score.is_finite() && *score > 0.0)
            .collect::<Vec<_>>();
        hits.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        hits.truncate(limit);
        Ok(hits)
    }
}

/// Backend-neutral lexical index handle. The native zvec binding is the
/// product default; the portable variant is available only for minimal builds.
pub(crate) enum LexicalIndex {
    Portable(PortableLexicalIndex),
    #[cfg(feature = "zvec-rust-fts")]
    ZvecRust(super::zvec_rust::ZvecRustLexicalIndex),
}

impl LexicalIndex {
    fn build(
        documents: &[(String, String)],
        engine: WorkspaceLexicalEngine,
    ) -> WorkspaceIndexResult<Self> {
        match engine {
            WorkspaceLexicalEngine::Portable => {
                PortableLexicalIndex::build(documents).map(Self::Portable)
            }
            WorkspaceLexicalEngine::ZvecRust => {
                #[cfg(feature = "zvec-rust-fts")]
                {
                    super::zvec_rust::ZvecRustLexicalIndex::build(
                        documents
                            .iter()
                            .map(|(key, text)| (key.as_str(), text.as_str())),
                    )
                    .map(Self::ZvecRust)
                    .map_err(|error| {
                        WorkspaceIndexError::InvalidConfig(format!(
                            "zvec-rust lexical index failed: {error}"
                        ))
                    })
                }
                #[cfg(not(feature = "zvec-rust-fts"))]
                {
                    Err(WorkspaceIndexError::InvalidConfig(
                        "WorkspaceLexicalEngine::ZvecRust requires the zvec-rust-fts feature"
                            .to_owned(),
                    ))
                }
            }
        }
    }

    pub(crate) fn document_count(&self) -> usize {
        match self {
            Self::Portable(index) => index.document_count(),
            #[cfg(feature = "zvec-rust-fts")]
            Self::ZvecRust(index) => index.document_count(),
        }
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        match self {
            Self::Portable(index) => index.estimated_bytes(),
            #[cfg(feature = "zvec-rust-fts")]
            Self::ZvecRust(index) => index.estimated_bytes(),
        }
    }

    pub(crate) fn has_any_term(&self, terms: &[String]) -> bool {
        match self {
            Self::Portable(index) => index.has_any_term(terms),
            #[cfg(feature = "zvec-rust-fts")]
            Self::ZvecRust(index) => index.has_any_term(terms),
        }
    }

    pub(crate) fn search(
        &self,
        terms: &[String],
        limit: usize,
    ) -> WorkspaceIndexResult<Vec<(usize, f64)>> {
        match self {
            Self::Portable(index) => index.search(terms, limit),
            #[cfg(feature = "zvec-rust-fts")]
            Self::ZvecRust(index) => index.search(terms, limit).map_err(|error| {
                WorkspaceIndexError::InvalidQuery(format!("zvec-rust FTS search failed: {error}"))
            }),
        }
    }
}

/// Build one bounded lexical index through the selected backend.
///
/// The query-time path and incremental workspace catalog intentionally call the
/// same function, keeping backend selection in one place.
pub(crate) fn build_lexical_index<I, K, T>(
    documents: I,
    engine: WorkspaceLexicalEngine,
) -> WorkspaceIndexResult<LexicalIndex>
where
    I: IntoIterator<Item = (K, T)>,
    K: AsRef<str>,
    T: AsRef<str>,
{
    let documents = documents
        .into_iter()
        .map(|(key, text)| (key.as_ref().to_owned(), text.as_ref().to_owned()))
        .collect::<Vec<_>>();
    LexicalIndex::build(&documents, engine)
}

pub(crate) struct LexicalPartition {
    index: LexicalIndex,
    chunks: Arc<[Arc<WorkspaceChunk>]>,
    pub(crate) document_count: usize,
}

impl LexicalPartition {
    /// Build the catalog's lexical partition through the selected FTS API.
    ///
    /// Code still owns chunk admission and path policy, while tokenization,
    /// postings, BM25 statistics, and deterministic score ordering are owned
    /// by the selected engine. Workspace source remains authoritative and no
    /// durable cross-session index is introduced by lexical search.
    pub(crate) fn build(
        chunks: Arc<[Arc<WorkspaceChunk>]>,
        engine: WorkspaceLexicalEngine,
    ) -> WorkspaceIndexResult<Self> {
        let indexed_chunks: Arc<[Arc<WorkspaceChunk>]> = Arc::from(
            chunks
                .iter()
                .filter(|chunk| !tokenize(chunk.text.as_ref()).is_empty())
                .cloned()
                .collect::<Vec<_>>(),
        );
        let index = build_lexical_index(
            indexed_chunks
                .iter()
                .map(|chunk| (chunk.id.as_str(), chunk.text.as_ref())),
            engine,
        )?;
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
        self.index.search(terms, limit).map(|hits| {
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
            lexical_engine: snapshot.lexical_engine(),
            query_terms: terms,
            matching_files,
            selected_files: selected.len(),
            scored_chunks: 0,
            candidate_truncated,
            hits: Vec::new(),
        });
    }
    let mut ranked = Vec::new();
    // The final contract admits at most `max_results_per_file` hits from any
    // one file. Asking each backend for more candidates only adds native FTS
    // work (and result materialization) without changing the observable
    // ordering.
    let per_file_limit = request.limit.min(request.max_results_per_file);
    for (_, file) in selected {
        for (chunk, score) in file.lexical.search(&terms, per_file_limit)? {
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
        lexical_engine: snapshot.lexical_engine(),
        query_terms: terms,
        matching_files,
        selected_files,
        scored_chunks: document_count,
        candidate_truncated,
        hits,
    })
}

pub(crate) fn validate_request(request: &LexicalSearchRequest) -> Result<(), WorkspaceIndexError> {
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

pub(crate) fn path_matches(path: &str, base: &WorkspacePath, glob: Option<&glob::Pattern>) -> bool {
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

#[cfg(test)]
mod tests {
    use super::PortableLexicalIndex;

    #[test]
    fn large_portable_build_keeps_dense_ordinals_after_empty_documents() {
        let documents = (0..256)
            .map(|index| {
                let text = if index % 13 == 0 {
                    " \n\t".to_owned()
                } else {
                    format!(
                        "{} {}",
                        if index == 129 {
                            "parallelneedle"
                        } else {
                            "common"
                        },
                        "workspace payload ".repeat(64)
                    )
                };
                (format!("doc-{index:03}"), text)
            })
            .collect::<Vec<_>>();
        let index = PortableLexicalIndex::build(&documents).expect("portable index");
        let hits = index
            .search(&["parallelneedle".to_owned()], 1)
            .expect("portable query");
        let expected_ordinal = (0..129).filter(|index| index % 13 != 0).count();
        assert_eq!(hits.first().map(|hit| hit.0), Some(expected_ordinal));
        assert_eq!(index.document_count(), 236);
    }
}
