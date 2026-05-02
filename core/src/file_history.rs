//! File version history tracking
//!
//! Automatically captures file snapshots before modifications (write, edit, patch).
//! Provides lightweight version listing for internal diagnostics.
//!
//! ## Design
//!
//! - Per-session file history stored in memory
//! - Snapshots captured before each file-modifying tool execution

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

use crate::error::{read_or_recover, write_or_recover};

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
        let mut snapshots = write_or_recover(&self.snapshots);

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
        let snapshots = read_or_recover(&self.snapshots);
        snapshots
            .get(path)
            .map(|versions| versions.iter().map(VersionSummary::from).collect())
            .unwrap_or_default()
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

    #[test]
    fn new_history_starts_empty() {
        let history = FileHistory::new(100);
        assert!(history.list_versions("missing.rs").is_empty());
    }

    #[test]
    fn save_snapshot_records_summary() {
        let history = FileHistory::new(100);
        let v = history.save_snapshot("test.rs", "fn main() {}", "write");
        assert_eq!(v, 0);

        let versions = history.list_versions("test.rs");
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, 0);
        assert_eq!(versions[0].path, "test.rs");
        assert_eq!(versions[0].tool_name, "write");
        assert_eq!(versions[0].size, "fn main() {}".len());
    }

    #[test]
    fn save_multiple_snapshots_keeps_versions_per_file() {
        let history = FileHistory::new(100);
        let v0 = history.save_snapshot("test.rs", "version 0", "write");
        let v1 = history.save_snapshot("test.rs", "version 1", "edit");
        let v2 = history.save_snapshot("test.rs", "version 2", "patch");
        assert_eq!(v0, 0);
        assert_eq!(v1, 1);
        assert_eq!(v2, 2);

        let versions = history.list_versions("test.rs");
        assert_eq!(versions.len(), 3);
        assert_eq!(versions[0].version, 0);
        assert_eq!(versions[1].version, 1);
        assert_eq!(versions[2].version, 2);
    }

    #[test]
    fn eviction_keeps_history_within_limit() {
        let history = FileHistory::new(2);
        history.save_snapshot("test.rs", "v0", "write");
        history.save_snapshot("test.rs", "v1", "edit");
        history.save_snapshot("test.rs", "v2", "patch");

        assert!(history.list_versions("test.rs").len() <= 2);
    }

    #[test]
    fn version_summary_is_content_free() {
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
        assert_eq!(summary.size, 11);
    }

    #[test]
    fn detects_file_modifying_tools() {
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
    fn extracts_file_path_only_for_modifying_tools() {
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
    fn missing_file_path_returns_none() {
        let args = serde_json::json!({"content": "hello"});
        assert_eq!(extract_file_path("write", &args), None);
    }
}
