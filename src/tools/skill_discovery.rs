//! Skill Discovery - Native search and install for the agent skills ecosystem
//!
//! Provides two tools:
//! - `search_skills`: Search for skills via GitHub Repository Search API
//! - `install_skill`: Download and install skills from GitHub repositories
//!
//! These replace the previous `npx skills` dependency with a zero-dependency
//! native implementation using the public GitHub API.

use crate::tools::types::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;

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
                     The skill will be loaded automatically on next session start.\n\
                     To use it now, load it via the LoadSkill API.",
                    filename, owner, repo, path.display(), location,
                )))
            }
            Err(e) => Ok(ToolOutput::error(format!("Failed to save skill: {}", e))),
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
}
