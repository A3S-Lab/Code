//! Ripgrep Context Provider
//!
//! Provides indexless, real-time code search using ripgrep-style pattern matching.
//! Unlike vector-based RAG, this approach:
//! - Requires no pre-indexing or embedding generation
//! - Works directly on raw source files
//! - Supports real-time code changes without re-indexing
//! - Uses fast regex-based search with relevance scoring

use crate::context::{ContextItem, ContextProvider, ContextQuery, ContextResult, ContextType};
use async_trait::async_trait;
use ignore::WalkBuilder;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

/// Ripgrep context provider configuration
#[derive(Debug, Clone)]
pub struct RipgrepContextConfig {
    /// Root directory to search
    pub root_path: PathBuf,
    /// Include patterns (glob syntax: ["**/*.rs", "**/*.md"])
    pub include_patterns: Vec<String>,
    /// Exclude patterns (glob syntax: ["**/target/**", "**/node_modules/**"])
    pub exclude_patterns: Vec<String>,
    /// Maximum file size in bytes (default: 1MB)
    pub max_file_size: usize,
    /// Case insensitive search (default: true)
    pub case_insensitive: bool,
    /// Number of context lines before/after match (default: 2)
    pub context_lines: usize,
}

impl RipgrepContextConfig {
    /// Create a new config with default settings
    pub fn new(root_path: impl Into<PathBuf>) -> Self {
        Self {
            root_path: root_path.into(),
            include_patterns: vec![
                "**/*.rs".to_string(),
                "**/*.py".to_string(),
                "**/*.ts".to_string(),
                "**/*.tsx".to_string(),
                "**/*.js".to_string(),
                "**/*.jsx".to_string(),
                "**/*.go".to_string(),
                "**/*.java".to_string(),
                "**/*.c".to_string(),
                "**/*.cpp".to_string(),
                "**/*.h".to_string(),
                "**/*.hpp".to_string(),
                "**/*.md".to_string(),
                "**/*.toml".to_string(),
                "**/*.yaml".to_string(),
                "**/*.yml".to_string(),
                "**/*.json".to_string(),
            ],
            exclude_patterns: vec![
                "**/target/**".to_string(),
                "**/node_modules/**".to_string(),
                "**/.git/**".to_string(),
                "**/dist/**".to_string(),
                "**/build/**".to_string(),
                "**/*.lock".to_string(),
                "**/vendor/**".to_string(),
                "**/__pycache__/**".to_string(),
            ],
            max_file_size: 1024 * 1024, // 1MB
            case_insensitive: true,
            context_lines: 2,
        }
    }

    /// Set include patterns
    pub fn with_include_patterns(mut self, patterns: Vec<String>) -> Self {
        self.include_patterns = patterns;
        self
    }

    /// Set exclude patterns
    pub fn with_exclude_patterns(mut self, patterns: Vec<String>) -> Self {
        self.exclude_patterns = patterns;
        self
    }

    /// Set max file size
    pub fn with_max_file_size(mut self, size: usize) -> Self {
        self.max_file_size = size;
        self
    }

    /// Set case sensitivity
    pub fn with_case_insensitive(mut self, enabled: bool) -> Self {
        self.case_insensitive = enabled;
        self
    }

    /// Set context lines
    pub fn with_context_lines(mut self, lines: usize) -> Self {
        self.context_lines = lines;
        self
    }
}

/// Search result from a single file
#[derive(Debug, Clone)]
struct FileMatch {
    path: PathBuf,
    matches: Vec<MatchResult>,
    relevance: f32,
}

/// A single match within a file
#[derive(Debug, Clone)]
struct MatchResult {
    line_number: usize,
    line_content: String,
    context_before: Vec<String>,
    context_after: Vec<String>,
}

/// Ripgrep context provider
pub struct RipgrepContextProvider {
    config: RipgrepContextConfig,
}

impl RipgrepContextProvider {
    /// Create a new ripgrep context provider
    pub fn new(config: RipgrepContextConfig) -> Self {
        Self { config }
    }

    /// Search files for pattern matches
    async fn search_files(&self, query: &str, max_results: usize) -> anyhow::Result<Vec<FileMatch>> {
        let root = self.config.root_path.clone();
        let max_file_size = self.config.max_file_size;
        let include = self.config.include_patterns.clone();
        let exclude = self.config.exclude_patterns.clone();
        let case_insensitive = self.config.case_insensitive;
        let context_lines = self.config.context_lines;
        let query = query.to_string();

        // Run search in blocking task
        tokio::task::spawn_blocking(move || {
            // Build regex pattern
            let pattern = if case_insensitive {
                format!("(?i){}", regex::escape(&query))
            } else {
                regex::escape(&query)
            };

            let regex = Regex::new(&pattern)?;

            let mut file_matches = Vec::new();

            let walker = WalkBuilder::new(&root)
                .hidden(false)
                .git_ignore(true)
                .build();

            for entry in walker {
                let entry = entry.map_err(|e| anyhow::anyhow!("Walk error: {}", e))?;
                let path = entry.path();

                if !path.is_file() {
                    continue;
                }

                let metadata = fs::metadata(path)
                    .map_err(|e| anyhow::anyhow!("Metadata error for {}: {}", path.display(), e))?;

                if metadata.len() > max_file_size as u64 {
                    continue;
                }

                if !matches_patterns(path, &include, true) {
                    continue;
                }

                if matches_patterns(path, &exclude, false) {
                    continue;
                }

                let content = match fs::read_to_string(path) {
                    Ok(c) => c,
                    Err(_) => continue, // Skip binary files
                };

                if content.trim().is_empty() {
                    continue;
                }

                // Search for matches in this file
                let lines: Vec<&str> = content.lines().collect();
                let mut matches = Vec::new();

                for (line_idx, line) in lines.iter().enumerate() {
                    if regex.is_match(line) {
                        let context_before = if line_idx >= context_lines {
                            lines[line_idx - context_lines..line_idx]
                                .iter()
                                .map(|s| s.to_string())
                                .collect()
                        } else {
                            lines[0..line_idx]
                                .iter()
                                .map(|s| s.to_string())
                                .collect()
                        };

                        let context_after = if line_idx + context_lines < lines.len() {
                            lines[line_idx + 1..=line_idx + context_lines]
                                .iter()
                                .map(|s| s.to_string())
                                .collect()
                        } else {
                            lines[line_idx + 1..]
                                .iter()
                                .map(|s| s.to_string())
                                .collect()
                        };

                        matches.push(MatchResult {
                            line_number: line_idx + 1,
                            line_content: line.to_string(),
                            context_before,
                            context_after,
                        });
                    }
                }

                if !matches.is_empty() {
                    // Calculate relevance score based on match count and file size
                    let relevance = (matches.len() as f32) / (lines.len() as f32).sqrt();

                    file_matches.push(FileMatch {
                        path: path.to_path_buf(),
                        matches,
                        relevance,
                    });
                }
            }

            // Sort by relevance (descending)
            file_matches.sort_by(|a, b| b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal));
            file_matches.truncate(max_results);

            Ok::<_, anyhow::Error>(file_matches)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Spawn blocking failed: {}", e))?
    }

    /// Format match results into context content
    fn format_match(&self, file_match: &FileMatch, depth: &crate::context::ContextDepth) -> String {
        let mut output = String::new();
        let path_str = file_match.path.display().to_string();

        match depth {
            crate::context::ContextDepth::Abstract => {
                // Just show file path and match count
                output.push_str(&format!("{}: {} matches\n", path_str, file_match.matches.len()));
            }
            crate::context::ContextDepth::Overview => {
                // Show first few matches with limited context
                output.push_str(&format!("{}:\n", path_str));
                for (idx, m) in file_match.matches.iter().take(3).enumerate() {
                    if idx > 0 {
                        output.push_str("\n");
                    }
                    output.push_str(&format!("  Line {}:\n", m.line_number));
                    output.push_str(&format!("    {}\n", m.line_content));
                }
                if file_match.matches.len() > 3 {
                    output.push_str(&format!("  ... and {} more matches\n", file_match.matches.len() - 3));
                }
            }
            crate::context::ContextDepth::Full => {
                // Show all matches with full context
                output.push_str(&format!("{}:\n", path_str));
                for (idx, m) in file_match.matches.iter().enumerate() {
                    if idx > 0 {
                        output.push_str("\n");
                    }
                    output.push_str(&format!("  Line {}:\n", m.line_number));
                    for ctx in &m.context_before {
                        output.push_str(&format!("    {}\n", ctx));
                    }
                    output.push_str(&format!("  > {}\n", m.line_content));
                    for ctx in &m.context_after {
                        output.push_str(&format!("    {}\n", ctx));
                    }
                }
            }
        }

        output
    }
}

#[async_trait]
impl ContextProvider for RipgrepContextProvider {
    fn name(&self) -> &str {
        "ripgrep"
    }

    async fn query(&self, query: &ContextQuery) -> anyhow::Result<ContextResult> {
        let file_matches = self.search_files(&query.query, query.max_results).await?;

        let mut result = ContextResult::new("ripgrep");
        let mut total_tokens = 0usize;

        for file_match in file_matches {
            if total_tokens >= query.max_tokens {
                result.truncated = true;
                break;
            }

            let content = self.format_match(&file_match, &query.depth);
            let token_count = content.split_whitespace().count();

            if total_tokens + token_count > query.max_tokens {
                result.truncated = true;
                break;
            }

            total_tokens += token_count;

            result.add_item(
                ContextItem::new(
                    file_match.path.to_string_lossy().to_string(),
                    ContextType::Resource,
                    content,
                )
                .with_token_count(token_count)
                .with_relevance(file_match.relevance)
                .with_source(&format!("file:{}", file_match.path.display()))
                .with_metadata("match_count", serde_json::json!(file_match.matches.len())),
            );
        }

        Ok(result)
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Check if a path matches any of the given glob patterns
fn matches_patterns(path: &Path, patterns: &[String], default_if_empty: bool) -> bool {
    if patterns.is_empty() {
        return default_if_empty;
    }
    let path_str = path.to_string_lossy().replace('\\', "/");
    patterns.iter().any(|pattern| {
        glob::Pattern::new(pattern)
            .map(|p| p.matches(&path_str))
            .unwrap_or(false)
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_test_workspace() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Create test files
        let mut f1 = File::create(root.join("main.rs")).unwrap();
        writeln!(f1, "fn main() {{\n    println!(\"Hello, world!\");\n}}").unwrap();

        let mut f2 = File::create(root.join("lib.rs")).unwrap();
        writeln!(
            f2,
            "pub mod auth;\npub mod database;\n\npub fn init() -> Result<()> {{\n    Ok(())\n}}"
        )
        .unwrap();

        let mut f3 = File::create(root.join("README.md")).unwrap();
        writeln!(
            f3,
            "# My Project\n\nA Rust project for testing ripgrep context."
        )
        .unwrap();

        std::fs::create_dir(root.join("src")).unwrap();
        let mut f4 = File::create(root.join("src/auth.rs")).unwrap();
        writeln!(
            f4,
            "use jwt::Token;\n\npub fn verify_token(token: &str) -> Result<Claims> {{\n    // JWT verification logic\n    todo!()\n}}"
        )
        .unwrap();

        dir
    }

    #[test]
    fn test_config_defaults() {
        let config = RipgrepContextConfig::new("/tmp/test");
        assert_eq!(config.root_path, PathBuf::from("/tmp/test"));
        assert!(!config.include_patterns.is_empty());
        assert!(!config.exclude_patterns.is_empty());
        assert_eq!(config.max_file_size, 1024 * 1024);
        assert!(config.case_insensitive);
        assert_eq!(config.context_lines, 2);
    }

    #[test]
    fn test_config_builders() {
        let config = RipgrepContextConfig::new("/tmp")
            .with_include_patterns(vec!["**/*.rs".to_string()])
            .with_exclude_patterns(vec!["**/test/**".to_string()])
            .with_max_file_size(2048)
            .with_case_insensitive(false)
            .with_context_lines(5);

        assert_eq!(config.include_patterns, vec!["**/*.rs"]);
        assert_eq!(config.exclude_patterns, vec!["**/test/**"]);
        assert_eq!(config.max_file_size, 2048);
        assert!(!config.case_insensitive);
        assert_eq!(config.context_lines, 5);
    }

    #[tokio::test]
    async fn test_provider_search() {
        let dir = setup_test_workspace();
        let config = RipgrepContextConfig::new(dir.path());
        let provider = RipgrepContextProvider::new(config);

        let query = ContextQuery::new("Rust");
        let result = provider.query(&query).await.unwrap();

        assert_eq!(result.provider, "ripgrep");
        assert!(!result.items.is_empty());
        // Should find "Rust" in README.md
        assert!(result.items.iter().any(|item| item.content.contains("Rust")));
    }

    #[tokio::test]
    async fn test_provider_case_insensitive() {
        let dir = setup_test_workspace();
        let config = RipgrepContextConfig::new(dir.path()).with_case_insensitive(true);
        let provider = RipgrepContextProvider::new(config);

        let query = ContextQuery::new("RUST");
        let result = provider.query(&query).await.unwrap();

        assert!(!result.items.is_empty());
    }

    #[tokio::test]
    async fn test_provider_max_results() {
        let dir = setup_test_workspace();
        let config = RipgrepContextConfig::new(dir.path());
        let provider = RipgrepContextProvider::new(config);

        let query = ContextQuery::new("fn").with_max_results(1);
        let result = provider.query(&query).await.unwrap();

        assert!(result.items.len() <= 1);
    }

    #[tokio::test]
    async fn test_provider_name() {
        let dir = TempDir::new().unwrap();
        let config = RipgrepContextConfig::new(dir.path());
        let provider = RipgrepContextProvider::new(config);
        assert_eq!(provider.name(), "ripgrep");
    }

    #[test]
    fn test_matches_patterns_empty_default_true() {
        assert!(matches_patterns(Path::new("test.rs"), &[], true));
    }

    #[test]
    fn test_matches_patterns_empty_default_false() {
        assert!(!matches_patterns(Path::new("test.rs"), &[], false));
    }

    #[test]
    fn test_matches_patterns_include() {
        let patterns = vec!["**/*.rs".to_string()];
        assert!(matches_patterns(Path::new("src/main.rs"), &patterns, false));
        assert!(!matches_patterns(Path::new("src/main.py"), &patterns, false));
    }
}
