//! In-memory artifact storage for large tool observations.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, RwLock};

const DEFAULT_MAX_ARTIFACTS: usize = 256;
const DEFAULT_MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolArtifact {
    pub artifact_id: String,
    pub artifact_uri: String,
    pub tool_name: String,
    pub content: String,
    pub original_bytes: usize,
    pub shown_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ArtifactStoreSnapshot {
    artifacts: Vec<ToolArtifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactStoreLimits {
    pub max_artifacts: usize,
    pub max_bytes: usize,
}

impl Default for ArtifactStoreLimits {
    fn default() -> Self {
        Self {
            max_artifacts: DEFAULT_MAX_ARTIFACTS,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

#[derive(Debug, Default)]
struct ArtifactStoreState {
    artifacts: HashMap<String, ToolArtifact>,
    insertion_order: VecDeque<String>,
    total_bytes: usize,
}

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    inner: Arc<RwLock<ArtifactStoreState>>,
    limits: ArtifactStoreLimits,
}

impl ArtifactStore {
    pub fn new() -> Self {
        Self::with_limits(ArtifactStoreLimits::default())
    }

    pub fn with_limits(limits: ArtifactStoreLimits) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ArtifactStoreState::default())),
            limits,
        }
    }

    pub fn put(&self, artifact: ToolArtifact) {
        let mut state = self.inner.write().unwrap();
        let artifact_uri = artifact.artifact_uri.clone();
        if let Some(existing) = state.artifacts.remove(&artifact_uri) {
            state.total_bytes = state.total_bytes.saturating_sub(existing.content.len());
            state.insertion_order.retain(|uri| uri != &artifact_uri);
        }

        state.total_bytes += artifact.content.len();
        state.insertion_order.push_back(artifact_uri.clone());
        state.artifacts.insert(artifact_uri, artifact);

        self.enforce_limits(&mut state);
    }

    pub fn get(&self, artifact_uri: &str) -> Option<ToolArtifact> {
        self.inner
            .read()
            .unwrap()
            .artifacts
            .get(artifact_uri)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.inner.read().unwrap().artifacts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn total_bytes(&self) -> usize {
        self.inner.read().unwrap().total_bytes
    }

    pub fn limits(&self) -> ArtifactStoreLimits {
        self.limits
    }

    pub fn artifacts(&self) -> Vec<ToolArtifact> {
        self.ordered_artifacts()
    }

    pub fn save_to_dir(&self, dir: impl AsRef<Path>) -> Result<()> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create artifact directory '{}'", dir.display()))?;
        let snapshot = ArtifactStoreSnapshot {
            artifacts: self.ordered_artifacts(),
        };
        let json = serde_json::to_string_pretty(&snapshot)
            .context("failed to serialize artifact store snapshot")?;
        let path = artifact_manifest_path(dir);
        std::fs::write(&path, json)
            .with_context(|| format!("failed to write artifact manifest '{}'", path.display()))?;
        Ok(())
    }

    pub fn load_from_dir(dir: impl AsRef<Path>) -> Result<Self> {
        Self::load_from_dir_with_limits(dir, ArtifactStoreLimits::default())
    }

    pub fn load_from_dir_with_limits(
        dir: impl AsRef<Path>,
        limits: ArtifactStoreLimits,
    ) -> Result<Self> {
        let path = artifact_manifest_path(dir.as_ref());
        if !path.exists() {
            return Ok(Self::with_limits(limits));
        }

        let json = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read artifact manifest '{}'", path.display()))?;
        let snapshot: ArtifactStoreSnapshot =
            serde_json::from_str(&json).context("failed to parse artifact store snapshot")?;
        let store = Self::with_limits(limits);
        for artifact in snapshot.artifacts {
            store.put(artifact);
        }
        Ok(store)
    }

    fn enforce_limits(&self, state: &mut ArtifactStoreState) {
        while state.artifacts.len() > self.limits.max_artifacts
            || state.total_bytes > self.limits.max_bytes
        {
            let Some(uri) = state.insertion_order.pop_front() else {
                break;
            };
            if let Some(removed) = state.artifacts.remove(&uri) {
                state.total_bytes = state.total_bytes.saturating_sub(removed.content.len());
            }
        }
    }

    fn ordered_artifacts(&self) -> Vec<ToolArtifact> {
        let state = self.inner.read().unwrap();
        state
            .insertion_order
            .iter()
            .filter_map(|uri| state.artifacts.get(uri).cloned())
            .collect()
    }
}

impl Default for ArtifactStore {
    fn default() -> Self {
        Self::new()
    }
}

fn artifact_manifest_path(dir: &Path) -> std::path::PathBuf {
    dir.join("artifacts.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_artifact_store_put_and_get() {
        let store = ArtifactStore::new();
        let artifact = ToolArtifact {
            artifact_id: "tool-output:test:abc".to_string(),
            artifact_uri: "a3s://tool-output/test/abc".to_string(),
            tool_name: "test".to_string(),
            content: "full output".to_string(),
            original_bytes: 11,
            shown_bytes: 4,
        };

        store.put(artifact.clone());

        assert_eq!(store.len(), 1);
        assert_eq!(store.get("a3s://tool-output/test/abc"), Some(artifact));
    }

    #[test]
    fn test_artifact_store_missing_uri() {
        let store = ArtifactStore::new();

        assert!(store.is_empty());
        assert!(store.get("a3s://missing").is_none());
    }

    #[test]
    fn test_artifact_store_evicts_oldest_by_count() {
        let store = ArtifactStore::with_limits(ArtifactStoreLimits {
            max_artifacts: 2,
            max_bytes: 1024,
        });

        for index in 0..3 {
            store.put(ToolArtifact {
                artifact_id: format!("tool-output:test:{index}"),
                artifact_uri: format!("a3s://tool-output/test/{index}"),
                tool_name: "test".to_string(),
                content: format!("artifact {index}"),
                original_bytes: 10,
                shown_bytes: 4,
            });
        }

        assert_eq!(store.len(), 2);
        assert!(store.get("a3s://tool-output/test/0").is_none());
        assert!(store.get("a3s://tool-output/test/1").is_some());
        assert!(store.get("a3s://tool-output/test/2").is_some());
    }

    #[test]
    fn test_artifact_store_evicts_oldest_by_bytes() {
        let store = ArtifactStore::with_limits(ArtifactStoreLimits {
            max_artifacts: 10,
            max_bytes: 8,
        });

        store.put(ToolArtifact {
            artifact_id: "tool-output:test:a".to_string(),
            artifact_uri: "a3s://tool-output/test/a".to_string(),
            tool_name: "test".to_string(),
            content: "aaaa".to_string(),
            original_bytes: 4,
            shown_bytes: 2,
        });
        store.put(ToolArtifact {
            artifact_id: "tool-output:test:b".to_string(),
            artifact_uri: "a3s://tool-output/test/b".to_string(),
            tool_name: "test".to_string(),
            content: "bbbbb".to_string(),
            original_bytes: 5,
            shown_bytes: 2,
        });

        assert_eq!(store.len(), 1);
        assert_eq!(store.total_bytes(), 5);
        assert!(store.get("a3s://tool-output/test/a").is_none());
        assert!(store.get("a3s://tool-output/test/b").is_some());
    }

    #[test]
    fn test_artifact_store_replacing_artifact_updates_order_and_bytes() {
        let store = ArtifactStore::with_limits(ArtifactStoreLimits {
            max_artifacts: 2,
            max_bytes: 1024,
        });

        store.put(ToolArtifact {
            artifact_id: "tool-output:test:a".to_string(),
            artifact_uri: "a3s://tool-output/test/a".to_string(),
            tool_name: "test".to_string(),
            content: "a".to_string(),
            original_bytes: 1,
            shown_bytes: 1,
        });
        store.put(ToolArtifact {
            artifact_id: "tool-output:test:b".to_string(),
            artifact_uri: "a3s://tool-output/test/b".to_string(),
            tool_name: "test".to_string(),
            content: "bb".to_string(),
            original_bytes: 2,
            shown_bytes: 1,
        });
        store.put(ToolArtifact {
            artifact_id: "tool-output:test:a".to_string(),
            artifact_uri: "a3s://tool-output/test/a".to_string(),
            tool_name: "test".to_string(),
            content: "aaaa".to_string(),
            original_bytes: 4,
            shown_bytes: 1,
        });

        assert_eq!(store.len(), 2);
        assert_eq!(store.total_bytes(), 6);
        assert_eq!(
            store.get("a3s://tool-output/test/a").unwrap().content,
            "aaaa"
        );
    }

    #[test]
    fn test_artifact_store_saves_and_loads_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let store = ArtifactStore::with_limits(ArtifactStoreLimits {
            max_artifacts: 10,
            max_bytes: 1024,
        });
        store.put(ToolArtifact {
            artifact_id: "tool-output:test:a".to_string(),
            artifact_uri: "a3s://tool-output/test/a".to_string(),
            tool_name: "test".to_string(),
            content: "artifact content".to_string(),
            original_bytes: 16,
            shown_bytes: 4,
        });

        store.save_to_dir(dir.path()).unwrap();
        let loaded = ArtifactStore::load_from_dir_with_limits(
            dir.path(),
            ArtifactStoreLimits {
                max_artifacts: 10,
                max_bytes: 1024,
            },
        )
        .unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded
                .get("a3s://tool-output/test/a")
                .expect("artifact")
                .content,
            "artifact content"
        );
    }

    #[test]
    fn test_artifact_store_load_missing_manifest_returns_empty_store() {
        let dir = tempfile::tempdir().unwrap();

        let loaded = ArtifactStore::load_from_dir(dir.path()).unwrap();

        assert!(loaded.is_empty());
    }
}
