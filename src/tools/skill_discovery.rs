//! Skill Discovery - Native search and install for the agent skills ecosystem
//!
//! Provides two tools:
//! - `search_skills`: Search for skills via GitHub Repository Search API
//! - `install_skill`: Download and install skills from GitHub repositories
//!
//! These replace the previous `npx skills` dependency with a zero-dependency
//! native implementation using the public GitHub API.

use crate::tools::skill::{builtin_skills, Skill};
use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use std::path::Path;

const GITHUB_API_BASE: &str = "https://api.github.com";
const USER_AGENT: &str = "a3s-code";

// ============================================================================
// GitHub API Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
struct GitHubSearchReposResponse {
    total_count: u64,
    items: Vec<GitHubRepo>,
}

#[derive(Debug, Deserialize)]
struct GitHubRepo {
    full_name: String,
    description: Option<String>,
    html_url: String,
    stargazers_count: u64,
    #[serde(default)]
    topics: Vec<String>,
}

// ============================================================================
// Shared HTTP Client
// ============================================================================

fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent(USER_AGENT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

// ============================================================================
// SearchSkillsTool
// ============================================================================

/// Tool for searching skills in the open agent skills ecosystem
pub struct SearchSkillsTool {
    client: reqwest::Client,
}

impl SearchSkillsTool {
    pub fn new() -> Self {
        Self {
            client: build_client(),
        }
    }

    /// Search GitHub for skill repositories
    async fn search_github(&self, query: &str, limit: usize) -> Result<Vec<GitHubRepo>> {
        // Search for repos tagged with claude-code-skill topic
        let search_query = format!("{} topic:claude-code-skill", query);

        let response = self
            .client
            .get(format!("{}/search/repositories", GITHUB_API_BASE))
            .header("Accept", "application/vnd.github.v3+json")
            .query(&[
                ("q", search_query.as_str()),
                ("sort", "stars"),
                ("order", "desc"),
                ("per_page", &limit.to_string()),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("GitHub API returned {}: {}", status, body);
        }

        let search_result: GitHubSearchReposResponse = response.json().await?;
        Ok(search_result.items)
    }

    /// Format search results as readable text
    fn format_results(repos: &[GitHubRepo], query: &str) -> String {
        if repos.is_empty() {
            return format!(
                "No skills found for query: \"{}\"\n\n\
                Tips:\n\
                - Try broader keywords (e.g., \"react\" instead of \"react performance\")\n\
                - Browse available skills at: https://skills.sh/\n\
                - Search GitHub: https://github.com/topics/claude-code-skill",
                query
            );
        }

        let mut output = format!("Found {} skill(s) for \"{}\":\n\n", repos.len(), query);

        for (i, repo) in repos.iter().enumerate() {
            let desc = repo.description.as_deref().unwrap_or("No description");
            let topics_str = if repo.topics.is_empty() {
                String::new()
            } else {
                format!("   Topics: {}\n", repo.topics.join(", "))
            };

            output.push_str(&format!(
                "{}. {} (stars: {})\n\
                   {}\n\
                {}\
                   Install: install_skill(source: \"{}\")\n\
                   URL: {}\n\n",
                i + 1,
                repo.full_name,
                repo.stargazers_count,
                desc,
                topics_str,
                repo.full_name,
                repo.html_url,
            ));
        }

        output.push_str(
            "To install a skill, use the install_skill tool with the source parameter.\n\
             Browse more at: https://skills.sh/\n",
        );

        output
    }
}

#[async_trait]
impl Tool for SearchSkillsTool {
    fn name(&self) -> &str {
        "search_skills"
    }

    fn description(&self) -> &str {
        "Search for agent skills in the open skills ecosystem (skills.sh / GitHub). \
         Returns matching skills with descriptions and install commands."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query keywords (e.g., 'react', 'testing', 'deployment')"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results to return (default: 10, max: 30)"
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, _ctx: &ToolContext) -> Result<ToolOutput> {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");

        if query.trim().is_empty() {
            return Ok(ToolOutput::error(
                "query parameter is required and must not be empty",
            ));
        }

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10)
            .min(30) as usize;

        match self.search_github(query, limit).await {
            Ok(repos) => {
                let output = Self::format_results(&repos, query);
                Ok(ToolOutput::success(output))
            }
            Err(e) => Ok(ToolOutput::error(format!(
                "Failed to search skills: {}\n\n\
                 You can browse skills manually at: https://skills.sh/\n\
                 Or search GitHub: https://github.com/topics/claude-code-skill",
                e
            ))),
        }
    }
}

// ============================================================================
// InstallSkillTool
// ============================================================================

/// Tool for installing skills from GitHub repositories
pub struct InstallSkillTool {
    client: reqwest::Client,
}

impl InstallSkillTool {
    pub fn new() -> Self {
        Self {
            client: build_client(),
        }
    }

    /// Parse source string into (owner, repo, optional skill_name)
    ///
    /// Formats:
    /// - `owner/repo` -> (owner, repo, None)
    /// - `owner/repo@skill-name` -> (owner, repo, Some(skill-name))
    fn parse_source(source: &str) -> Result<(String, String, Option<String>)> {
        let source = source.trim();

        if source.is_empty() {
            anyhow::bail!("Source cannot be empty");
        }

        // Handle "owner/repo@skill-name" format
        if let Some((repo_part, skill_name)) = source.split_once('@') {
            let (owner, repo) = repo_part.split_once('/').ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid source format: \"{}\". Expected: owner/repo or owner/repo@skill-name",
                    source
                )
            })?;
            Ok((
                owner.to_string(),
                repo.to_string(),
                Some(skill_name.to_string()),
            ))
        } else {
            let (owner, repo) = source.split_once('/').ok_or_else(|| {
                anyhow::anyhow!(
                    "Invalid source format: \"{}\". Expected: owner/repo or owner/repo@skill-name",
                    source
                )
            })?;
            Ok((owner.to_string(), repo.to_string(), None))
        }
    }

    /// Try to fetch SKILL.md from various paths in the repository
    ///
    /// Returns (content, suggested_filename) on success.
    async fn fetch_skill_content(
        &self,
        owner: &str,
        repo: &str,
        skill_name: Option<&str>,
    ) -> Result<(String, String)> {
        let mut paths = Vec::new();

        if let Some(name) = skill_name {
            paths.push(format!("skills/{}/SKILL.md", name));
            paths.push(format!("{}/SKILL.md", name));
        }

        // Always try root SKILL.md
        paths.push("SKILL.md".to_string());

        for path in &paths {
            let url = format!(
                "https://raw.githubusercontent.com/{}/{}/main/{}",
                owner, repo, path
            );

            tracing::debug!("Trying to fetch skill from: {}", url);

            match self.client.get(&url).send().await {
                Ok(response) if response.status().is_success() => {
                    let content = response.text().await?;
                    // Validate it looks like a skill (has frontmatter)
                    if content.contains("---") {
                        let skill_filename = if let Some(name) = skill_name {
                            format!("{}.md", name)
                        } else {
                            format!("{}.md", repo)
                        };
                        return Ok((content, skill_filename));
                    }
                }
                Ok(_) => continue,
                Err(_) => continue,
            }
        }

        anyhow::bail!(
            "Could not find SKILL.md in {}/{}. Tried paths: {}",
            owner,
            repo,
            paths.join(", ")
        )
    }

    /// Save skill content to disk
    fn save_skill(
        content: &str,
        filename: &str,
        install_dir: &std::path::Path,
    ) -> Result<std::path::PathBuf> {
        std::fs::create_dir_all(install_dir).map_err(|e| {
            anyhow::anyhow!(
                "Failed to create skills directory {}: {}",
                install_dir.display(),
                e
            )
        })?;

        let path = install_dir.join(filename);
        std::fs::write(&path, content).map_err(|e| {
            anyhow::anyhow!("Failed to write skill file {}: {}", path.display(), e)
        })?;

        Ok(path)
    }
}

#[async_trait]
impl Tool for InstallSkillTool {
    fn name(&self) -> &str {
        "install_skill"
    }

    fn description(&self) -> &str {
        "Install an agent skill from a GitHub repository. Downloads the SKILL.md definition \
         and saves it to the local or global skills directory."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "source": {
                    "type": "string",
                    "description": "Skill source in format: owner/repo or owner/repo@skill-name (e.g., 'vercel-labs/agent-skills@vercel-react-best-practices')"
                },
                "global": {
                    "type": "boolean",
                    "description": "Install globally (~/.a3s/skills/) instead of project-locally (.a3s/skills/). Default: false"
                }
            },
            "required": ["source"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let source = args.get("source").and_then(|v| v.as_str()).unwrap_or("");

        if source.trim().is_empty() {
            return Ok(ToolOutput::error("source parameter is required"));
        }

        let global = args
            .get("global")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Parse source
        let (owner, repo, skill_name) = match Self::parse_source(source) {
            Ok(parsed) => parsed,
            Err(e) => return Ok(ToolOutput::error(format!("{}", e))),
        };

        // Fetch skill content from GitHub
        let (content, filename) =
            match self
                .fetch_skill_content(&owner, &repo, skill_name.as_deref())
                .await
            {
                Ok(result) => result,
                Err(e) => return Ok(ToolOutput::error(format!("Failed to fetch skill: {}", e))),
            };

        // Validate it parses as a valid skill
        if crate::tools::skill::Skill::parse(&content).is_none() {
            return Ok(ToolOutput::error(
                "Downloaded content is not a valid skill (missing or invalid frontmatter)",
            ));
        }

        // Determine install directory
        let install_dir = if global {
            dirs::home_dir()
                .map(|h| h.join(".a3s").join("skills"))
                .unwrap_or_else(|| ctx.workspace.join(".a3s").join("skills"))
        } else {
            ctx.workspace.join(".a3s").join("skills")
        };

        // Save skill to disk
        match Self::save_skill(&content, &filename, &install_dir) {
            Ok(path) => {
                let location = if global { "globally" } else { "locally" };
                Ok(ToolOutput::success(format!(
                    "Installed skill \"{}\" from {}/{}\n\
                     Saved to: {}\n\
                     Location: {}\n\n\
                     The skill is now active in this session.",
                    filename, owner, repo, path.display(), location,
                ))
                .with_metadata(serde_json::json!({
                    "_load_skill": true,
                    "skill_name": filename,
                    "skill_content": content,
                })))
            }
            Err(e) => Ok(ToolOutput::error(format!("Failed to save skill: {}", e))),
        }
    }
}

// ============================================================================
// LoadSkillTool
// ============================================================================

/// Tool for loading a skill's full instructions on-demand by name.
///
/// Search order:
/// 1. `{workspace}/.a3s/skills/{name}.md`
/// 2. `~/.a3s/skills/{name}.md`
/// 3. Fallback: scan `.md` files in those dirs matching by parsed `skill.name`
/// 4. Built-in skills via `builtin_skills()`
pub struct LoadSkillTool;

impl LoadSkillTool {
    pub fn new() -> Self {
        Self
    }

    /// Try to load a skill from a directory by filename or by scanning for matching name.
    fn try_load_from_dir(dir: &Path, filename: &str, name: &str) -> Option<(Skill, String)> {
        // Try direct filename match first
        let direct_path = dir.join(filename);
        if direct_path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&direct_path) {
                if let Some(skill) = Skill::parse(&content) {
                    return Some((skill, content));
                }
            }
        }

        // Fallback: scan directory for a skill whose parsed name matches
        let Ok(entries) = std::fs::read_dir(dir) else {
            return None;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Some(skill) = Skill::parse(&content) {
                    if skill.name == name {
                        return Some((skill, content));
                    }
                }
            }
        }

        None
    }

    /// Find a skill by name, searching workspace, home, and built-in locations.
    fn find_skill(name: &str, workspace: &Path) -> Option<(Skill, String)> {
        // Normalize: strip .md extension if provided
        let name = name.strip_suffix(".md").unwrap_or(name);
        let filename = format!("{}.md", name);

        // 1. Workspace skills directory
        let workspace_dir = workspace.join(".a3s").join("skills");
        if let Some(result) = Self::try_load_from_dir(&workspace_dir, &filename, name) {
            return Some(result);
        }

        // 2. Home directory skills
        if let Some(home) = dirs::home_dir() {
            let home_dir = home.join(".a3s").join("skills");
            if let Some(result) = Self::try_load_from_dir(&home_dir, &filename, name) {
                return Some(result);
            }
        }

        // 3. Built-in skills
        for skill in builtin_skills() {
            if skill.name == name {
                // Reconstruct the raw content from the built-in include
                let raw = include_str!("../../skills/find-skills.md");
                if skill.name == "find-skills" {
                    return Some((skill, raw.to_string()));
                }
                // For other future built-in skills, synthesize content
                let synthetic = format!(
                    "---\nname: {}\ndescription: {}\n---\n{}",
                    skill.name, skill.description, skill.content
                );
                return Some((skill, synthetic));
            }
        }

        None
    }
}

#[async_trait]
impl Tool for LoadSkillTool {
    fn name(&self) -> &str {
        "load_skill"
    }

    fn description(&self) -> &str {
        "Load a skill's full instructions by name. Use when you need detailed \
         content of a skill from the skill catalog."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name of the skill to load (e.g., 'react-best-practices')"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: &serde_json::Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");

        if name.trim().is_empty() {
            return Ok(ToolOutput::error(
                "name parameter is required and must not be empty",
            ));
        }

        match Self::find_skill(name.trim(), &ctx.workspace) {
            Some((skill, raw_content)) => {
                let summary = format!(
                    "Loaded skill \"{}\".\n\nDescription: {}\nKind: {:?}",
                    skill.name,
                    if skill.description.is_empty() {
                        "No description"
                    } else {
                        &skill.description
                    },
                    skill.kind,
                );

                Ok(ToolOutput::success(summary).with_metadata(serde_json::json!({
                    "_load_skill": true,
                    "skill_name": skill.name,
                    "skill_content": raw_content,
                })))
            }
            None => Ok(ToolOutput::error(format!(
                "Skill \"{}\" not found.\n\n\
                 Searched in:\n\
                 - {}\n\
                 - ~/.a3s/skills/\n\
                 - Built-in skills\n\n\
                 Use search_skills to find and install_skill to install new skills.",
                name,
                ctx.workspace.join(".a3s/skills").display(),
            ))),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ===================
    // parse_source Tests
    // ===================

    #[test]
    fn test_parse_source_owner_repo() {
        let (owner, repo, skill) = InstallSkillTool::parse_source("vercel-labs/agent-skills").unwrap();
        assert_eq!(owner, "vercel-labs");
        assert_eq!(repo, "agent-skills");
        assert!(skill.is_none());
    }

    #[test]
    fn test_parse_source_owner_repo_skill() {
        let (owner, repo, skill) =
            InstallSkillTool::parse_source("vercel-labs/agent-skills@react-best-practices").unwrap();
        assert_eq!(owner, "vercel-labs");
        assert_eq!(repo, "agent-skills");
        assert_eq!(skill, Some("react-best-practices".to_string()));
    }

    #[test]
    fn test_parse_source_empty() {
        let result = InstallSkillTool::parse_source("");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_source_no_slash() {
        let result = InstallSkillTool::parse_source("just-a-name");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_source_at_without_slash() {
        let result = InstallSkillTool::parse_source("no-slash@skill");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_source_whitespace_trimmed() {
        let (owner, repo, skill) = InstallSkillTool::parse_source("  owner/repo  ").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
        assert!(skill.is_none());
    }

    // ===================
    // format_results Tests
    // ===================

    #[test]
    fn test_format_results_empty() {
        let output = SearchSkillsTool::format_results(&[], "react");
        assert!(output.contains("No skills found"));
        assert!(output.contains("react"));
        assert!(output.contains("skills.sh"));
    }

    #[test]
    fn test_format_results_single() {
        let repos = vec![GitHubRepo {
            full_name: "owner/skill-repo".to_string(),
            description: Some("A test skill".to_string()),
            html_url: "https://github.com/owner/skill-repo".to_string(),
            stargazers_count: 42,
            topics: vec!["claude-code-skill".to_string()],
        }];

        let output = SearchSkillsTool::format_results(&repos, "test");
        assert!(output.contains("Found 1 skill(s)"));
        assert!(output.contains("owner/skill-repo"));
        assert!(output.contains("A test skill"));
        assert!(output.contains("42"));
        assert!(output.contains("install_skill"));
    }

    #[test]
    fn test_format_results_multiple() {
        let repos = vec![
            GitHubRepo {
                full_name: "a/b".to_string(),
                description: Some("First".to_string()),
                html_url: "https://github.com/a/b".to_string(),
                stargazers_count: 100,
                topics: vec![],
            },
            GitHubRepo {
                full_name: "c/d".to_string(),
                description: None,
                html_url: "https://github.com/c/d".to_string(),
                stargazers_count: 50,
                topics: vec!["skill".to_string()],
            },
        ];

        let output = SearchSkillsTool::format_results(&repos, "query");
        assert!(output.contains("Found 2 skill(s)"));
        assert!(output.contains("1. a/b"));
        assert!(output.contains("2. c/d"));
        assert!(output.contains("No description"));
    }

    #[test]
    fn test_format_results_with_topics() {
        let repos = vec![GitHubRepo {
            full_name: "owner/repo".to_string(),
            description: Some("desc".to_string()),
            html_url: "https://github.com/owner/repo".to_string(),
            stargazers_count: 10,
            topics: vec!["react".to_string(), "claude-code-skill".to_string()],
        }];

        let output = SearchSkillsTool::format_results(&repos, "react");
        assert!(output.contains("Topics: react, claude-code-skill"));
    }

    // ===================
    // save_skill Tests
    // ===================

    #[test]
    fn test_save_skill_creates_dir_and_file() {
        let temp = tempfile::tempdir().unwrap();
        let install_dir = temp.path().join("skills");

        let content = "---\nname: test\n---\nContent";
        let path = InstallSkillTool::save_skill(content, "test.md", &install_dir).unwrap();

        assert!(path.exists());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        assert_eq!(path.file_name().unwrap(), "test.md");
    }

    #[test]
    fn test_save_skill_overwrites_existing() {
        let temp = tempfile::tempdir().unwrap();
        let install_dir = temp.path().join("skills");
        std::fs::create_dir_all(&install_dir).unwrap();

        let old_content = "old";
        let new_content = "---\nname: updated\n---\nNew";
        let path = install_dir.join("skill.md");
        std::fs::write(&path, old_content).unwrap();

        let saved = InstallSkillTool::save_skill(new_content, "skill.md", &install_dir).unwrap();
        assert_eq!(std::fs::read_to_string(saved).unwrap(), new_content);
    }

    // ===================
    // SearchSkillsTool Trait Tests
    // ===================

    #[test]
    fn test_search_skills_tool_name() {
        let tool = SearchSkillsTool::new();
        assert_eq!(tool.name(), "search_skills");
    }

    #[test]
    fn test_search_skills_tool_description() {
        let tool = SearchSkillsTool::new();
        assert!(!tool.description().is_empty());
        assert!(tool.description().contains("search") || tool.description().contains("Search"));
    }

    #[test]
    fn test_search_skills_tool_parameters() {
        let tool = SearchSkillsTool::new();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["query"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("query")));
    }

    #[tokio::test]
    async fn test_search_skills_empty_query() {
        let tool = SearchSkillsTool::new();
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool
            .execute(&serde_json::json!({"query": ""}), &ctx)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_search_skills_missing_query() {
        let tool = SearchSkillsTool::new();
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool.execute(&serde_json::json!({}), &ctx).await.unwrap();
        assert!(!result.success);
    }

    // ===================
    // InstallSkillTool Trait Tests
    // ===================

    #[test]
    fn test_install_skill_tool_name() {
        let tool = InstallSkillTool::new();
        assert_eq!(tool.name(), "install_skill");
    }

    #[test]
    fn test_install_skill_tool_description() {
        let tool = InstallSkillTool::new();
        assert!(!tool.description().is_empty());
        assert!(tool.description().contains("install") || tool.description().contains("Install"));
    }

    #[test]
    fn test_install_skill_tool_parameters() {
        let tool = InstallSkillTool::new();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["source"].is_object());
        assert!(params["properties"]["global"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("source")));
    }

    #[tokio::test]
    async fn test_install_skill_empty_source() {
        let tool = InstallSkillTool::new();
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool
            .execute(&serde_json::json!({"source": ""}), &ctx)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_install_skill_invalid_source_format() {
        let tool = InstallSkillTool::new();
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool
            .execute(&serde_json::json!({"source": "no-slash"}), &ctx)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.content.contains("Invalid source format"));
    }

    // ===================
    // install_skill metadata Tests
    // ===================

    #[test]
    fn test_install_skill_save_returns_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let install_dir = temp.path().join("skills");

        let skill_content = "---\nname: test-skill\ndescription: A test\n---\n# Test Skill\nDo things.";
        let filename = "test-skill.md";

        // Simulate what execute() does after save_skill succeeds
        let path = InstallSkillTool::save_skill(skill_content, filename, &install_dir).unwrap();

        let output = ToolOutput::success(format!(
            "Installed skill \"{}\" from test/repo\nSaved to: {}\nLocation: locally\n\nThe skill is now active in this session.",
            filename,
            path.display(),
        ))
        .with_metadata(serde_json::json!({
            "_load_skill": true,
            "skill_name": filename,
            "skill_content": skill_content,
        }));

        assert!(output.success);
        let meta = output.metadata.unwrap();
        assert_eq!(meta["_load_skill"], true);
        assert_eq!(meta["skill_name"], "test-skill.md");
        assert!(meta["skill_content"].as_str().unwrap().contains("# Test Skill"));
    }

    // ===================
    // build_client Test
    // ===================

    #[test]
    fn test_build_client_does_not_panic() {
        let _client = build_client();
    }

    // ===================
    // GitHubRepo Deserialization Tests
    // ===================

    #[test]
    fn test_github_repo_deserialize_full() {
        let json = serde_json::json!({
            "full_name": "owner/repo",
            "description": "A skill",
            "html_url": "https://github.com/owner/repo",
            "stargazers_count": 10,
            "topics": ["skill"]
        });
        let repo: GitHubRepo = serde_json::from_value(json).unwrap();
        assert_eq!(repo.full_name, "owner/repo");
        assert_eq!(repo.description, Some("A skill".to_string()));
        assert_eq!(repo.stargazers_count, 10);
        assert_eq!(repo.topics, vec!["skill"]);
    }

    #[test]
    fn test_github_repo_deserialize_minimal() {
        let json = serde_json::json!({
            "full_name": "a/b",
            "html_url": "https://github.com/a/b",
            "stargazers_count": 0
        });
        let repo: GitHubRepo = serde_json::from_value(json).unwrap();
        assert_eq!(repo.full_name, "a/b");
        assert!(repo.description.is_none());
        assert!(repo.topics.is_empty());
    }

    #[test]
    fn test_github_search_response_deserialize() {
        let json = serde_json::json!({
            "total_count": 2,
            "items": [
                {
                    "full_name": "a/b",
                    "html_url": "https://github.com/a/b",
                    "stargazers_count": 5,
                    "topics": []
                },
                {
                    "full_name": "c/d",
                    "description": "skill d",
                    "html_url": "https://github.com/c/d",
                    "stargazers_count": 3,
                    "topics": ["claude-code-skill"]
                }
            ]
        });
        let resp: GitHubSearchReposResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.total_count, 2);
        assert_eq!(resp.items.len(), 2);
    }

    // ===================
    // LoadSkillTool Tests
    // ===================

    #[test]
    fn test_load_skill_tool_name() {
        let tool = LoadSkillTool::new();
        assert_eq!(tool.name(), "load_skill");
    }

    #[test]
    fn test_load_skill_tool_description() {
        let tool = LoadSkillTool::new();
        assert!(!tool.description().is_empty());
        assert!(tool.description().contains("skill"));
    }

    #[test]
    fn test_load_skill_tool_parameters() {
        let tool = LoadSkillTool::new();
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["name"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("name")));
    }

    #[tokio::test]
    async fn test_load_skill_empty_name() {
        let tool = LoadSkillTool::new();
        let ctx = ToolContext::new(PathBuf::from("/tmp"));
        let result = tool
            .execute(&serde_json::json!({"name": ""}), &ctx)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.content.contains("required"));
    }

    #[tokio::test]
    async fn test_load_skill_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let tool = LoadSkillTool::new();
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = tool
            .execute(&serde_json::json!({"name": "nonexistent-skill"}), &ctx)
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.content.contains("not found"));
    }

    #[tokio::test]
    async fn test_load_skill_from_workspace_dir() {
        let temp = tempfile::tempdir().unwrap();
        let skills_dir = temp.path().join(".a3s").join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("my-skill.md"),
            "---\nname: my-skill\ndescription: Test skill\n---\nSkill instructions here.",
        )
        .unwrap();

        let tool = LoadSkillTool::new();
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = tool
            .execute(&serde_json::json!({"name": "my-skill"}), &ctx)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.content.contains("my-skill"));
        let meta = result.metadata.unwrap();
        assert_eq!(meta["_load_skill"], true);
        assert_eq!(meta["skill_name"], "my-skill");
        assert!(meta["skill_content"].as_str().unwrap().contains("Skill instructions here."));
    }

    #[tokio::test]
    async fn test_load_skill_builtin_find_skills() {
        let temp = tempfile::tempdir().unwrap();
        let tool = LoadSkillTool::new();
        let ctx = ToolContext::new(temp.path().to_path_buf());
        let result = tool
            .execute(&serde_json::json!({"name": "find-skills"}), &ctx)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.content.contains("find-skills"));
        let meta = result.metadata.unwrap();
        assert_eq!(meta["_load_skill"], true);
        assert_eq!(meta["skill_name"], "find-skills");
    }

    #[tokio::test]
    async fn test_load_skill_accepts_md_extension() {
        let temp = tempfile::tempdir().unwrap();
        let skills_dir = temp.path().join(".a3s").join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("test.md"),
            "---\nname: test\ndescription: Test\n---\nContent.",
        )
        .unwrap();

        let tool = LoadSkillTool::new();
        let ctx = ToolContext::new(temp.path().to_path_buf());
        // Passing "test.md" should strip .md and find "test"
        let result = tool
            .execute(&serde_json::json!({"name": "test.md"}), &ctx)
            .await
            .unwrap();
        assert!(result.success);
        assert!(result.content.contains("test"));
    }

    #[test]
    fn test_load_skill_find_by_scan() {
        // Skill file has a different filename than its frontmatter name
        let temp = tempfile::tempdir().unwrap();
        let skills_dir = temp.path().join(".a3s").join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("different-filename.md"),
            "---\nname: actual-name\ndescription: Scanned\n---\nFound by scan.",
        )
        .unwrap();

        let result = LoadSkillTool::find_skill("actual-name", temp.path());
        assert!(result.is_some());
        let (skill, raw) = result.unwrap();
        assert_eq!(skill.name, "actual-name");
        assert!(raw.contains("Found by scan."));
    }
}
