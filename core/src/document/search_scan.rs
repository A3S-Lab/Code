use anyhow::Result;
use ignore::WalkBuilder;
use regex::Regex;
use std::path::{Path, PathBuf};

pub(crate) fn compile_search_patterns(keywords: &[String]) -> Vec<Regex> {
    keywords
        .iter()
        .filter_map(|kw| Regex::new(&format!("(?i){}", regex::escape(kw))).ok())
        .collect()
}

pub(crate) fn find_matching_paths(
    workspace: &Path,
    keywords: &[String],
    max_results: usize,
    include_glob: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let patterns = compile_search_patterns(keywords);
    let builder = build_walk_builder(workspace, include_glob, false);
    let mut results: Vec<PathBuf> = Vec::new();

    for entry in builder.build().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if patterns.iter().any(|p| p.is_match(name)) {
            results.push(path.to_path_buf());
            if results.len() >= max_results {
                break;
            }
        }
    }

    Ok(results)
}

pub(crate) fn build_walk_builder(
    workspace: &Path,
    include_glob: Option<&str>,
    include_git_global: bool,
) -> WalkBuilder {
    let mut builder = WalkBuilder::new(workspace);
    builder.hidden(false).git_ignore(true);
    if include_git_global {
        builder.git_global(true);
    }

    if let Some(glob) = include_glob {
        let mut types = ignore::types::TypesBuilder::new();
        if types.add("custom", glob).is_ok() {
            types.select("custom");
            if let Ok(built) = types.build() {
                builder.types(built);
            }
        }
    }

    builder
}
