//! Unified workspace search tool.

use super::{bm25::Bm25Tool, glob_tool::GlobTool, grep::GrepTool};
use crate::tools::types::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;

const MAX_PAGE_LIMIT: usize = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SearchMode {
    Grep,
    Glob,
    Bm25,
}

impl SearchMode {
    fn parse(args: &serde_json::Value) -> std::result::Result<Self, String> {
        match args.get("mode").and_then(serde_json::Value::as_str) {
            Some("grep") => Ok(Self::Grep),
            Some("glob") => Ok(Self::Glob),
            Some("bm25") => Ok(Self::Bm25),
            Some(_) => Err("mode must be 'grep', 'glob', or 'bm25'".to_string()),
            None => Err("mode parameter is required".to_string()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Grep => "grep",
            Self::Glob => "glob",
            Self::Bm25 => "bm25",
        }
    }
}

/// Model-facing workspace search abstraction.
///
/// Grep, glob, and BM25 remain separate internal implementations because they
/// have different backend and rendering behavior. The model sees one stable
/// tool contract and selects the behavior with `mode`.
pub struct SearchTool {
    read_enabled: bool,
}

impl SearchTool {
    pub fn new(read_enabled: bool) -> Self {
        Self { read_enabled }
    }

    fn modes(&self) -> Vec<&'static str> {
        let mut modes = vec!["grep", "glob"];
        if self.read_enabled {
            modes.push("bm25");
        }
        modes
    }

    fn adapted_args(
        &self,
        mode: SearchMode,
        args: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, String> {
        if mode == SearchMode::Bm25 && !self.read_enabled {
            return Err(
                "mode='bm25' is unavailable because this workspace backend did not provide file reads"
                    .to_string(),
            );
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
                SearchMode::Bm25 => "query",
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
        "Search the workspace with regex content search, file globbing, or native BM25 lexical ranking. Select the behavior with mode."
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
        if self.read_enabled {
            examples.push(serde_json::json!({
                "mode": "bm25",
                "query": "workspace permission policy",
                "path": "core/src",
                "limit": 8
            }));
        }
        let grep_output_modes = if self.read_enabled {
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
                    "description": "Required. grep searches file contents with a regular expression; glob finds paths; bm25 ranks text chunks by lexical relevance."
                },
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "description": "Required. A regular expression in grep mode, a glob pattern in glob mode, or a plain-text relevance query in bm25 mode."
                },
                "path": {
                    "type": "string",
                    "description": "Optional. Workspace-relative directory or file to search. Default: workspace root."
                },
                "include": {
                    "type": "string",
                    "description": "Optional for grep/bm25. Glob pattern used to filter candidate files, for example '*.rs' or '*.{ts,tsx}'."
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
                    "description": "Optional. Page size for glob or paginated grep (default 200, maximum 1000), or result count for bm25 (default 10, maximum 25)."
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
            Ok(SearchMode::Bm25) => ToolCapabilities::parallel_safe_read(2),
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
