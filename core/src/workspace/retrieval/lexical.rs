use super::catalog::ChunkCatalogSnapshot;
use super::types::{WorkspaceChunk, WorkspaceIndexError};
use crate::workspace::WorkspacePath;
use std::collections::{HashMap, HashSet};
use std::mem::size_of;
use std::sync::Arc;

pub(crate) const K1: f64 = 1.2;
pub(crate) const B: f64 = 0.75;
const DEFAULT_QUERY_TERM_LIMIT: usize = 32;
const DEFAULT_CANDIDATE_FILE_LIMIT: usize = 256;
const DEFAULT_RESULT_LIMIT: usize = 10;
const MAX_RESULT_LIMIT: usize = 25;
const MAX_QUERY_BYTES: usize = 2_048;
const DEFAULT_RESULTS_PER_FILE: usize = 2;

#[derive(Debug, Clone)]
pub(crate) struct Bm25Document {
    pub(crate) term_frequencies: HashMap<String, u32>,
    pub(crate) length: usize,
}

impl Bm25Document {
    pub(crate) fn from_text(text: &str) -> Self {
        let tokens = tokenize(text);
        let mut term_frequencies = HashMap::new();
        for token in &tokens {
            *term_frequencies.entry(token.clone()).or_insert(0) += 1;
        }
        Self {
            term_frequencies,
            length: tokens.len(),
        }
    }
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
    pub query_terms: Vec<String>,
    pub matching_files: usize,
    pub selected_files: usize,
    pub scored_chunks: usize,
    pub candidate_truncated: bool,
    pub hits: Vec<LexicalSearchHit>,
}

#[derive(Clone, Debug)]
pub(crate) struct Posting {
    document: usize,
    term_frequency: u32,
}

pub(crate) struct LexicalPartition {
    chunks: Arc<[Arc<WorkspaceChunk>]>,
    documents: Arc<[Bm25Document]>,
    postings: HashMap<String, Arc<[Posting]>>,
    pub(crate) document_count: usize,
    pub(crate) total_document_terms: usize,
}

impl LexicalPartition {
    pub(crate) fn build(chunks: Arc<[Arc<WorkspaceChunk>]>) -> Self {
        let indexed = chunks
            .iter()
            .filter_map(|chunk| {
                let document = Bm25Document::from_text(&chunk.text);
                (document.length > 0).then(|| (Arc::clone(chunk), document))
            })
            .collect::<Vec<_>>();
        let (chunks, documents): (Vec<_>, Vec<_>) = indexed.into_iter().unzip();
        let mut postings = HashMap::<String, Vec<Posting>>::new();
        for (document, stats) in documents.iter().enumerate() {
            for (term, term_frequency) in &stats.term_frequencies {
                postings.entry(term.clone()).or_default().push(Posting {
                    document,
                    term_frequency: *term_frequency,
                });
            }
        }
        let total_document_terms = documents.iter().map(|document| document.length).sum();
        Self {
            chunks: Arc::from(chunks),
            document_count: documents.len(),
            total_document_terms,
            documents: Arc::from(documents),
            postings: postings
                .into_iter()
                .map(|(term, postings)| (term, Arc::from(postings)))
                .collect(),
        }
    }

    fn has_any_term(&self, terms: &[String]) -> bool {
        terms.iter().any(|term| self.postings.contains_key(term))
    }

    pub(crate) fn estimated_bytes(&self) -> usize {
        let document_bytes = self
            .documents
            .len()
            .saturating_mul(size_of::<Bm25Document>());
        let frequency_bytes = self.documents.iter().fold(0usize, |total, document| {
            let entries = document
                .term_frequencies
                .capacity()
                .saturating_mul(size_of::<(String, u32)>() + 1);
            let strings = document
                .term_frequencies
                .keys()
                .fold(0usize, |bytes, term| bytes.saturating_add(term.capacity()));
            total.saturating_add(entries).saturating_add(strings)
        });
        let posting_map_bytes = self
            .postings
            .capacity()
            .saturating_mul(size_of::<(String, Arc<[Posting]>)>() + 1);
        let postings_bytes = self
            .postings
            .iter()
            .fold(0usize, |total, (term, postings)| {
                total
                    .saturating_add(term.capacity())
                    .saturating_add(postings.len().saturating_mul(size_of::<Posting>()))
            });
        let chunk_refs = self
            .chunks
            .len()
            .saturating_mul(size_of::<Arc<WorkspaceChunk>>());
        size_of::<Self>()
            .saturating_add(document_bytes)
            .saturating_add(frequency_bytes)
            .saturating_add(posting_map_bytes)
            .saturating_add(postings_bytes)
            .saturating_add(chunk_refs)
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
    let total_terms = selected
        .iter()
        .map(|(_, file)| file.lexical.total_document_terms)
        .sum::<usize>();
    let average_document_length = (total_terms as f64 / document_count as f64).max(1.0);
    let mut scores = selected
        .iter()
        .map(|(_, file)| vec![0.0f64; file.lexical.document_count])
        .collect::<Vec<_>>();

    for term in &terms {
        let document_frequency = selected
            .iter()
            .map(|(_, file)| {
                file.lexical
                    .postings
                    .get(term)
                    .map_or(0, |postings| postings.len())
            })
            .sum::<usize>() as f64;
        if document_frequency == 0.0 {
            continue;
        }
        let corpus_size = document_count as f64;
        let inverse_document_frequency =
            (1.0 + (corpus_size - document_frequency + 0.5) / (document_frequency + 0.5)).ln();
        for ((_, file), file_scores) in selected.iter().zip(&mut scores) {
            let Some(postings) = file.lexical.postings.get(term) else {
                continue;
            };
            for posting in postings.iter() {
                let document = &file.lexical.documents[posting.document];
                let term_frequency = posting.term_frequency as f64;
                let length_ratio = document.length as f64 / average_document_length;
                let denominator = term_frequency + K1 * (1.0 - B + B * length_ratio);
                file_scores[posting.document] += inverse_document_frequency
                    * (term_frequency * (K1 + 1.0) / denominator.max(f64::EPSILON));
            }
        }
    }

    let mut ranked = selected
        .iter()
        .zip(scores)
        .flat_map(|((_, file), file_scores)| {
            file_scores
                .into_iter()
                .enumerate()
                .filter(|(_, score)| score.is_finite() && *score > 0.0)
                .map(|(document, score)| LexicalSearchHit {
                    chunk: Arc::clone(&file.lexical.chunks[document]),
                    score,
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
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
        selected_files: selected.len(),
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

pub(crate) fn score_documents(query_terms: &[String], documents: &[Bm25Document]) -> Vec<f64> {
    let mut scores = vec![0.0; documents.len()];
    if query_terms.is_empty() || documents.is_empty() {
        return scores;
    }

    let document_count = documents.len() as f64;
    let average_document_length = documents
        .iter()
        .map(|document| document.length)
        .sum::<usize>() as f64
        / document_count;
    let average_document_length = average_document_length.max(1.0);
    let mut seen = HashSet::new();

    for term in query_terms {
        if !seen.insert(term.as_str()) {
            continue;
        }
        let document_frequency = documents
            .iter()
            .filter(|document| document.term_frequencies.contains_key(term))
            .count() as f64;
        if document_frequency == 0.0 {
            continue;
        }
        let inverse_document_frequency =
            (1.0 + (document_count - document_frequency + 0.5) / (document_frequency + 0.5)).ln();

        for (document, score) in documents.iter().zip(&mut scores) {
            let term_frequency = document
                .term_frequencies
                .get(term)
                .copied()
                .unwrap_or_default() as f64;
            if term_frequency == 0.0 {
                continue;
            }
            let length_ratio = document.length as f64 / average_document_length;
            let denominator = term_frequency + K1 * (1.0 - B + B * length_ratio);
            *score += inverse_document_frequency
                * (term_frequency * (K1 + 1.0) / denominator.max(f64::EPSILON));
        }
    }
    scores
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
