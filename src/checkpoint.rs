//! Checkpoint management for session state snapshots

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub session_id: String,
    pub label: Option<String>,
    pub created_at: DateTime<Utc>,
    pub files: HashMap<String, String>,
    pub message_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreResult {
    pub checkpoint_id: String,
    pub files_restored: usize,
    pub diffs: Vec<FileDiff>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: String,
    pub diff: String,
}

pub struct CheckpointManager {
    base_dir: PathBuf,
}

impl CheckpointManager {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub async fn create(&self, session_id: &str, label: Option<&str>, files: HashMap<String, String>, message_count: usize) -> anyhow::Result<Checkpoint> {
        let id = uuid::Uuid::new_v4().to_string();
        let checkpoint = Checkpoint {
            id: id.clone(),
            session_id: session_id.to_string(),
            label: label.map(|s| s.to_string()),
            created_at: Utc::now(),
            files,
            message_count,
        };
        let dir = self.checkpoint_dir(session_id)?;
        tokio::fs::create_dir_all(&dir).await?;
        let path = dir.join(format!("{}.json", id));
        let json = serde_json::to_string_pretty(&checkpoint)?;
        tokio::fs::write(&path, json).await?;
        Ok(checkpoint)
    }

    pub async fn list(&self, session_id: &str) -> anyhow::Result<Vec<Checkpoint>> {
        let dir = self.checkpoint_dir(session_id)?;
        if !dir.exists() { return Ok(Vec::new()); }
        let mut checkpoints = Vec::new();
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                let content = tokio::fs::read_to_string(entry.path()).await?;
                if let Ok(cp) = serde_json::from_str::<Checkpoint>(&content) {
                    checkpoints.push(cp);
                }
            }
        }
        checkpoints.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(checkpoints)
    }

    pub async fn get(&self, session_id: &str, checkpoint_id: &str) -> anyhow::Result<Option<Checkpoint>> {
        let path = self.checkpoint_dir(session_id)?.join(format!("{}.json", checkpoint_id));
        if !path.exists() { return Ok(None); }
        let content = tokio::fs::read_to_string(&path).await?;
        Ok(Some(serde_json::from_str(&content)?))
    }

    pub async fn restore_files(&self, checkpoint: &Checkpoint, workspace: &Path) -> anyhow::Result<RestoreResult> {
        let mut diffs = Vec::new();
        let mut files_restored = 0;
        for (rel_path, content) in &checkpoint.files {
            let full_path = workspace.join(rel_path);
            let current = tokio::fs::read_to_string(&full_path).await.unwrap_or_default();
            if current != *content {
                let diff = generate_diff(&current, content, rel_path);
                diffs.push(FileDiff { path: rel_path.clone(), diff });
                if let Some(parent) = full_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&full_path, content).await?;
                files_restored += 1;
            }
        }
        Ok(RestoreResult { checkpoint_id: checkpoint.id.clone(), files_restored, diffs })
    }

    pub async fn diff(&self, checkpoint: &Checkpoint, workspace: &Path) -> anyhow::Result<Vec<FileDiff>> {
        let mut diffs = Vec::new();
        for (rel_path, content) in &checkpoint.files {
            let full_path = workspace.join(rel_path);
            let current = tokio::fs::read_to_string(&full_path).await.unwrap_or_default();
            if current != *content {
                diffs.push(FileDiff { path: rel_path.clone(), diff: generate_diff(&current, content, rel_path) });
            }
        }
        Ok(diffs)
    }

    pub async fn clear(&self, session_id: &str) -> anyhow::Result<usize> {
        let dir = self.checkpoint_dir(session_id)?;
        if !dir.exists() { return Ok(0); }
        let mut count = 0;
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            tokio::fs::remove_file(entry.path()).await?;
            count += 1;
        }
        Ok(count)
    }

    fn checkpoint_dir(&self, session_id: &str) -> anyhow::Result<PathBuf> {
        // Validate session_id to prevent path traversal
        let is_safe = !session_id.is_empty()
            && session_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
            && !session_id.starts_with('.')
            && !session_id.contains("..");
        if !is_safe {
            anyhow::bail!("Invalid session ID for checkpoint path: {session_id:?}");
        }
        Ok(self.base_dir.join("checkpoints").join(session_id))
    }
}

fn generate_diff(old: &str, new: &str, path: &str) -> String {
    use similar::TextDiff;
    let diff = TextDiff::from_lines(old, new);
    diff.unified_diff().header(&format!("a/{}", path), &format!("b/{}", path)).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_serialization() {
        let cp = Checkpoint {
            id: "test-id".to_string(),
            session_id: "sess-1".to_string(),
            label: Some("before refactor".to_string()),
            created_at: Utc::now(),
            files: HashMap::from([("main.rs".to_string(), "fn main() {}".to_string())]),
            message_count: 5,
        };
        let json = serde_json::to_string(&cp).unwrap();
        let parsed: Checkpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "test-id");
        assert_eq!(parsed.files.len(), 1);
    }

    #[test]
    fn test_generate_diff() {
        let diff = generate_diff("line1\nline2\n", "line1\nline3\n", "test.rs");
        assert!(diff.contains("line2"));
        assert!(diff.contains("line3"));
    }

    #[test]
    fn test_generate_diff_identical() {
        let diff = generate_diff("same\n", "same\n", "test.rs");
        assert!(diff.is_empty() || !diff.contains('-'));
    }

    #[tokio::test]
    async fn test_checkpoint_manager_create_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = CheckpointManager::new(dir.path().to_path_buf());
        let files = HashMap::from([("main.rs".to_string(), "fn main() {}".to_string())]);
        let cp = mgr.create("sess-1", Some("test"), files, 3).await.unwrap();
        assert_eq!(cp.session_id, "sess-1");
        let list = mgr.list("sess-1").await.unwrap();
        assert_eq!(list.len(), 1);
    }

    #[tokio::test]
    async fn test_checkpoint_manager_get() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = CheckpointManager::new(dir.path().to_path_buf());
        let cp = mgr.create("sess-1", None, HashMap::new(), 0).await.unwrap();
        let retrieved = mgr.get("sess-1", &cp.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, cp.id);
    }

    #[tokio::test]
    async fn test_checkpoint_manager_get_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = CheckpointManager::new(dir.path().to_path_buf());
        let result = mgr.get("sess-1", "nonexistent").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_checkpoint_manager_clear() {
        let dir = tempfile::tempdir().unwrap();
        let mgr = CheckpointManager::new(dir.path().to_path_buf());
        mgr.create("sess-1", None, HashMap::new(), 0).await.unwrap();
        mgr.create("sess-1", None, HashMap::new(), 0).await.unwrap();
        let cleared = mgr.clear("sess-1").await.unwrap();
        assert_eq!(cleared, 2);
        let list = mgr.list("sess-1").await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn test_checkpoint_restore_files() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let mgr = CheckpointManager::new(dir.path().to_path_buf());
        
        // Write a current file
        tokio::fs::write(workspace.path().join("test.rs"), "old content").await.unwrap();
        
        let files = HashMap::from([("test.rs".to_string(), "new content".to_string())]);
        let cp = mgr.create("sess-1", None, files, 0).await.unwrap();
        let result = mgr.restore_files(&cp, workspace.path()).await.unwrap();
        assert_eq!(result.files_restored, 1);
        
        let content = tokio::fs::read_to_string(workspace.path().join("test.rs")).await.unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn test_checkpoint_diff() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let mgr = CheckpointManager::new(dir.path().to_path_buf());
        
        tokio::fs::write(workspace.path().join("test.rs"), "current").await.unwrap();
        
        let files = HashMap::from([("test.rs".to_string(), "checkpoint".to_string())]);
        let cp = mgr.create("sess-1", None, files, 0).await.unwrap();
        let diffs = mgr.diff(&cp, workspace.path()).await.unwrap();
        assert_eq!(diffs.len(), 1);
    }
}
