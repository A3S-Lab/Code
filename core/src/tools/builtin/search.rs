//! Unified workspace search tool.

use super::{
    bm25::Bm25Tool, glob_tool::GlobTool, grep::GrepTool, hybrid_search::HybridSearchTool,
    semantic_search::SemanticSearchTool,
};
use crate::tools::types::{Tool, ToolCapabilities, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

const MAX_PAGE_LIMIT: usize = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SearchMode {
    Grep,
    Glob,
    Bm25,
    Semantic,
    Hybrid,
}

impl SearchMode {
    fn parse(args: &serde_json::Value) -> std::result::Result<Self, String> {
        match args.get("mode").and_then(serde_json::Value::as_str) {
            Some("grep") => Ok(Self::Grep),
            Some("glob") => Ok(Self::Glob),
            Some("bm25") => Ok(Self::Bm25),
            Some("semantic") => Ok(Self::Semantic),
            Some("hybrid") => Ok(Self::Hybrid),
            Some(_) => {
                Err("mode must be 'grep', 'glob', 'bm25', 'semantic', or 'hybrid'".to_string())
            }
            None => Err("mode parameter is required".to_string()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Grep => "grep",
            Self::Glob => "glob",
            Self::Bm25 => "bm25",
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
    persistent_backend_enabled: bool,
    /// Live index phase. `Degraded` removes semantic modes from later definitions.
    retrieval: Option<Arc<crate::workspace::WorkspaceRetrievalRuntime>>,
    /// Model-facing text at registration. [`Tool::definition`] rebuilds it from
    /// the live phase so a later `Degraded` index is not still advertised.
    description: String,
    /// Passed into the fuzzy widen the grep path already runs.
    fuzzy_budget: Duration,
}

impl SearchTool {
    pub fn new(read_enabled: bool) -> Self {
        Self {
            backend_search_enabled: true,
            read_enabled,
            semantic_enabled: false,
            persistent_backend_enabled: false,
            retrieval: None,
            description: String::new(),
            fuzzy_budget: super::search_fallback::FUZZY_TIME_BUDGET,
        }
        .sync_description()
    }

    /// Test seam for an already-spent widen budget. Production keeps [`Self::new`].
    #[cfg(test)]
    pub fn with_fuzzy_budget(mut self, budget: Duration) -> Self {
        self.fuzzy_budget = budget;
        self
    }

    pub fn with_backend_search(mut self, enabled: bool) -> Self {
        self.backend_search_enabled = enabled;
        self.sync_description()
    }

    pub fn with_semantic(mut self, enabled: bool) -> Self {
        self.semantic_enabled = enabled;
        self.sync_description()
    }

    /// Observe the session index so a failed build stops advertising semantic modes.
    pub fn with_retrieval_runtime(
        mut self,
        runtime: Arc<crate::workspace::WorkspaceRetrievalRuntime>,
    ) -> Self {
        self.retrieval = Some(runtime);
        self
    }

    fn sync_description(mut self) -> Self {
        self.description = describe_modes(&self.modes());
        self
    }

    /// Prefer the workspace's durable lexical projection without exposing an
    /// implementation-specific mode in the model-facing schema.
    pub fn with_persistent_backend(mut self, enabled: bool) -> Self {
        self.persistent_backend_enabled = enabled;
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
        if self.semantic_advertised() {
            modes.extend(["semantic", "hybrid"]);
        }
        modes
    }

    fn semantic_advertised(&self) -> bool {
        if !self.semantic_enabled {
            return false;
        }
        match &self.retrieval {
            Some(runtime) => {
                runtime.status().phase != crate::workspace::WorkspaceRetrievalPhase::Degraded
            }
            None => true,
        }
    }

    fn mode_schema_description(&self) -> String {
        let listed = describe_modes(&self.modes());
        if self.modes().contains(&"bm25") {
            format!("{listed} A bm25 request may use the durable local index automatically.")
        } else {
            listed
        }
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
                SearchMode::Bm25 | SearchMode::Semantic | SearchMode::Hybrid => "query",
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
                if self.persistent_backend_enabled {
                    // Preserve the existing public `bm25` mode while routing
                    // local manifest workspaces to their durable a3s-vec
                    // projection automatically. Callers do not need to know
                    // whether the native cache is ready; Bm25Tool falls back
                    // to the catalog path until it is available.
                    adapted.insert(
                        "_persistent_index".to_owned(),
                        serde_json::Value::Bool(true),
                    );
                }
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
        &self.description
    }

    fn definition(&self) -> crate::llm::ToolDefinition {
        // The cached description is fixed at registration. The index can fail
        // afterwards, and the next model-facing definition must follow that phase.
        crate::llm::ToolDefinition {
            name: self.name().to_string(),
            description: describe_modes(&self.modes()),
            parameters: self.parameters(),
        }
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
        if modes.contains(&"semantic") {
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
        let mode_description = self.mode_schema_description();
        let query_description = query_schema_description(&modes);
        let limit_description = limit_schema_description(&modes);
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
                    "description": mode_description
                },
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "description": query_description
                },
                "path": {
                    "type": "string",
                    "description": "Optional. Workspace-relative directory or file to search, or an a3s://tool-output/... artifact URI (grep mode only). Default: workspace root."
                },
                "include": {
                    "type": "string",
                    "description": "Optional. Glob pattern used to filter candidate files, for example '*.rs' or '*.{ts,tsx}'."
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
                    "description": limit_description
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
            Ok(SearchMode::Bm25 | SearchMode::Semantic | SearchMode::Hybrid) => {
                ToolCapabilities::parallel_safe_read(2)
            }
            Ok(SearchMode::Grep) | Err(_) => ToolCapabilities::parallel_safe_read(16),
        }
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let mode = match SearchMode::parse(args) {
            Ok(mode) => mode,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        if let Some(uri) = args
            .get("path")
            .and_then(serde_json::Value::as_str)
            .filter(|path| crate::tools::artifact_view::is_tool_output_uri(path))
        {
            return Ok(with_search_mode(artifact_grep(mode, uri, args, ctx), mode));
        }
        let adapted = match self.adapted_args(mode, args) {
            Ok(adapted) => adapted,
            Err(error) => return Ok(ToolOutput::error(error)),
        };
        let output = match mode {
            SearchMode::Grep => {
                let exact = GrepTool.execute(&adapted, ctx).await?;
                widen_literal_grep(args, &adapted, exact, ctx, self.fuzzy_budget).await
            }
            SearchMode::Glob => GlobTool.execute(&adapted, ctx).await?,
            SearchMode::Bm25 => Bm25Tool.execute(&adapted, ctx).await?,
            SearchMode::Semantic => SemanticSearchTool.execute(&adapted, ctx).await?,
            SearchMode::Hybrid => HybridSearchTool.execute(&adapted, ctx).await?,
        };
        Ok(with_search_mode(output, mode))
    }
}

async fn widen_literal_grep(
    args: &serde_json::Value,
    adapted: &serde_json::Value,
    exact: ToolOutput,
    ctx: &ToolContext,
    budget: Duration,
) -> ToolOutput {
    let Some(query) = args.get("query").and_then(serde_json::Value::as_str) else {
        return exact;
    };
    if !exact.success
        || !super::search_fallback::is_literal_query(query)
        || !super::search_fallback::grep_is_empty(&exact.content, matching_lines(&exact))
    {
        return exact;
    }
    let case_sensitive = args
        .get("case_sensitive")
        .and_then(serde_json::Value::as_bool);
    let already_insensitive = adapted.get("-i").and_then(serde_json::Value::as_bool) == Some(true);
    let mut current = exact;
    if !query.chars().any(|ch| ch.is_uppercase())
        && case_sensitive != Some(true)
        && !already_insensitive
    {
        let mut folded = match adapted.as_object().cloned() {
            Some(map) => map,
            None => return current,
        };
        folded.insert("-i".to_owned(), serde_json::Value::Bool(true));
        let second = match GrepTool
            .execute(&serde_json::Value::Object(folded), ctx)
            .await
        {
            Ok(output) => output,
            Err(_) => return current,
        };
        if second.success
            && !super::search_fallback::grep_is_empty(&second.content, matching_lines(&second))
        {
            let mut second = second;
            stamp_match(&mut second, "case_insensitive");
            return second;
        }
        if second.success {
            current = second;
        }
    }
    let root = ctx.workspace.clone();
    let path = adapted
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let glob = adapted
        .get("glob")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let needle = query.to_owned();
    let insensitive = case_sensitive != Some(true);
    let scanned = tokio::task::spawn_blocking(move || {
        let base = super::search_fallback::search_base(&root, path.as_deref())?;
        super::search_fallback::fuzzy_scan(
            &root,
            &base,
            glob.as_deref(),
            &needle,
            insensitive,
            budget,
        )
    })
    .await;
    match scanned {
        Ok(Some(content)) => {
            let mut output = ToolOutput::success(content);
            // A budget stop with no hit line is an unfinished scan, not a fuzzy match.
            if output.content.lines().any(|line| line.starts_with('>')) {
                stamp_match(&mut output, "fuzzy");
            }
            output
        }
        _ => current,
    }
}

fn matching_lines(output: &ToolOutput) -> Option<u64> {
    output
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.pointer("/search/matching_lines"))
        .and_then(serde_json::Value::as_u64)
}

fn stamp_match(output: &mut ToolOutput, kind: &str) {
    let metadata = output.metadata.get_or_insert_with(|| serde_json::json!({}));
    if let Some(object) = metadata.as_object_mut() {
        object.insert("match".to_owned(), serde_json::json!(kind));
    }
}

fn artifact_grep(
    mode: SearchMode,
    uri: &str,
    args: &serde_json::Value,
    ctx: &ToolContext,
) -> ToolOutput {
    if mode != SearchMode::Grep {
        return ToolOutput::error(format!(
            "a tool-output artifact path supports mode='grep' only, not mode='{}'",
            mode.as_str()
        ));
    }
    let Some(query) = args
        .get("query")
        .and_then(serde_json::Value::as_str)
        .filter(|query| !query.trim().is_empty())
    else {
        return ToolOutput::error("query parameter is required");
    };
    let count = |name: &str, default: usize| {
        args.get(name)
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(default)
    };
    let case_insensitive = args
        .get("case_sensitive")
        .and_then(serde_json::Value::as_bool)
        == Some(false);
    crate::tools::artifact_view::grep(
        ctx,
        uri,
        query,
        case_insensitive,
        count("context", 0),
        count("limit", crate::tools::artifact_view::DEFAULT_GREP_MATCHES),
    )
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

fn describe_modes(modes: &[&str]) -> String {
    if modes.is_empty() {
        return "Workspace search has no available mode in this session.".to_string();
    }
    let hints = modes
        .iter()
        .map(|mode| match *mode {
            "grep" => "grep for an exact string or regex",
            "glob" => "glob for file names",
            "bm25" => "bm25 for ranked lexical matches",
            "semantic" => "semantic for concept similarity",
            "hybrid" => "hybrid when exact and conceptual evidence both matter",
            other => other,
        })
        .collect::<Vec<_>>();
    format!(
        "Search the workspace with one model-selected mode. Use {}.",
        hints.join(", ")
    )
}

fn query_schema_description(modes: &[&str]) -> String {
    if modes.is_empty() {
        return "Required. No search mode is available in this session.".to_string();
    }
    format!(
        "Required. Query text for the selected mode. Available modes: {}.",
        modes.join(", ")
    )
}

fn limit_schema_description(modes: &[&str]) -> String {
    let ranked = modes
        .iter()
        .copied()
        .filter(|mode| matches!(*mode, "bm25" | "semantic" | "hybrid"))
        .collect::<Vec<_>>();
    if ranked.is_empty() {
        return "Optional. Page size for glob or paginated grep (default 200, maximum 1000)."
            .to_string();
    }
    format!(
        "Optional. Page size for glob or paginated grep (default 200, maximum 1000), or result count for {} (default 10, maximum 25).",
        ranked.join("/")
    )
}

fn with_search_mode(mut output: ToolOutput, mode: SearchMode) -> ToolOutput {
    match output.metadata.as_mut() {
        Some(serde_json::Value::Object(metadata)) => {
            if let Some(execution_mode) = metadata.get("mode").cloned() {
                metadata
                    .entry("execution_mode".to_owned())
                    .or_insert(execution_mode);
            }
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
    fn search_tool_description_lists_only_enabled_modes() {
        let lexical = SearchTool::new(true);
        assert!(lexical.description().contains("bm25"));
        assert!(!lexical.description().contains("semantic"));
        assert!(!lexical.description().contains("hybrid"));
        let schema = lexical.parameters().to_string();
        assert!(schema.contains("bm25"));
        assert!(!schema.contains("semantic"));
        assert!(!schema.contains("hybrid"));

        let semantic = SearchTool::new(true).with_semantic(true);
        assert!(semantic.description().contains("semantic"));
        assert!(semantic.description().contains("hybrid"));
        let schema = semantic.parameters();
        let modes = schema["properties"]["mode"]["enum"].as_array().unwrap();
        assert!(modes.iter().any(|mode| mode == "semantic"));
        assert!(modes.iter().any(|mode| mode == "hybrid"));
    }

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
    fn bm25_mode_transparently_prefers_persistent_backend_when_available() {
        let adapted = SearchTool::new(true)
            .with_persistent_backend(true)
            .adapted_args(
                SearchMode::Bm25,
                &serde_json::json!({"mode": "bm25", "query": "workspace policy"}),
            )
            .unwrap();
        assert_eq!(adapted["_persistent_index"], true);

        let fallback = SearchTool::new(true)
            .adapted_args(
                SearchMode::Bm25,
                &serde_json::json!({"mode": "bm25", "query": "workspace policy"}),
            )
            .unwrap();
        assert!(fallback.get("_persistent_index").is_none());
    }

    #[test]
    fn unified_search_preserves_the_internal_execution_path() {
        let output = with_search_mode(
            ToolOutput::success("fallback")
                .with_metadata(serde_json::json!({"mode": "incremental_catalog"})),
            SearchMode::Bm25,
        );
        let metadata = output.metadata.unwrap();
        assert_eq!(metadata["mode"], "bm25");
        assert_eq!(metadata["execution_mode"], "incremental_catalog");
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
