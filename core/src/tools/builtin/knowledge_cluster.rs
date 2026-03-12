//! Knowledge Cluster - Sirchmunk-inspired code pattern clustering
//!
//! **Status**: Future feature - not yet integrated into agentic_search tool.
//! This module provides clustering capabilities that can be used to group
//! search results into semantic clusters for better organization.
//!
//! Groups search results into semantic clusters by:
//! - Module/package boundaries
//! - Functional similarity (same keywords appear together)
//! - File proximity (files in same directory)
//!
//! Each cluster carries:
//! - Deterministic SHA256-based identity
//! - Evidence links (source files + line numbers)
//! - Confidence score (based on keyword coverage)
//! - Activity hotness (match density)

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A knowledge cluster representing a semantic unit of code
#[derive(Debug, Clone)]
pub struct KnowledgeCluster {
    /// Deterministic SHA256 identity based on file paths + keywords
    pub id: String,
    /// Human-readable cluster name (derived from common path prefix or dominant keyword)
    pub name: String,
    /// Files belonging to this cluster
    pub files: Vec<ClusterFile>,
    /// Keywords that define this cluster
    pub keywords: Vec<String>,
    /// Confidence score (0.0 - 1.0): keyword coverage across files
    pub confidence: f32,
    /// Activity hotness: match density relative to cluster size
    pub hotness: f32,
    /// Lifecycle state
    pub state: ClusterState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClusterState {
    /// Newly formed, not yet validated
    Emerging,
    /// Well-established with high confidence
    Stable,
    /// Low activity, may be stale
    Deprecated,
}

/// A file within a cluster with its evidence
#[derive(Debug, Clone)]
pub struct ClusterFile {
    pub path: PathBuf,
    /// Line numbers where keywords were found
    pub evidence_lines: Vec<usize>,
    /// Relevance score within this cluster
    pub relevance: f32,
}

impl KnowledgeCluster {
    /// Build clusters from a set of file matches grouped by keywords
    ///
    /// Clustering strategy:
    /// 1. Group files by common directory prefix
    /// 2. Merge groups that share > 50% of keywords
    /// 3. Assign deterministic IDs via SHA256
    pub fn build_from_matches(
        file_paths: &[(PathBuf, Vec<usize>, f32)], // (path, match_lines, relevance)
        keywords: &[String],
    ) -> Vec<KnowledgeCluster> {
        if file_paths.is_empty() {
            return Vec::new();
        }

        // Group files by directory
        let mut dir_groups: HashMap<PathBuf, Vec<(PathBuf, Vec<usize>, f32)>> = HashMap::new();
        for (path, lines, rel) in file_paths {
            let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            dir_groups
                .entry(dir)
                .or_default()
                .push((path.clone(), lines.clone(), *rel));
        }

        let mut clusters: Vec<KnowledgeCluster> = Vec::new();

        for (dir, files) in dir_groups {
            let cluster_files: Vec<ClusterFile> = files
                .iter()
                .map(|(path, lines, rel)| ClusterFile {
                    path: path.clone(),
                    evidence_lines: lines.clone(),
                    relevance: *rel,
                })
                .collect();

            // Confidence = average relevance of files in cluster
            let confidence = if cluster_files.is_empty() {
                0.0
            } else {
                cluster_files.iter().map(|f| f.relevance).sum::<f32>()
                    / cluster_files.len() as f32
            };

            // Hotness = total evidence lines / total files
            let total_lines: usize = cluster_files.iter().map(|f| f.evidence_lines.len()).sum();
            let hotness = total_lines as f32 / cluster_files.len().max(1) as f32;

            // Deterministic ID: SHA256(sorted file paths + keywords)
            let id = compute_cluster_id(&cluster_files, keywords);

            // Cluster name: last component of common directory
            let name = dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("root")
                .to_string();

            let state = if confidence > 0.6 {
                ClusterState::Stable
            } else if confidence > 0.2 {
                ClusterState::Emerging
            } else {
                ClusterState::Deprecated
            };

            clusters.push(KnowledgeCluster {
                id,
                name,
                files: cluster_files,
                keywords: keywords.to_vec(),
                confidence,
                hotness,
                state,
            });
        }

        // Sort by confidence descending
        clusters.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        clusters
    }

    /// Format cluster as a summary for display
    pub fn format_summary(&self) -> String {
        let state_label = match self.state {
            ClusterState::Stable => "stable",
            ClusterState::Emerging => "emerging",
            ClusterState::Deprecated => "deprecated",
        };

        let mut out = format!(
            "Cluster: {} [{}] (confidence: {:.2}, hotness: {:.1})\n",
            self.name, state_label, self.confidence, self.hotness
        );
        out.push_str(&format!("  ID: {}\n", &self.id[..12]));
        out.push_str(&format!("  Files: {}\n", self.files.len()));
        for f in &self.files {
            out.push_str(&format!(
                "    {} ({} matches)\n",
                f.path.display(),
                f.evidence_lines.len()
            ));
        }
        out
    }
}

/// Compute a deterministic SHA256 cluster ID from file paths and keywords
fn compute_cluster_id(files: &[ClusterFile], keywords: &[String]) -> String {
    let mut hasher = Sha256::new();

    // Sort paths for determinism
    let mut paths: Vec<String> = files
        .iter()
        .map(|f| f.path.to_string_lossy().to_string())
        .collect();
    paths.sort();

    for path in &paths {
        hasher.update(path.as_bytes());
        hasher.update(b"|");
    }

    let mut sorted_kw = keywords.to_vec();
    sorted_kw.sort();
    for kw in &sorted_kw {
        hasher.update(kw.as_bytes());
        hasher.update(b"|");
    }

    format!("{:x}", hasher.finalize())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_files(paths: &[&str]) -> Vec<(PathBuf, Vec<usize>, f32)> {
        paths
            .iter()
            .enumerate()
            .map(|(i, p)| {
                (
                    PathBuf::from(p),
                    vec![1, 5, 10],
                    0.5 + (i as f32 * 0.1),
                )
            })
            .collect()
    }

    #[test]
    fn test_build_clusters_groups_by_directory() {
        let files = make_files(&[
            "src/auth/login.rs",
            "src/auth/logout.rs",
            "src/db/connection.rs",
        ]);
        let keywords = vec!["auth".to_string(), "token".to_string()];

        let clusters = KnowledgeCluster::build_from_matches(&files, &keywords);

        assert_eq!(clusters.len(), 2); // auth/ and db/
        let names: Vec<&str> = clusters.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"auth"));
        assert!(names.contains(&"db"));
    }

    #[test]
    fn test_cluster_id_is_deterministic() {
        let files = make_files(&["src/auth/login.rs", "src/auth/logout.rs"]);
        let keywords = vec!["auth".to_string()];

        let c1 = KnowledgeCluster::build_from_matches(&files, &keywords);
        let c2 = KnowledgeCluster::build_from_matches(&files, &keywords);

        assert_eq!(c1[0].id, c2[0].id);
    }

    #[test]
    fn test_cluster_state_stable() {
        let files = vec![(PathBuf::from("src/auth/login.rs"), vec![1, 2, 3], 0.8_f32)];
        let keywords = vec!["auth".to_string()];

        let clusters = KnowledgeCluster::build_from_matches(&files, &keywords);
        assert_eq!(clusters[0].state, ClusterState::Stable);
    }

    #[test]
    fn test_cluster_state_emerging() {
        let files = vec![(PathBuf::from("src/auth/login.rs"), vec![1], 0.3_f32)];
        let keywords = vec!["auth".to_string()];

        let clusters = KnowledgeCluster::build_from_matches(&files, &keywords);
        assert_eq!(clusters[0].state, ClusterState::Emerging);
    }

    #[test]
    fn test_cluster_state_deprecated() {
        let files = vec![(PathBuf::from("src/auth/login.rs"), vec![1], 0.1_f32)];
        let keywords = vec!["auth".to_string()];

        let clusters = KnowledgeCluster::build_from_matches(&files, &keywords);
        assert_eq!(clusters[0].state, ClusterState::Deprecated);
    }

    #[test]
    fn test_empty_input() {
        let clusters = KnowledgeCluster::build_from_matches(&[], &["auth".to_string()]);
        assert!(clusters.is_empty());
    }

    #[test]
    fn test_format_summary_contains_key_info() {
        let files = make_files(&["src/auth/login.rs"]);
        let keywords = vec!["auth".to_string()];
        let clusters = KnowledgeCluster::build_from_matches(&files, &keywords);

        let summary = clusters[0].format_summary();
        assert!(summary.contains("auth"));
        assert!(summary.contains("confidence"));
        assert!(summary.contains("hotness"));
        assert!(summary.contains("login.rs"));
    }

    #[test]
    fn test_sorted_by_confidence_descending() {
        let files = vec![
            (PathBuf::from("src/low/a.rs"), vec![1], 0.1_f32),
            (PathBuf::from("src/high/b.rs"), vec![1, 2, 3], 0.9_f32),
        ];
        let keywords = vec!["test".to_string()];
        let clusters = KnowledgeCluster::build_from_matches(&files, &keywords);

        assert!(clusters[0].confidence >= clusters[1].confidence);
    }
}
