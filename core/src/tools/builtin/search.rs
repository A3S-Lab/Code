//! Unified workspace search tool.

use super::{
    bm25::Bm25Tool, glob_tool::GlobTool, grep::GrepTool, hybrid_search::HybridSearchTool,
    semantic_search::SemanticSearchTool,
};
use crate::tools::types::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;

const MAX_PAGE_LIMIT: usize = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SearchMode {
    Grep,
    Glob,
    Bm25,
    Indexed,
    Semantic,
    Hybrid,
}

impl SearchMode {
    fn parse(args: &serde_json::Value) -> std::result::Result<Self, String> {
        match args.get("mode").and_then(serde_json::Value::as_str) {
            Some("grep") => Ok(Self::Grep),
            Some("glob") => Ok(Self::Glob),
            Some("bm25") => Ok(Self::Bm25),
            Some("indexed") => Ok(Self::Indexed),
            Some("semantic") => Ok(Self::Semantic),
            Some("hybrid") => Ok(Self::Hybrid),
            Some(_) => Err(
                "mode must be 'grep', 'glob', 'bm25', 'indexed', 'semantic', or 'hybrid'"
                    .to_string(),
            ),
            None => Err("mode parameter is required".to_string()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Grep => "grep",
            Self::Glob => "glob",
            Self::Bm25 => "bm25",
            Self::Indexed => "indexed",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
        }
    }
}

/// Model-facing workspace search abstraction.
///
/// Grep, glob, and BM25 remain separate internal implementations because they
/// have different backend and rendering behavior. The model sees one stable
/// tool contract and selects the behavior with `mode`.
pub struct SearchTool {
    backend_search_enabled: bool,
    read_enabled: bool,
    semantic_enabled: bool,
    indexed_enabled: bool,
}

impl SearchTool {
    pub fn new(read_enabled: bool) -> Self {
        Self {
            backend_search_enabled: true,
            read_enabled,
            semantic_enabled: false,
            indexed_enabled: false,
        }
    }

    pub fn with_backend_search(mut self, enabled: bool) -> Self {
        self.backend_search_enabled = enabled;
        self
    }

    pub fn with_semantic(mut self, enabled: bool) -> Self {
        self.semantic_enabled = enabled;
        self
    }

    pub fn with_indexed(mut self, enabled: bool) -> Self {
        self.indexed_enabled = enabled;
        self
    }

    fn modes(&self) -> Vec<&'static str> {
        let mut modes = Vec::new();
        if self.backend_search_enabled {
            modes.extend(["grep", "glob"]);
        }
        if self.backend_search_enabled && self.read_enabled {
            modes.push("bm25");
        }
        if self.indexed_enabled {
            modes.push("indexed");
        }
        if self.semantic_enabled {
            modes.extend(["semantic", "hybrid"]);
        }
        modes
    }

    fn adapted_args(
        &self,
        mode: SearchMode,
        args: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, String> {
        if matches!(mode, SearchMode::Grep | SearchMode::Glob) && !self.backend_search_enabled {
            return Err(format!(
                "mode='{}' is unavailable because this workspace backend did not provide search",
                mode.as_str()
            ));
        }
        if mode == SearchMode::Bm25 && !self.read_enabled {
            return Err(
                "mode='bm25' is unavailable because this workspace backend did not provide file reads"
                    .to_string(),
            );
        }
        if mode == SearchMode::Indexed && !self.indexed_enabled {
            return Err(
                "mode='indexed' is unavailable because this workspace did not enable a persistent index"
                    .to_string(),
            );
        }
        if matches!(mode, SearchMode::Semantic | SearchMode::Hybrid) && !self.semantic_enabled {
            return Err(format!(
                "mode='{}' is unavailable because this session did not enable semantic retrieval",
                mode.as_str()
            ));
        }

        let query = args
            .get("query")
            .and_then(serde_json::Value::as_str)
            .filter(|query| !query.trim().is_empty())
            .ok_or_else(|| "query parameter is required".to_string())?;
        let mut adapted = serde_json::Map::new();
        adapted.insert(
            match mode {
                SearchMode::Grep | SearchMode::Glob => "pattern",
                SearchMode::Bm25
                | SearchMode::Indexed
                | SearchMode::Semantic
                | SearchMode::Hybrid => "query",
            }
            .to_string(),
            serde_json::Value::String(query.to_string()),
        );
        copy_if_present(args, &mut adapted, "path", "path");

        match mode {
            SearchMode::Grep => {
                if !self.read_enabled
                    && args.get("output_mode").and_then(serde_json::Value::as_str) == Some("count")
                {
                    return Err(
                        "output_mode='count' is unavailable because this workspace backend did not provide file reads"
                            .to_string(),
                    );
                }
                copy_if_present(args, &mut adapted, "include", "glob");
                copy_if_present(args, &mut adapted, "context", "context");
                copy_if_present(args, &mut adapted, "output_mode", "output_mode");
                copy_if_present(args, &mut adapted, "limit", "limit");
                copy_if_present(args, &mut adapted, "cursor", "cursor");
                if let Some(case_sensitive) = args
                    .get("case_sensitive")
                    .and_then(serde_json::Value::as_bool)
                {
                    adapted.insert("-i".to_string(), serde_json::Value::Bool(!case_sensitive));
                }
            }
            SearchMode::Glob => {
                copy_if_present(args, &mut adapted, "limit", "limit");
                copy_if_present(args, &mut adapted, "cursor", "cursor");
                copy_if_present(args, &mut adapted, "sort", "sort");
            }
            SearchMode::Bm25 => {
                copy_if_present(args, &mut adapted, "include", "glob");
                copy_if_present(args, &mut adapted, "context", "context");
                copy_if_present(args, &mut adapted, "limit", "limit");
            }
            SearchMode::Indexed => {
                copy_if_present(args, &mut adapted, "include", "glob");
                copy_if_present(args, &mut adapted, "context", "context");
                copy_if_present(args, &mut adapted, "limit", "limit");
                adapted.insert(
                    "_persistent_index".to_owned(),
                    serde_json::Value::Bool(true),
                );
            }
            SearchMode::Semantic | SearchMode::Hybrid => {
                copy_if_present(args, &mut adapted, "include", "include");
                copy_if_present(args, &mut adapted, "limit", "limit");
            }
        }

        Ok(serde_json::Value::Object(adapted))
    }
}

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search"
    }

    fn description(&self) -> &str {
        "Search the workspace with regex, glob, BM25 lexical ranking, a persistent zvec index, or session-scoped semantic similarity. Select the behavior with mode."
    }

    fn parameters(&self) -> serde_json::Value {
        let modes = self.modes();
        let mut examples = vec![
            serde_json::json!({
                "mode": "grep",
                "query": "TODO|FIXME",
                "path": "core/src",
                "include": "*.rs",
                "context": 2
            }),
            serde_json::json!({
                "mode": "glob",
                "query": "**/*.md",
                "sort": "path"
            }),
        ];
        if self.backend_search_enabled && self.read_enabled {
            examples.push(serde_json::json!({
                "mode": "bm25",
                "query": "workspace permission policy",
                "path": "core/src",
                "limit": 8
            }));
        }
        if self.indexed_enabled {
            examples.push(serde_json::json!({
                "mode": "indexed",
                "query": "workspace permission policy",
                "path": "core/src",
                "limit": 8
            }));
        }
        if self.semantic_enabled {
            examples.push(serde_json::json!({
                "mode": "hybrid",
                "query": "where session shutdown releases temporary indexes",
                "path": "core/src",
                "limit": 8
            }));
            examples.push(serde_json::json!({
                "mode": "semantic",
                "query": "where session shutdown releases temporary indexes",
                "path": "core/src",
                "limit": 8
            }));
        }
        let grep_output_modes = if self.backend_search_enabled && self.read_enabled {
            vec!["content", "files_with_matches", "count", "summary"]
        } else {
            vec!["content", "files_with_matches", "summary"]
        };
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": modes,
                    "description": "Required. grep searches file contents with a regular expression; glob finds paths; bm25 ranks chunks lexically; indexed searches the persistent workspace FTS index; semantic ranks by meaning; hybrid fuses exact, lexical, symbol, and semantic evidence."
                },
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Required. A regular expression in grep mode, a glob pattern in glob mode, or a plain-text relevance query in bm25/indexed/semantic/hybrid mode."
                },
                "path": {
                    "type": "string",
                    "description": "Optional. Workspace-relative directory or file to search. Default: workspace root."
                },
                "include": {
                    "type": "string",
                    "description": "Optional for grep/bm25/semantic/hybrid. Glob pattern used to filter candidate files, for example '*.rs' or '*.{ts,tsx}'."
                },
                "context": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "Optional for grep/bm25. Context lines around a match. BM25 defaults to 2 and allows at most 8."
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Optional for grep. Whether matching is case-sensitive. Default: true."
                },
                "output_mode": {
                    "type": "string",
                    "enum": grep_output_modes,
                    "description": "Optional for grep. content returns matches (default); files_with_matches and count are paginated; summary returns totals."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_PAGE_LIMIT,
                    "description": "Optional. Page size for glob or paginated grep (default 200, maximum 1000), or result count for bm25/semantic/hybrid (default 10, maximum 25)."
                },
                "cursor": {
                    "type": "string",
                    "description": "Optional for glob and paginated grep. Copy the exact opaque cursor from the previous result."
                },
                "sort": {
                    "type": "string",
                    "enum": ["path", "backend"],
                    "description": "Optional for glob. backend (default) preserves backend order; path applies lexical ordering before pagination."
                }
            },
            "required": ["mode", "query"],
            "examples": examples
        })
    }

    fn capabilities(&self, args: &serde_json::Value) -> ToolCapabilities {
        match SearchMode::parse(args) {
            Ok(SearchMode::Glob) => ToolCapabilities::read_only_paginated(16),
            Ok(SearchMode::Grep)
                if matches!(
                    args.get("output_mode").and_then(serde_json::Value::as_str),
                    Some("files_with_matches" | "count")
                ) =>
            {
                ToolCapabilities::read_only_paginated(16)
            }
            Ok(
                SearchMode::Bm25 | SearchMode::Indexed | SearchMode::Semantic | SearchMode::Hybrid,
            ) => ToolCapabilities::parallel_safe_read(2),
            Ok(SearchMode::Grep) | Err(_) => ToolCapabilities::parallel_safe_read(16),
        }
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let mode = match SearchMode::parse(args) {
            Ok(mode) => mode,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        let adapted = match self.adapted_args(mode, args) {
            Ok(adapted) => adapted,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        let output = match mode {
            SearchMode::Grep => GrepTool.execute(&adapted, ctx).await?,
            SearchMode::Glob => GlobTool.execute(&adapted, ctx).await?,
            SearchMode::Bm25 => Bm25Tool.execute(&adapted, ctx).await?,
            SearchMode::Indexed => Bm25Tool.execute(&adapted, ctx).await?,
            SearchMode::Semantic => SemanticSearchTool.execute(&adapted, ctx).await?,
            SearchMode::Hybrid => HybridSearchTool.execute(&adapted, ctx).await?,
        };
        Ok(with_search_mode(output, mode))
    }
}

fn copy_if_present(
    source: &serde_json::Value,
    destination: &mut serde_json::Map<String, serde_json::Value>,
    source_name: &str,
    destination_name: &str,
) {
    if let Some(value) = source.get(source_name).filter(|value| !value.is_null()) {
        destination.insert(destination_name.to_string(), value.clone());
    }
}

fn with_search_mode(mut output: ToolOutput, mode: SearchMode) -> ToolOutput {
    match output.metadata.as_mut() {
        Some(serde_json::Value::Object(metadata)) => {
            metadata.insert("mode".to_string(), serde_json::json!(mode.as_str()));
        }
        Some(metadata) => {
            let previous = std::mem::take(metadata);
            *metadata = serde_json::json!({
                "mode": mode.as_str(),
                "details": previous,
            });
        }
        None => {
            output.metadata = Some(serde_json::json!({ "mode": mode.as_str() }));
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn schema_exposes_only_supported_modes() {
        assert_eq!(
            SearchTool::new(false).parameters()["properties"]["mode"]["enum"],
            serde_json::json!(["grep", "glob"])
        );
        assert_eq!(
            SearchTool::new(true).parameters()["properties"]["mode"]["enum"],
            serde_json::json!(["grep", "glob", "bm25"])
        );
        assert_eq!(
            SearchTool::new(true).with_semantic(true).parameters()["properties"]["mode"]["enum"],
            serde_json::json!(["grep", "glob", "bm25", "semantic", "hybrid"])
        );
        assert_eq!(
            SearchTool::new(true)
                .with_backend_search(false)
                .with_semantic(true)
                .parameters()["properties"]["mode"]["enum"],
            serde_json::json!(["semantic", "hybrid"])
        );
        assert_eq!(
            SearchTool::new(false).parameters()["examples"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            SearchTool::new(false).parameters()["properties"]["output_mode"]["enum"],
            serde_json::json!(["content", "files_with_matches", "summary"])
        );
    }

    #[test]
    fn unified_schema_is_smaller_than_three_separate_tool_schemas() {
        let separate = serde_json::to_vec(&GrepTool.parameters()).unwrap().len()
            + serde_json::to_vec(&GlobTool.parameters()).unwrap().len()
            + serde_json::to_vec(&Bm25Tool.parameters()).unwrap().len();
        let unified = serde_json::to_vec(&SearchTool::new(true).parameters())
            .unwrap()
            .len();
        assert!(
            unified < separate,
            "unified schema should save context bytes: unified={unified}, separate={separate}"
        );
    }

    #[test]
    fn grep_arguments_use_the_unified_contract() {
        let tool = SearchTool::new(true);
        let args = tool
            .adapted_args(
                SearchMode::Grep,
                &serde_json::json!({
                    "mode": "grep",
                    "query": "TODO",
                    "include": "*.rs",
                    "case_sensitive": false,
                }),
            )
            .unwrap();
        assert_eq!(args["pattern"], "TODO");
        assert_eq!(args["glob"], "*.rs");
        assert_eq!(args["-i"], true);
    }

    #[test]
    fn grep_keeps_significant_query_whitespace() {
        let args = SearchTool::new(true)
            .adapted_args(
                SearchMode::Grep,
                &serde_json::json!({"mode": "grep", "query": "^  indented$"}),
            )
            .unwrap();
        assert_eq!(args["pattern"], "^  indented$");
    }

    #[test]
    fn bm25_mode_requires_workspace_reads() {
        let error = SearchTool::new(false)
            .adapted_args(
                SearchMode::Bm25,
                &serde_json::json!({"mode": "bm25", "query": "workspace"}),
            )
            .unwrap_err();
        assert!(error.contains("file reads"));
    }

    #[test]
    fn indexed_mode_is_explicit_and_routes_to_persistent_backend() {
        let tool = SearchTool::new(true).with_indexed(true);
        assert_eq!(
            tool.parameters()["properties"]["mode"]["enum"],
            serde_json::json!(["grep", "glob", "bm25", "indexed"])
        );
        let adapted = tool
            .adapted_args(
                SearchMode::Indexed,
                &serde_json::json!({
                    "mode": "indexed",
                    "query": "workspace policy",
                    "include": "*.rs",
                    "limit": 5
                }),
            )
            .unwrap();
        assert_eq!(adapted["query"], "workspace policy");
        assert_eq!(adapted["glob"], "*.rs");
        assert_eq!(adapted["_persistent_index"], true);
    }

    #[test]
    fn semantic_mode_requires_a_session_runtime() {
        let error = SearchTool::new(true)
            .adapted_args(
                SearchMode::Semantic,
                &serde_json::json!({"mode": "semantic", "query": "workspace"}),
            )
            .unwrap_err();
        assert!(error.contains("did not enable semantic retrieval"));

        let error = SearchTool::new(true)
            .adapted_args(
                SearchMode::Hybrid,
                &serde_json::json!({"mode": "hybrid", "query": "workspace"}),
            )
            .unwrap_err();
        assert!(error.contains("did not enable semantic retrieval"));
    }

    #[test]
    fn grep_count_mode_requires_workspace_reads() {
        let error = SearchTool::new(false)
            .adapted_args(
                SearchMode::Grep,
                &serde_json::json!({
                    "mode": "grep",
                    "query": "TODO",
                    "output_mode": "count"
                }),
            )
            .unwrap_err();
        assert!(error.contains("file reads"));
    }

    #[tokio::test]
    async fn one_tool_executes_all_search_modes() {
        let workspace = tempfile::tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("src")).unwrap();
        fs::write(
            workspace.path().join("src/main.rs"),
            "fn main() { println!(\"Hello workspace\"); }\n",
        )
        .unwrap();
        fs::write(workspace.path().join("README.md"), "hello docs\n").unwrap();
        let ctx = ToolContext::new(workspace.path().to_path_buf());
        let tool = SearchTool::new(true);

        let grep = tool
            .execute(
                &serde_json::json!({
                    "mode": "grep",
                    "query": "hello",
                    "include": "*.rs",
                    "case_sensitive": false
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(grep.success, "{}", grep.content);
        assert!(grep.content.contains("src/main.rs"));
        assert_eq!(grep.metadata.unwrap()["mode"], "grep");

        let glob = tool
            .execute(
                &serde_json::json!({"mode": "glob", "query": "**/*.rs"}),
                &ctx,
            )
            .await
            .unwrap();
        assert!(glob.success, "{}", glob.content);
        assert!(glob.content.contains("src/main.rs"));
        assert_eq!(glob.metadata.unwrap()["mode"], "glob");

        let bm25 = tool
            .execute(
                &serde_json::json!({
                    "mode": "bm25",
                    "query": "hello workspace",
                    "include": "*.rs"
                }),
                &ctx,
            )
            .await
            .unwrap();
        assert!(bm25.success, "{}", bm25.content);
        assert!(bm25.content.contains("src/main.rs"));
        assert_eq!(bm25.metadata.unwrap()["mode"], "bm25");
    }
}
