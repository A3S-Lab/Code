//! Tool Search — Semantic tool matching for dynamic MCP tool loading
//!
//! When the MCP ecosystem grows large (100+ tools), injecting all tool
//! descriptions into the system prompt wastes context. Tool Search selects
//! only the relevant tools per-turn based on keyword and semantic matching.
//!
//! ## How It Works
//!
//! 1. All registered tools (builtin + MCP) are indexed with their name,
//!    description, and parameter schema.
//! 2. Before each LLM call, the user prompt is matched against the index.
//! 3. Only tools scoring above the threshold are included in the request.
//! 4. Builtin tools are always included (they're small and essential).
//!
//! ## Usage
//!
//! ```rust,no_run
//! use a3s_code_core::tool_search::{ToolIndex, ToolSearchConfig};
//!
//! let config = ToolSearchConfig::default();
//! let mut index = ToolIndex::new(config);
//!
//! // Index tools
//! index.add("mcp__github__create_issue", "Create a GitHub issue", &["github", "issue", "bug"]);
//! index.add("mcp__postgres__query", "Run a SQL query", &["sql", "database", "query"]);
//!
//! // Search
//! let matches = index.search("create a bug report on GitHub", 5);
//! // → ["mcp__github__create_issue"]
//! ```

use std::collections::HashMap;

/// Configuration for tool search behavior.
#[derive(Debug, Clone)]
pub struct ToolSearchConfig {
    /// Minimum relevance score (0.0–1.0) for a tool to be included.
    /// Default: 0.3
    pub threshold: f32,
    /// Maximum number of MCP tools to include per turn.
    /// Default: 20
    pub max_tools: usize,
    /// Always include builtin tools regardless of score.
    /// Default: true
    pub always_include_builtins: bool,
    /// Enable tool search. When false, all tools are included (legacy behavior).
    /// Default: true
    pub enabled: bool,
}

impl Default for ToolSearchConfig {
    fn default() -> Self {
        Self {
            threshold: 0.3,
            max_tools: 20,
            always_include_builtins: true,
            enabled: true,
        }
    }
}

/// An indexed tool entry.
#[derive(Debug, Clone)]
struct ToolEntry {
    /// Tool name (e.g., "mcp__github__create_issue").
    name: String,
    /// Tool description.
    description: String,
    /// Searchable keywords (extracted from name, description, params).
    keywords: Vec<String>,
    /// Whether this is a builtin tool.
    is_builtin: bool,
}

/// A scored search result.
#[derive(Debug, Clone)]
pub struct ToolMatch {
    /// Tool name.
    pub name: String,
    /// Relevance score (0.0–1.0).
    pub score: f32,
    /// Whether this is a builtin tool.
    pub is_builtin: bool,
}

/// Index of all registered tools for semantic search.
#[derive(Clone)]
pub struct ToolIndex {
    config: ToolSearchConfig,
    entries: HashMap<String, ToolEntry>,
}

impl ToolIndex {
    /// Create a new tool index with the given configuration.
    pub fn new(config: ToolSearchConfig) -> Self {
        Self {
            config,
            entries: HashMap::new(),
        }
    }

    /// Add a tool to the index.
    pub fn add(&mut self, name: &str, description: &str, extra_keywords: &[&str]) {
        let is_builtin = !name.starts_with("mcp__");

        // Extract keywords from name and description
        let mut keywords: Vec<String> = Vec::new();

        // Split tool name by underscores and double-underscores
        for part in name.split("__").flat_map(|s| s.split('_')) {
            if part.len() >= 2 {
                keywords.push(part.to_lowercase());
            }
        }

        // Extract words from description
        for word in description.split_whitespace() {
            let clean = word
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase();
            if clean.len() >= 3 {
                keywords.push(clean);
            }
        }

        // Add extra keywords
        for kw in extra_keywords {
            keywords.push(kw.to_lowercase());
        }

        self.entries.insert(
            name.to_string(),
            ToolEntry {
                name: name.to_string(),
                description: description.to_string(),
                keywords,
                is_builtin,
            },
        );
    }

    /// Remove a tool from the index.
    pub fn remove(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }

    /// Search for tools relevant to the given query.
    ///
    /// Returns tools sorted by relevance score (highest first),
    /// limited to `max_results` entries.
    pub fn search(&self, query: &str, max_results: usize) -> Vec<ToolMatch> {
        if !self.config.enabled {
            // Return all tools when search is disabled
            return self
                .entries
                .values()
                .map(|e| ToolMatch {
                    name: e.name.clone(),
                    score: 1.0,
                    is_builtin: e.is_builtin,
                })
                .collect();
        }

        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            // Empty query: return builtins only
            return self
                .entries
                .values()
                .filter(|e| e.is_builtin)
                .map(|e| ToolMatch {
                    name: e.name.clone(),
                    score: 1.0,
                    is_builtin: true,
                })
                .collect();
        }

        let mut matches: Vec<ToolMatch> = self
            .entries
            .values()
            .map(|entry| {
                let score = compute_relevance(&query_tokens, entry);
                ToolMatch {
                    name: entry.name.clone(),
                    score,
                    is_builtin: entry.is_builtin,
                }
            })
            .filter(|m| {
                if self.config.always_include_builtins && m.is_builtin {
                    true
                } else {
                    m.score >= self.config.threshold
                }
            })
            .collect();

        // Sort by score descending
        matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Limit results
        let limit = max_results.min(self.config.max_tools);
        matches.truncate(limit);

        matches
    }

    /// Number of indexed tools.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all tool names in the index.
    pub fn tool_names(&self) -> Vec<&str> {
        self.entries.keys().map(|s| s.as_str()).collect()
    }
}

/// Tokenize a query string into lowercase words.
fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|w| w.len() >= 2)
        .collect()
}

/// Compute relevance score between query tokens and a tool entry.
fn compute_relevance(query_tokens: &[String], entry: &ToolEntry) -> f32 {
    if query_tokens.is_empty() || entry.keywords.is_empty() {
        return 0.0;
    }

    let mut matched = 0u32;
    let mut partial = 0u32;

    for qt in query_tokens {
        // Exact keyword match
        if entry.keywords.iter().any(|kw| kw == qt) {
            matched += 2;
        }
        // Substring match (query token contained in keyword or vice versa)
        else if entry
            .keywords
            .iter()
            .any(|kw| kw.contains(qt.as_str()) || qt.contains(kw.as_str()))
        {
            partial += 1;
        }
        // Tool name contains the token
        else if entry.name.to_lowercase().contains(qt.as_str()) {
            partial += 1;
        }
    }

    let total_score = (matched as f32 * 1.0) + (partial as f32 * 0.5);
    let max_possible = query_tokens.len() as f32 * 2.0;

    (total_score / max_possible).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_index() -> ToolIndex {
        let mut index = ToolIndex::new(ToolSearchConfig::default());
        // Builtins
        index.add(
            "bash",
            "Execute shell commands",
            &["shell", "terminal", "run"],
        );
        index.add("read", "Read file contents", &["file", "open", "cat"]);
        index.add(
            "write",
            "Write content to a file",
            &["file", "save", "create"],
        );
        index.add(
            "edit",
            "Edit a file with search and replace",
            &["modify", "change", "replace"],
        );
        index.add(
            "grep",
            "Search file contents",
            &["search", "find", "pattern"],
        );
        index.add("glob", "Find files by pattern", &["find", "files", "match"]);
        // MCP tools
        index.add(
            "mcp__github__create_issue",
            "Create a GitHub issue",
            &["github", "issue", "bug", "ticket"],
        );
        index.add(
            "mcp__github__list_prs",
            "List pull requests",
            &["github", "pull", "request", "pr"],
        );
        index.add(
            "mcp__postgres__query",
            "Execute a SQL query against PostgreSQL",
            &["sql", "database", "postgres", "db"],
        );
        index.add(
            "mcp__fetch__fetch",
            "Fetch a URL and return its content",
            &["http", "url", "web", "download"],
        );
        index.add(
            "mcp__sentry__get_issues",
            "Get issues from Sentry",
            &["sentry", "error", "monitoring", "crash"],
        );
        index
    }

    #[test]
    fn test_search_github() {
        let index = build_index();
        let matches = index.search("create a bug report on GitHub", 10);
        let names: Vec<&str> = matches.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"mcp__github__create_issue"));
    }

    #[test]
    fn test_search_database() {
        let index = build_index();
        let matches = index.search("run a SQL query on the database", 10);
        let names: Vec<&str> = matches.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"mcp__postgres__query"));
    }

    #[test]
    fn test_search_web() {
        let index = build_index();
        let matches = index.search("fetch the URL content", 10);
        let mcp_matches: Vec<&str> = matches
            .iter()
            .filter(|m| !m.is_builtin)
            .map(|m| m.name.as_str())
            .collect();
        assert!(mcp_matches.contains(&"mcp__fetch__fetch"));
    }

    #[test]
    fn test_builtins_always_included() {
        let index = build_index();
        let matches = index.search("create a GitHub issue", 20);
        let builtins: Vec<&str> = matches
            .iter()
            .filter(|m| m.is_builtin)
            .map(|m| m.name.as_str())
            .collect();
        // All builtins should be present
        assert!(builtins.contains(&"bash"));
        assert!(builtins.contains(&"read"));
    }

    #[test]
    fn test_empty_query_returns_builtins() {
        let index = build_index();
        let matches = index.search("", 20);
        assert!(matches.iter().all(|m| m.is_builtin));
    }

    #[test]
    fn test_disabled_returns_all() {
        let mut index = build_index();
        index.config.enabled = false;
        let matches = index.search("anything", 100);
        assert_eq!(matches.len(), index.len());
    }

    #[test]
    fn test_max_results_limit() {
        let index = build_index();
        let matches = index.search("file search", 3);
        assert!(matches.len() <= 3);
    }

    #[test]
    fn test_remove_tool() {
        let mut index = build_index();
        let before = index.len();
        assert!(index.remove("mcp__sentry__get_issues"));
        assert_eq!(index.len(), before - 1);
        assert!(!index.remove("nonexistent"));
    }

    #[test]
    fn test_threshold_filtering() {
        let config = ToolSearchConfig {
            threshold: 0.9,
            always_include_builtins: false,
            ..Default::default()
        };
        let mut index = ToolIndex::new(config);
        index.add("mcp__foo__bar", "Completely unrelated tool", &["xyz"]);
        let matches = index.search("github issue", 10);
        // High threshold + unrelated tool = no matches
        assert!(matches.is_empty());
    }
}
