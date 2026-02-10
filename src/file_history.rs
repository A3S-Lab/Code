//! File version history tracking
//!
//! Automatically captures file snapshots before modifications (write, edit, patch).
//! Provides version listing, diff generation, and restore capabilities.
//!
//! ## Design
//!
//! - Per-session file history stored in memory
//! - Snapshots captured before each file-modifying tool execution
//! - Unified diff generation between any two versions
//! - Restore to any previous version
//!
//! ## Usage
//!
//! ```rust,ignore
//! use a3s_code::file_history::FileHistory;
//!
//! let history = FileHistory::new(100); // max 100 snapshots
//! history.save_snapshot("/path/to/file.rs", "original content");
//! history.save_snapshot("/path/to/file.rs", "modified content");
//!
//! let versions = history.list_versions("/path/to/file.rs");
//! let diff = history.diff("/path/to/file.rs", 0, 1);
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use std::collections::HashMap;
use std::sync::RwLock;

/// A single file version snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshot {
    /// Version number (0-indexed, monotonically increasing per file)
    pub version: usize,
    /// File path (absolute or workspace-relative)
    pub path: String,
    /// File content at this version
    pub content: String,
    /// Timestamp when the snapshot was taken
    pub timestamp: DateTime<Utc>,
    /// Tool that triggered the snapshot (e.g., "write", "edit", "patch")
    pub tool_name: String,
}

/// Summary of a file version (without content, for listing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionSummary {
    /// Version number
    pub version: usize,
    /// File path
    pub path: String,
    /// Timestamp
    pub timestamp: DateTime<Utc>,
    /// Tool that triggered the snapshot
    pub tool_name: String,
    /// Content size in bytes
    pub size: usize,
}

impl From<&FileSnapshot> for VersionSummary {
    fn from(snapshot: &FileSnapshot) -> Self {
        Self {
            version: snapshot.version,
            path: snapshot.path.clone(),
            timestamp: snapshot.timestamp,
            tool_name: snapshot.tool_name.clone(),
            size: snapshot.content.len(),
        }
    }
}

/// File version history tracker
///
/// Thread-safe storage for file snapshots. Each file maintains an ordered
/// list of versions. Old versions are evicted when `max_snapshots` is reached.
pub struct FileHistory {
    /// Map from file path to ordered list of snapshots
    snapshots: RwLock<HashMap<String, Vec<FileSnapshot>>>,
    /// Maximum total snapshots across all files
    max_snapshots: usize,
}

impl FileHistory {
    /// Create a new file history tracker
    ///
    /// `max_snapshots` limits the total number of snapshots stored.
    /// When exceeded, the oldest snapshots (across all files) are evicted.
    pub fn new(max_snapshots: usize) -> Self {
        Self {
            snapshots: RwLock::new(HashMap::new()),
            max_snapshots,
        }
    }

    /// Save a snapshot of a file's content before modification
    ///
    /// Returns the version number assigned to this snapshot.
    pub fn save_snapshot(&self, path: &str, content: &str, tool_name: &str) -> usize {
        let mut snapshots = self.snapshots.write().unwrap();

        let file_versions = snapshots.entry(path.to_string()).or_default();
        let version = file_versions.len();

        file_versions.push(FileSnapshot {
            version,
            path: path.to_string(),
            content: content.to_string(),
            timestamp: Utc::now(),
            tool_name: tool_name.to_string(),
        });

        // Evict oldest snapshots if over limit
        self.evict_if_needed(&mut snapshots);

        version
    }

    /// List all versions of a specific file
    pub fn list_versions(&self, path: &str) -> Vec<VersionSummary> {
        let snapshots = self.snapshots.read().unwrap();
        snapshots
            .get(path)
            .map(|versions| versions.iter().map(VersionSummary::from).collect())
            .unwrap_or_default()
    }

    /// List all tracked files with their version counts
    pub fn list_files(&self) -> Vec<(String, usize)> {
        let snapshots = self.snapshots.read().unwrap();
        snapshots
            .iter()
            .map(|(path, versions)| (path.clone(), versions.len()))
            .collect()
    }

    /// Get a specific version's content
    pub fn get_version(&self, path: &str, version: usize) -> Option<FileSnapshot> {
        let snapshots = self.snapshots.read().unwrap();
        snapshots
            .get(path)
            .and_then(|versions| versions.get(version).cloned())
    }

    /// Get the latest version of a file
    pub fn get_latest(&self, path: &str) -> Option<FileSnapshot> {
        let snapshots = self.snapshots.read().unwrap();
        snapshots
            .get(path)
            .and_then(|versions| versions.last().cloned())
    }

    /// Generate a unified diff between two versions of a file
    ///
    /// Returns `None` if either version doesn't exist.
    pub fn diff(&self, path: &str, from_version: usize, to_version: usize) -> Option<String> {
        let snapshots = self.snapshots.read().unwrap();
        let versions = snapshots.get(path)?;

        let from = versions.get(from_version)?;
        let to = versions.get(to_version)?;

        Some(generate_unified_diff(
            &from.content,
            &to.content,
            path,
            from_version,
            to_version,
        ))
    }

    /// Generate a diff between a version and the current file content
    pub fn diff_with_current(
        &self,
        path: &str,
        version: usize,
        current_content: &str,
    ) -> Option<String> {
        let snapshots = self.snapshots.read().unwrap();
        let versions = snapshots.get(path)?;
        let from = versions.get(version)?;

        Some(generate_unified_diff(
            &from.content,
            current_content,
            path,
            version,
            versions.len(), // "current" as pseudo-version
        ))
    }

    /// Get the total number of snapshots across all files
    pub fn total_snapshots(&self) -> usize {
        let snapshots = self.snapshots.read().unwrap();
        snapshots.values().map(|v| v.len()).sum()
    }

    /// Clear all history for a specific file
    pub fn clear_file(&self, path: &str) {
        let mut snapshots = self.snapshots.write().unwrap();
        snapshots.remove(path);
    }

    /// Clear all history
    pub fn clear_all(&self) {
        let mut snapshots = self.snapshots.write().unwrap();
        snapshots.clear();
    }

    /// Evict oldest snapshots when over the limit
    fn evict_if_needed(&self, snapshots: &mut HashMap<String, Vec<FileSnapshot>>) {
        let total: usize = snapshots.values().map(|v| v.len()).sum();
        if total <= self.max_snapshots {
            return;
        }

        let to_remove = total - self.max_snapshots;

        // Collect all snapshots with their file path, sorted by timestamp
        let mut all_entries: Vec<(String, usize, DateTime<Utc>)> = Vec::new();
        for (path, versions) in snapshots.iter() {
            for snapshot in versions {
                all_entries.push((path.clone(), snapshot.version, snapshot.timestamp));
            }
        }
        all_entries.sort_by_key(|e| e.2);

        // Remove the oldest entries
        for (path, version, _) in all_entries.into_iter().take(to_remove) {
            if let Some(versions) = snapshots.get_mut(&path) {
                versions.retain(|s| s.version != version);
                if versions.is_empty() {
                    snapshots.remove(&path);
                }
            }
        }
    }
}

/// Generate a unified diff between two strings
fn generate_unified_diff(
    old: &str,
    new: &str,
    path: &str,
    from_version: usize,
    to_version: usize,
) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut output = String::new();

    output.push_str(&format!("--- a/{} (version {})\n", path, from_version));
    output.push_str(&format!("+++ b/{} (version {})\n", path, to_version));

    for hunk in diff.unified_diff().context_radius(3).iter_hunks() {
        output.push_str(&format!("{}", hunk));
    }

    output
}

/// Check if a tool name is a file-modifying tool that should trigger snapshots
pub fn is_file_modifying_tool(tool_name: &str) -> bool {
    matches!(tool_name, "write" | "edit" | "patch")
}

/// Extract the file path from tool arguments for file-modifying tools
pub fn extract_file_path(tool_name: &str, args: &serde_json::Value) -> Option<String> {
    if is_file_modifying_tool(tool_name) {
        args.get("file_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // FileHistory basic operations
    // ========================================================================

    #[test]
    fn test_new_history() {
        let history = FileHistory::new(100);
        assert_eq!(history.total_snapshots(), 0);
        assert!(history.list_files().is_empty());
    }

    #[test]
    fn test_save_snapshot() {
        let history = FileHistory::new(100);
        let v = history.save_snapshot("test.rs", "fn main() {}", "write");
        assert_eq!(v, 0);
        assert_eq!(history.total_snapshots(), 1);
    }

    #[test]
    fn test_save_multiple_snapshots() {
        let history = FileHistory::new(100);
        let v0 = history.save_snapshot("test.rs", "version 0", "write");
        let v1 = history.save_snapshot("test.rs", "version 1", "edit");
        let v2 = history.save_snapshot("test.rs", "version 2", "patch");
        assert_eq!(v0, 0);
        assert_eq!(v1, 1);
        assert_eq!(v2, 2);
        assert_eq!(history.total_snapshots(), 3);
    }

    #[test]
    fn test_save_multiple_files() {
        let history = FileHistory::new(100);
        history.save_snapshot("a.rs", "content a", "write");
        history.save_snapshot("b.rs", "content b", "write");
        assert_eq!(history.total_snapshots(), 2);
        assert_eq!(history.list_files().len(), 2);
    }

    // ========================================================================
    // list_versions
    // ========================================================================

    #[test]
    fn test_list_versions_empty() {
        let history = FileHistory::new(100);
        assert!(history.list_versions("nonexistent.rs").is_empty());
    }

    #[test]
    fn test_list_versions() {
        let history = FileHistory::new(100);
        history.save_snapshot("test.rs", "v0", "write");
        history.save_snapshot("test.rs", "v1", "edit");

        let versions = history.list_versions("test.rs");
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, 0);
        assert_eq!(versions[0].tool_name, "write");
        assert_eq!(versions[0].size, 2);
        assert_eq!(versions[1].version, 1);
        assert_eq!(versions[1].tool_name, "edit");
    }

    // ========================================================================
    // get_version / get_latest
    // ========================================================================

    #[test]
    fn test_get_version() {
        let history = FileHistory::new(100);
        history.save_snapshot("test.rs", "original", "write");
        history.save_snapshot("test.rs", "modified", "edit");

        let v0 = history.get_version("test.rs", 0).unwrap();
        assert_eq!(v0.content, "original");
        assert_eq!(v0.tool_name, "write");

        let v1 = history.get_version("test.rs", 1).unwrap();
        assert_eq!(v1.content, "modified");
    }

    #[test]
    fn test_get_version_nonexistent() {
        let history = FileHistory::new(100);
        assert!(history.get_version("test.rs", 0).is_none());

        history.save_snapshot("test.rs", "content", "write");
        assert!(history.get_version("test.rs", 99).is_none());
    }

    #[test]
    fn test_get_latest() {
        let history = FileHistory::new(100);
        assert!(history.get_latest("test.rs").is_none());

        history.save_snapshot("test.rs", "v0", "write");
        history.save_snapshot("test.rs", "v1", "edit");

        let latest = history.get_latest("test.rs").unwrap();
        assert_eq!(latest.content, "v1");
        assert_eq!(latest.version, 1);
    }

    // ========================================================================
    // diff
    // ========================================================================

    #[test]
    fn test_diff_between_versions() {
        let history = FileHistory::new(100);
        history.save_snapshot("test.rs", "line1\nline2\nline3\n", "write");
        history.save_snapshot("test.rs", "line1\nmodified\nline3\n", "edit");

        let diff = history.diff("test.rs", 0, 1).unwrap();
        assert!(diff.contains("--- a/test.rs (version 0)"));
        assert!(diff.contains("+++ b/test.rs (version 1)"));
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+modified"));
    }

    #[test]
    fn test_diff_nonexistent_version() {
        let history = FileHistory::new(100);
        history.save_snapshot("test.rs", "content", "write");
        assert!(history.diff("test.rs", 0, 5).is_none());
        assert!(history.diff("nonexistent.rs", 0, 1).is_none());
    }

    #[test]
    fn test_diff_same_version() {
        let history = FileHistory::new(100);
        history.save_snapshot("test.rs", "same content\n", "write");

        let diff = history.diff("test.rs", 0, 0).unwrap();
        // Same content should produce minimal diff (just headers)
        assert!(diff.contains("--- a/test.rs"));
        assert!(!diff.contains("-same content"));
    }

    #[test]
    fn test_diff_with_current() {
        let history = FileHistory::new(100);
        history.save_snapshot("test.rs", "old\n", "write");

        let diff = history
            .diff_with_current("test.rs", 0, "new\n")
            .unwrap();
        assert!(diff.contains("-old"));
        assert!(diff.contains("+new"));
    }

    // ========================================================================
    // list_files
    // ========================================================================

    #[test]
    fn test_list_files() {
        let history = FileHistory::new(100);
        history.save_snapshot("a.rs", "a", "write");
        history.save_snapshot("b.rs", "b1", "write");
        history.save_snapshot("b.rs", "b2", "edit");

        let files = history.list_files();
        assert_eq!(files.len(), 2);

        let a_count = files.iter().find(|(p, _)| p == "a.rs").unwrap().1;
        let b_count = files.iter().find(|(p, _)| p == "b.rs").unwrap().1;
        assert_eq!(a_count, 1);
        assert_eq!(b_count, 2);
    }

    // ========================================================================
    // clear operations
    // ========================================================================

    #[test]
    fn test_clear_file() {
        let history = FileHistory::new(100);
        history.save_snapshot("a.rs", "a", "write");
        history.save_snapshot("b.rs", "b", "write");

        history.clear_file("a.rs");
        assert_eq!(history.total_snapshots(), 1);
        assert!(history.list_versions("a.rs").is_empty());
        assert_eq!(history.list_versions("b.rs").len(), 1);
    }

    #[test]
    fn test_clear_all() {
        let history = FileHistory::new(100);
        history.save_snapshot("a.rs", "a", "write");
        history.save_snapshot("b.rs", "b", "write");

        history.clear_all();
        assert_eq!(history.total_snapshots(), 0);
        assert!(history.list_files().is_empty());
    }

    // ========================================================================
    // eviction
    // ========================================================================

    #[test]
    fn test_eviction_when_over_limit() {
        let history = FileHistory::new(3);
        history.save_snapshot("test.rs", "v0", "write");
        history.save_snapshot("test.rs", "v1", "edit");
        history.save_snapshot("test.rs", "v2", "edit");
        // At limit (3), no eviction yet
        assert_eq!(history.total_snapshots(), 3);

        // This should trigger eviction of the oldest
        history.save_snapshot("test.rs", "v3", "edit");
        assert!(history.total_snapshots() <= 3);
    }

    #[test]
    fn test_eviction_across_files() {
        let history = FileHistory::new(3);
        history.save_snapshot("a.rs", "a0", "write");
        history.save_snapshot("b.rs", "b0", "write");
        history.save_snapshot("c.rs", "c0", "write");

        // Adding a 4th should evict the oldest
        history.save_snapshot("d.rs", "d0", "write");
        assert!(history.total_snapshots() <= 3);
    }

    // ========================================================================
    // VersionSummary
    // ========================================================================

    #[test]
    fn test_version_summary_from_snapshot() {
        let snapshot = FileSnapshot {
            version: 5,
            path: "test.rs".to_string(),
            content: "hello world".to_string(),
            timestamp: Utc::now(),
            tool_name: "edit".to_string(),
        };
        let summary = VersionSummary::from(&snapshot);
        assert_eq!(summary.version, 5);
        assert_eq!(summary.path, "test.rs");
        assert_eq!(summary.tool_name, "edit");
        assert_eq!(summary.size, 11); // "hello world".len()
    }

    // ========================================================================
    // Helper functions
    // ========================================================================

    #[test]
    fn test_is_file_modifying_tool() {
        assert!(is_file_modifying_tool("write"));
        assert!(is_file_modifying_tool("edit"));
        assert!(is_file_modifying_tool("patch"));
        assert!(!is_file_modifying_tool("read"));
        assert!(!is_file_modifying_tool("bash"));
        assert!(!is_file_modifying_tool("grep"));
        assert!(!is_file_modifying_tool("glob"));
        assert!(!is_file_modifying_tool("ls"));
    }

    #[test]
    fn test_extract_file_path() {
        let args = serde_json::json!({"file_path": "src/main.rs", "content": "hello"});
        assert_eq!(
            extract_file_path("write", &args),
            Some("src/main.rs".to_string())
        );
        assert_eq!(
            extract_file_path("edit", &args),
            Some("src/main.rs".to_string())
        );
        assert_eq!(
            extract_file_path("patch", &args),
            Some("src/main.rs".to_string())
        );
        assert_eq!(extract_file_path("read", &args), None);
        assert_eq!(extract_file_path("bash", &args), None);
    }

    #[test]
    fn test_extract_file_path_missing() {
        let args = serde_json::json!({"content": "hello"});
        assert_eq!(extract_file_path("write", &args), None);
    }

    // ========================================================================
    // generate_unified_diff
    // ========================================================================

    #[test]
    fn test_generate_unified_diff() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nchanged\nline3\n";
        let diff = generate_unified_diff(old, new, "test.rs", 0, 1);
        assert!(diff.contains("--- a/test.rs (version 0)"));
        assert!(diff.contains("+++ b/test.rs (version 1)"));
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+changed"));
    }

    #[test]
    fn test_generate_unified_diff_no_changes() {
        let content = "same\n";
        let diff = generate_unified_diff(content, content, "test.rs", 0, 0);
        assert!(diff.contains("--- a/test.rs"));
        // No hunks for identical content
        assert!(!diff.contains("@@"));
    }

    #[test]
    fn test_generate_unified_diff_addition() {
        let old = "line1\nline3\n";
        let new = "line1\nline2\nline3\n";
        let diff = generate_unified_diff(old, new, "test.rs", 0, 1);
        assert!(diff.contains("+line2"));
    }

    #[test]
    fn test_generate_unified_diff_deletion() {
        let old = "line1\nline2\nline3\n";
        let new = "line1\nline3\n";
        let diff = generate_unified_diff(old, new, "test.rs", 0, 1);
        assert!(diff.contains("-line2"));
    }
}
