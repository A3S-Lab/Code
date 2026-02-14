//! Project memory file path helpers

use std::path::PathBuf;

/// Get the memory directory for a project
pub fn memory_dir(project_name: &str) -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".a3s").join("memory").join(project_name)
}

/// Get the memory file path for a project
pub fn memory_file(project_name: &str) -> PathBuf {
    memory_dir(project_name).join("memories.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_dir() {
        let dir = memory_dir("test-project");
        assert!(dir.to_string_lossy().contains(".a3s"));
        assert!(dir.to_string_lossy().contains("memory"));
        assert!(dir.to_string_lossy().contains("test-project"));
    }

    #[test]
    fn test_memory_file() {
        let file = memory_file("test-project");
        assert!(file.to_string_lossy().ends_with("memories.jsonl"));
    }
}
