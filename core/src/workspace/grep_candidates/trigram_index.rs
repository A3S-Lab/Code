//! In-process trigram candidate provider for workspace `grep`.
//!
//! Architecture mirrors microsoft/tgrep (trigrams as a candidate filter only):
//! exact matches still come from the workspace regex scan. Implemented in-tree
//! so `a3s-code-core` stays crates.io-publishable (`tgrep-core` is not on the
//! registry). Does not start a TCP server and does not touch durable zvec FTS.

use super::{GrepCandidateIndex, GrepCandidateSelection};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Workspace-relative directory for the durable trigram candidate cache stamp.
pub const GREP_TRIGRAM_INDEX_RELATIVE_DIR: &str = ".a3s-code/grep-trigram";

/// Skip automatic indexing above this admitted-file count so first-query
/// latency stays bounded; callers fall open to the exact regex scan.
pub const AUTO_GREP_TRIGRAM_MAX_FILES: usize = 100_000;

/// Errors while constructing a trigram candidate index.
#[derive(Debug)]
pub enum TrigramGrepCandidateIndexError {
    Io(std::io::Error),
    EmptyCorpus,
    TooManyFiles(usize),
}

impl std::fmt::Display for TrigramGrepCandidateIndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "trigram grep-candidate index I/O error: {error}"),
            Self::EmptyCorpus => {
                write!(f, "trigram grep-candidate index requires at least one file")
            }
            Self::TooManyFiles(count) => write!(
                f,
                "trigram grep-candidate auto-index skipped for {count} files (limit {AUTO_GREP_TRIGRAM_MAX_FILES})"
            ),
        }
    }
}

impl std::error::Error for TrigramGrepCandidateIndexError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::EmptyCorpus | Self::TooManyFiles(_) => None,
        }
    }
}

impl From<std::io::Error> for TrigramGrepCandidateIndexError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// In-memory trigram → file-id postings used only to prune grep paths.
pub struct TrigramGrepCandidateIndex {
    paths: Vec<String>,
    postings: HashMap<u32, Vec<u32>>,
    /// Stamp directory under the workspace (rebuild metadata / diagnostics).
    #[allow(dead_code)]
    index_dir: PathBuf,
}

impl TrigramGrepCandidateIndex {
    /// Default on-disk stamp location under a workspace root.
    pub fn default_index_dir(root: &Path) -> PathBuf {
        root.join(GREP_TRIGRAM_INDEX_RELATIVE_DIR)
    }

    /// Build an index for `files` under `root` and write a stamp under `index_dir`.
    ///
    /// `files` must be absolute paths that live under `root`. Relative paths
    /// stored in the index use `/` separators to match workspace normalization.
    pub fn build_from_files(
        root: &Path,
        index_dir: &Path,
        files: &[PathBuf],
    ) -> Result<Self, TrigramGrepCandidateIndexError> {
        if files.is_empty() {
            return Err(TrigramGrepCandidateIndexError::EmptyCorpus);
        }
        if files.len() > AUTO_GREP_TRIGRAM_MAX_FILES {
            return Err(TrigramGrepCandidateIndexError::TooManyFiles(files.len()));
        }

        let mut paths = Vec::with_capacity(files.len());
        let mut postings: HashMap<u32, Vec<u32>> = HashMap::new();

        for abs in files {
            let rel = match abs.strip_prefix(root) {
                Ok(path) => path.to_string_lossy().replace('\\', "/"),
                Err(_) => continue,
            };
            let Ok(bytes) = std::fs::read(abs) else {
                continue;
            };
            let file_id = paths.len() as u32;
            for trigram in unique_trigrams(&bytes) {
                postings.entry(trigram).or_default().push(file_id);
            }
            paths.push(rel);
        }

        if paths.is_empty() {
            return Err(TrigramGrepCandidateIndexError::EmptyCorpus);
        }

        // Sort + dedup posting lists for stable intersection.
        for list in postings.values_mut() {
            list.sort_unstable();
            list.dedup();
        }

        write_index_stamp(index_dir, paths.len())?;

        Ok(Self {
            paths,
            postings,
            index_dir: index_dir.to_path_buf(),
        })
    }

    /// Build from workspace-relative paths joined under `root`.
    pub fn build_from_relative_paths(
        root: &Path,
        index_dir: &Path,
        relative_paths: &[String],
    ) -> Result<Self, TrigramGrepCandidateIndexError> {
        let files: Vec<PathBuf> = relative_paths.iter().map(|path| root.join(path)).collect();
        Self::build_from_files(root, index_dir, &files)
    }
}

impl GrepCandidateIndex for TrigramGrepCandidateIndex {
    fn select_paths(&self, pattern: &str, case_insensitive: bool) -> GrepCandidateSelection {
        // Complex regex / short needles cannot be safely pruned from trigrams.
        let Some(needle) = literal_needle(pattern) else {
            return GrepCandidateSelection::Unconstrained;
        };
        if needle.len() < 3 {
            return GrepCandidateSelection::Unconstrained;
        }

        let needle_bytes = if case_insensitive {
            needle.to_ascii_lowercase().into_bytes()
        } else {
            needle.as_bytes().to_vec()
        };
        let required = overlapping_trigrams(&needle_bytes);
        if required.is_empty() {
            return GrepCandidateSelection::Unconstrained;
        }

        let mut candidates: Option<BTreeSet<u32>> = None;
        for trigram in required {
            let hits = if case_insensitive {
                lookup_case_insensitive(&self.postings, trigram)
            } else {
                self.postings
                    .get(&trigram)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect()
            };
            candidates = Some(match candidates {
                None => hits,
                Some(prev) => prev.intersection(&hits).copied().collect(),
            });
            if candidates.as_ref().is_some_and(|set| set.is_empty()) {
                break;
            }
        }

        let Some(file_ids) = candidates else {
            return GrepCandidateSelection::Unconstrained;
        };
        let mut paths = BTreeSet::new();
        for file_id in file_ids {
            if let Some(path) = self.paths.get(file_id as usize) {
                paths.insert(path.clone());
            }
        }
        GrepCandidateSelection::Paths(paths)
    }
}

fn write_index_stamp(
    index_dir: &Path,
    file_count: usize,
) -> Result<(), TrigramGrepCandidateIndexError> {
    let _ = std::fs::remove_dir_all(index_dir);
    std::fs::create_dir_all(index_dir)?;
    std::fs::write(
        index_dir.join("stamp.txt"),
        format!("a3s-code trigram grep candidates\nfiles={file_count}\n"),
    )?;
    Ok(())
}

fn pack_trigram(a: u8, b: u8, c: u8) -> u32 {
    (u32::from(a) << 16) | (u32::from(b) << 8) | u32::from(c)
}

fn unpack_trigram(trigram: u32) -> (u8, u8, u8) {
    (
        ((trigram >> 16) & 0xff) as u8,
        ((trigram >> 8) & 0xff) as u8,
        (trigram & 0xff) as u8,
    )
}

fn unique_trigrams(bytes: &[u8]) -> HashSet<u32> {
    overlapping_trigrams(bytes).into_iter().collect()
}

fn overlapping_trigrams(bytes: &[u8]) -> Vec<u32> {
    if bytes.len() < 3 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(bytes.len() - 2);
    for window in bytes.windows(3) {
        out.push(pack_trigram(window[0], window[1], window[2]));
    }
    out
}

/// Only pure literals are pruned; anything with regex metacharacters fails open.
fn literal_needle(pattern: &str) -> Option<&str> {
    if pattern.is_empty() {
        return None;
    }
    if pattern.chars().any(|ch| {
        matches!(
            ch,
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\' | '^' | '$'
        )
    }) {
        return None;
    }
    Some(pattern)
}

fn case_fold_byte(byte: u8) -> [u8; 2] {
    if byte.is_ascii_alphabetic() {
        [byte.to_ascii_lowercase(), byte.to_ascii_uppercase()]
    } else {
        [byte, byte]
    }
}

fn lookup_case_insensitive(postings: &HashMap<u32, Vec<u32>>, trigram: u32) -> BTreeSet<u32> {
    let (a, b, c) = unpack_trigram(trigram);
    let mut hits = BTreeSet::new();
    for aa in case_fold_byte(a) {
        for bb in case_fold_byte(b) {
            for cc in case_fold_byte(c) {
                if let Some(list) = postings.get(&pack_trigram(aa, bb, cc)) {
                    hits.extend(list.iter().copied());
                }
            }
        }
    }
    hits
}
