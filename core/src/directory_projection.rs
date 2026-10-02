//! Project a filesystem agent directory onto the single Meta Harness.
//!
//! `instructions.md` fills the stock `system` part. `skills/` is the existing
//! `skill_dirs` scan. `tools/` becomes MCP server configs or arguments for the
//! existing `program` tool. `schedules/` are prompts the host submits with
//! `session.send` on a stable session key. This module does not start a
//! process, own a clock, or add a second actor.

use crate::agent_api::SessionOptions;
use crate::mcp::{McpServerConfig, McpTransportConfig};
use crate::meta_harness::HarnessComposeOptions;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

const MAX_INSTRUCTIONS_BYTES: usize = 32 * 1024;
const MAX_SCHEDULE_PROMPT_BYTES: usize = 16 * 1024;
const MAX_MARKDOWN_BYTES: usize = 16 * 1024;
const MAX_TOOLS: usize = 64;
const MAX_SCHEDULES: usize = 64;
const STOCK_COMPONENTS: [&str; 5] = ["system", "tools", "budget", "compact", "infer"];

/// One directory, projected onto the stock Meta Harness compose list.
#[derive(Debug, Clone)]
pub struct DirectoryProjection {
    instructions: String,
    model: Option<String>,
    tool_budget: Option<u32>,
    skill_dir: Option<PathBuf>,
    mcp_servers: Vec<McpServerConfig>,
    scripts: Vec<DirectoryScript>,
    schedules: Vec<DirectorySchedule>,
}

/// A `tools/*.md` script that the host runs through the existing `program` tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryScript {
    pub name: String,
    pub description: String,
    /// Path relative to the agent directory, which is the session workspace.
    pub path: String,
    pub allowed_tools: Vec<String>,
}

/// A schedule whose prompt is a user fact, not a timer inside the actor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueSchedule {
    pub name: String,
    /// Stable fact-log identity. The host resumes this session and calls `send`.
    pub session_key: String,
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectorySchedule {
    name: String,
    prompt: String,
    cron: CronExpr,
}

impl DirectoryProjection {
    /// Load `instructions.md` and the optional `agent.acl`, `skills/`, `tools/`,
    /// and `schedules/` children. Missing instructions fail closed.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        let root = std::fs::canonicalize(dir)
            .with_context(|| format!("agent directory does not exist: {}", dir.display()))?;
        if !root.is_dir() {
            bail!("agent directory is not a directory: {}", root.display());
        }

        let instructions = read_bounded(
            &root.join("instructions.md"),
            MAX_INSTRUCTIONS_BYTES,
            "instructions.md",
        )?;
        let instructions = instructions.trim().to_owned();
        if instructions.is_empty() {
            bail!("instructions.md is empty");
        }

        let (model, tool_budget) = load_agent_acl(&root.join("agent.acl"))?;
        let skill_candidate = root.join("skills");
        let skill_dir =
            (skill_candidate.is_dir() && !skill_candidate.is_symlink()).then_some(skill_candidate);

        let mut mcp_servers = Vec::new();
        let mut scripts = Vec::new();
        load_tools(&root, &mut mcp_servers, &mut scripts)?;

        let schedules = load_schedules(&root)?;

        Ok(Self {
            instructions,
            model,
            tool_budget,
            skill_dir,
            mcp_servers,
            scripts,
            schedules,
        })
    }

    /// Stock compose list. Instructions occupy the existing `system` part.
    pub fn compose_options(&self) -> Result<HarnessComposeOptions> {
        HarnessComposeOptions::compose(
            STOCK_COMPONENTS.map(str::to_owned).to_vec(),
            self.tool_budget,
            None,
            vec![self.instructions.clone()],
        )
    }

    /// Session options for the one fact-log actor. MCP servers are applied
    /// afterwards with `add_mcp_server`; script files are arguments to `program`.
    pub fn session_options(&self) -> Result<SessionOptions> {
        let mut options = SessionOptions::new().with_harness(self.compose_options()?);
        if let Some(model) = &self.model {
            options = options.with_model(model.clone());
        }
        if let Some(dir) = &self.skill_dir {
            options = options.with_skill_dirs([dir.clone()]);
        }
        Ok(options)
    }

    pub fn mcp_servers(&self) -> &[McpServerConfig] {
        &self.mcp_servers
    }

    pub fn scripts(&self) -> &[DirectoryScript] {
        &self.scripts
    }

    /// Schedules whose cron matches `now` (UTC). The host sends each prompt.
    pub fn due(&self, now: DateTime<Utc>) -> Vec<DueSchedule> {
        self.schedules
            .iter()
            .filter(|schedule| schedule.cron.matches(now))
            .map(|schedule| DueSchedule {
                name: schedule.name.clone(),
                session_key: format!("schedule:{}", schedule.name),
                prompt: schedule.prompt.clone(),
            })
            .collect()
    }
}

impl DirectoryScript {
    /// Arguments for the existing `program` tool. `allowed_tools` is always set
    /// so an omitted list cannot fall through to every registered tool.
    pub fn program_arguments(&self) -> serde_json::Value {
        json!({
            "language": "javascript",
            "path": self.path,
            "allowed_tools": self.allowed_tools,
        })
    }
}

fn load_agent_acl(path: &Path) -> Result<(Option<String>, Option<u32>)> {
    if !path.is_file() {
        return Ok((None, None));
    }
    let text = read_bounded(path, MAX_MARKDOWN_BYTES, "agent.acl")?;
    let document = a3s_acl::parse_acl(&text).context("agent.acl did not parse")?;
    let agent = document
        .blocks
        .iter()
        .find(|block| block.name == "agent")
        .context("agent.acl requires an agent { } block")?;
    let model = agent
        .attributes
        .get("model")
        .map(|value| match value {
            a3s_acl::Value::String(text) if !text.trim().is_empty() => Ok(text.clone()),
            _ => bail!("agent.acl model must be a non-empty string"),
        })
        .transpose()?;
    let tool_budget = agent
        .attributes
        .get("tool_budget")
        .map(|value| match value {
            a3s_acl::Value::Number(number)
                if number.fract() == 0.0 && *number >= 0.0 && *number <= u32::MAX as f64 =>
            {
                Ok(*number as u32)
            }
            _ => bail!("agent.acl tool_budget must be a non-negative integer"),
        })
        .transpose()?;
    Ok((model, tool_budget))
}

fn load_tools(
    root: &Path,
    mcp_servers: &mut Vec<McpServerConfig>,
    scripts: &mut Vec<DirectoryScript>,
) -> Result<()> {
    let tools_dir = root.join("tools");
    if !tools_dir.is_dir() {
        return Ok(());
    }
    let mut paths = markdown_files(&tools_dir)?;
    if paths.len() > MAX_TOOLS {
        bail!("tools/ has more than {MAX_TOOLS} markdown files");
    }
    paths.sort();
    for path in paths {
        let name = file_stem(&path)?;
        let text = read_bounded(&path, MAX_MARKDOWN_BYTES, "tool markdown")?;
        let (front, body) = split_frontmatter(&text)
            .with_context(|| format!("tool {} is missing frontmatter", path.display()))?;
        let value: serde_yaml::Value =
            serde_yaml::from_str(&front).context("tool frontmatter is not YAML")?;
        let kind = yaml_string(&value, "kind").context("tool frontmatter requires kind")?;
        match kind.as_str() {
            "mcp" => mcp_servers.push(mcp_server(&name, &value)?),
            "script" => scripts.push(script_tool(root, &name, &value, body.trim())?),
            other => bail!("tool {name} has unknown kind `{other}`"),
        }
    }
    Ok(())
}

fn mcp_server(name: &str, value: &serde_yaml::Value) -> Result<McpServerConfig> {
    let command = yaml_string(value, "command");
    let url = yaml_string(value, "url");
    let transport = match (command, url) {
        (Some(command), None) => McpTransportConfig::Stdio {
            command,
            args: yaml_string_list(value, "args").unwrap_or_default(),
        },
        (None, Some(url)) => McpTransportConfig::StreamableHttp {
            url,
            headers: HashMap::new(),
        },
        _ => bail!("mcp tool {name} requires exactly one of command or url"),
    };
    Ok(McpServerConfig {
        name: name.to_owned(),
        transport,
        enabled: true,
        env: HashMap::new(),
        oauth: None,
        tool_timeout_secs: 60,
    })
}

fn script_tool(
    root: &Path,
    name: &str,
    value: &serde_yaml::Value,
    description: &str,
) -> Result<DirectoryScript> {
    let relative = yaml_string(value, "path").context("script tool requires path")?;
    let relative_path = Path::new(&relative);
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        bail!("script tool {name} path must stay inside the agent directory");
    }
    if !relative.ends_with(".js") && !relative.ends_with(".mjs") {
        bail!("script tool {name} path must be a .js or .mjs file");
    }
    let full = root.join(relative_path);
    let canonical = std::fs::canonicalize(&full)
        .with_context(|| format!("script tool {name} file is missing"))?;
    if !canonical.starts_with(root) || !canonical.is_file() {
        bail!("script tool {name} file escapes the agent directory");
    }
    let allowed_tools =
        yaml_string_list(value, "allowed_tools").context("script tool requires allowed_tools")?;
    Ok(DirectoryScript {
        name: name.to_owned(),
        description: description.to_owned(),
        path: relative.replace('\\', "/"),
        allowed_tools,
    })
}

fn load_schedules(root: &Path) -> Result<Vec<DirectorySchedule>> {
    let dir = root.join("schedules");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = markdown_files(&dir)?;
    if paths.len() > MAX_SCHEDULES {
        bail!("schedules/ has more than {MAX_SCHEDULES} markdown files");
    }
    paths.sort();
    let mut schedules = Vec::with_capacity(paths.len());
    for path in paths {
        let name = file_stem(&path)?;
        let text = read_bounded(&path, MAX_MARKDOWN_BYTES, "schedule markdown")?;
        let (front, body) = split_frontmatter(&text)
            .with_context(|| format!("schedule {name} is missing frontmatter"))?;
        let value: serde_yaml::Value =
            serde_yaml::from_str(&front).context("schedule frontmatter is not YAML")?;
        let cron = yaml_string(&value, "cron").context("schedule requires cron")?;
        let prompt = body.trim().to_owned();
        if prompt.is_empty() {
            bail!("schedule {name} prompt is empty");
        }
        if prompt.len() > MAX_SCHEDULE_PROMPT_BYTES {
            bail!("schedule {name} prompt is too large");
        }
        schedules.push(DirectorySchedule {
            name,
            prompt,
            cron: CronExpr::parse(&cron)?,
        });
    }
    Ok(schedules)
}

fn markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("md") && path.is_file() {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn file_stem(path: &Path) -> Result<String> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .context("file name is not UTF-8")?;
    if !valid_name(stem) {
        bail!("file name `{stem}` must be ascii letters, digits, '_' or '-'");
    }
    Ok(stem.to_owned())
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    name.len() <= 64 && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

fn read_bounded(path: &Path, max_bytes: usize, label: &str) -> Result<String> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("{label} is missing: {}", path.display()))?;
    if !metadata.is_file() || metadata.len() > max_bytes as u64 {
        bail!("{label} must be a file of at most {max_bytes} bytes");
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if text.len() > max_bytes {
        bail!("{label} must be at most {max_bytes} bytes");
    }
    Ok(text)
}

fn split_frontmatter(text: &str) -> Result<(String, String)> {
    let text = text.trim_start_matches('\u{feff}');
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
        .context("frontmatter must start with ---")?;
    let (front, body) = rest
        .split_once("\n---")
        .context("frontmatter must end with ---")?;
    let body = body.trim_start_matches(['\r', '\n']);
    Ok((front.to_owned(), body.to_owned()))
}

fn yaml_string(value: &serde_yaml::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|item| match item {
        serde_yaml::Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        serde_yaml::Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

fn yaml_string_list(value: &serde_yaml::Value, key: &str) -> Option<Vec<String>> {
    let item = value.get(key).or_else(|| {
        key.split('_')
            .next()
            .and_then(|_| value.get(key.replace('_', "-")))
    })?;
    match item {
        serde_yaml::Value::String(text) => Some(
            text.split_whitespace()
                .map(str::to_owned)
                .filter(|part| !part.is_empty())
                .collect(),
        ),
        serde_yaml::Value::Sequence(items) => Some(
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect(),
        ),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CronExpr {
    second: Option<CronField>,
    minute: CronField,
    hour: CronField,
    day_of_month: CronField,
    month: CronField,
    day_of_week: CronField,
    day_of_month_any: bool,
    day_of_week_any: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CronField {
    matches: Vec<bool>,
}

impl CronExpr {
    fn parse(expr: &str) -> Result<Self> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        let (second, rest) = match fields.as_slice() {
            [minute, hour, dom, month, dow] => (None, [*minute, *hour, *dom, *month, *dow]),
            [second, minute, hour, dom, month, dow] => (
                Some(CronField::parse(second, 0, 59)?),
                [*minute, *hour, *dom, *month, *dow],
            ),
            _ => bail!("cron `{expr}` must have 5 or 6 fields"),
        };
        let day_of_month_any = rest[2] == "*";
        let day_of_week_any = rest[4] == "*";
        Ok(Self {
            second,
            minute: CronField::parse(rest[0], 0, 59)?,
            hour: CronField::parse(rest[1], 0, 23)?,
            day_of_month: CronField::parse(rest[2], 1, 31)?,
            month: CronField::parse(rest[3], 1, 12)?,
            day_of_week: CronField::parse_dow(rest[4])?,
            day_of_month_any,
            day_of_week_any,
        })
    }

    fn matches(&self, now: DateTime<Utc>) -> bool {
        if let Some(second) = &self.second {
            if !second.contains(now.second()) {
                return false;
            }
        }
        if !self.minute.contains(now.minute()) || !self.hour.contains(now.hour()) {
            return false;
        }
        if !self.month.contains(now.month()) {
            return false;
        }
        let dom = self.day_of_month.contains(now.day());
        let dow = self
            .day_of_week
            .contains(now.weekday().num_days_from_sunday());
        match (self.day_of_month_any, self.day_of_week_any) {
            (true, true) => true,
            (false, true) => dom,
            (true, false) => dow,
            (false, false) => dom || dow,
        }
    }
}

impl CronField {
    fn parse(spec: &str, min: u32, max: u32) -> Result<Self> {
        let mut matches = vec![false; (max as usize) + 1];
        if spec.trim().is_empty() {
            bail!("cron field is empty");
        }
        for part in spec.split(',') {
            apply_part(part.trim(), min, max, &mut matches)?;
        }
        if !matches.iter().any(|on| *on) {
            bail!("cron field `{spec}` matches nothing");
        }
        Ok(Self { matches })
    }

    fn parse_dow(spec: &str) -> Result<Self> {
        let mut field = Self::parse(spec, 0, 7)?;
        if field.contains(7) {
            field.matches[0] = true;
        }
        if field.contains(0) && field.matches.len() > 7 {
            field.matches[7] = true;
        }
        Ok(field)
    }

    fn contains(&self, value: u32) -> bool {
        self.matches.get(value as usize).copied().unwrap_or(false)
    }
}

fn apply_part(part: &str, min: u32, max: u32, matches: &mut [bool]) -> Result<()> {
    let (range, step) = part
        .split_once('/')
        .map(|(range, step)| (range, Some(step)))
        .unwrap_or((part, None));
    let step: u32 = match step {
        Some(step) => step
            .parse()
            .ok()
            .filter(|step| *step >= 1)
            .context("cron step must be an integer >= 1")?,
        None => 1,
    };
    let (start, end) = if range == "*" {
        (min, max)
    } else if let Some((start, end)) = range.split_once('-') {
        (parse_bound(start, min, max)?, parse_bound(end, min, max)?)
    } else {
        let value = parse_bound(range, min, max)?;
        (value, value)
    };
    if start > end {
        bail!("cron range {start}-{end} is reversed");
    }
    let mut value = start;
    while value <= end {
        matches[value as usize] = true;
        value = value.saturating_add(step);
        if step == 0 {
            break;
        }
    }
    Ok(())
}

fn parse_bound(text: &str, min: u32, max: u32) -> Result<u32> {
    let value: u32 = text
        .parse()
        .ok()
        .filter(|value| *value >= min && *value <= max)
        .with_context(|| format!("cron value `{text}` is outside {min}-{max}"))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, relative: &str, body: &str) {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    fn sample(dir: &Path) {
        write(
            dir,
            "instructions.md",
            "You keep the repository building.\n",
        );
        write(
            dir,
            "agent.acl",
            "agent {\n  model = \"openai/gpt-4o\"\n  tool_budget = 4\n}\n",
        );
        write(
            dir,
            "skills/review/SKILL.md",
            "---\nname: review\n---\nReview.\n",
        );
        write(
            dir,
            "tools/run.js",
            "async function run(ctx, inputs) { return inputs; }\n",
        );
        write(
            dir,
            "tools/review.md",
            "---\nkind: script\npath: tools/run.js\nallowed_tools:\n  - read\n---\nReview the diff.\n",
        );
        write(
            dir,
            "tools/github.md",
            "---\nkind: mcp\ncommand: npx\nargs:\n  - -y\n  - server\n---\nGitHub.\n",
        );
        write(
            dir,
            "schedules/morning.md",
            "---\ncron: \"0 9 * * *\"\n---\nSummarize overnight changes.\n",
        );
    }

    #[test]
    fn directory_projects_onto_the_stock_harness() {
        let dir = tempfile::tempdir().unwrap();
        sample(dir.path());
        let projection = DirectoryProjection::load(dir.path()).unwrap();
        let compose = projection.compose_options().unwrap();
        assert_eq!(
            compose.components,
            vec!["system", "tools", "budget", "compact", "infer"]
        );
        assert_eq!(compose.system, vec!["You keep the repository building."]);
        assert_eq!(compose.tool_budget, Some(4));

        let options = projection.session_options().unwrap();
        assert_eq!(options.model.as_deref(), Some("openai/gpt-4o"));
        assert_eq!(options.skill_dirs.len(), 1);
        assert!(options.skill_dirs[0].ends_with("skills"));
        assert!(options.harness.is_some());

        assert_eq!(projection.mcp_servers().len(), 1);
        assert_eq!(projection.mcp_servers()[0].name, "github");
        let script = &projection.scripts()[0];
        assert_eq!(script.name, "review");
        assert_eq!(
            script.program_arguments(),
            json!({
                "language": "javascript",
                "path": "tools/run.js",
                "allowed_tools": ["read"],
            })
        );
    }

    #[test]
    fn seam_admission_directory_skills_and_schedules() {
        let dir = tempfile::tempdir().unwrap();
        sample(dir.path());
        let projection = DirectoryProjection::load(dir.path()).unwrap();
        let options = projection.session_options().unwrap();
        assert_eq!(options.skill_dirs.len(), 1);
        assert!(options.skill_dirs[0].ends_with("skills"));

        let due = DateTime::parse_from_rfc3339("2026-09-27T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let fires = projection.due(due);
        assert_eq!(fires.len(), 1);
        assert_eq!(fires[0].session_key, "schedule:morning");
        assert_eq!(fires[0].name, "morning");
        assert!(!fires[0].prompt.is_empty());

        let idle = DateTime::parse_from_rfc3339("2026-09-27T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(projection.due(idle).is_empty());
    }

    #[test]
    fn due_schedule_is_a_prompt_for_the_existing_send_path() {
        let dir = tempfile::tempdir().unwrap();
        sample(dir.path());
        let projection = DirectoryProjection::load(dir.path()).unwrap();
        let due = DateTime::parse_from_rfc3339("2026-09-27T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let fires = projection.due(due);
        assert_eq!(fires.len(), 1);
        assert_eq!(fires[0].session_key, "schedule:morning");
        assert_eq!(fires[0].prompt, "Summarize overnight changes.");

        let idle = DateTime::parse_from_rfc3339("2026-09-27T09:01:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(projection.due(idle).is_empty());
    }

    #[test]
    fn script_path_cannot_escape_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "instructions.md", "Stay inside.\n");
        write(
            dir.path(),
            "tools/escape.md",
            "---\nkind: script\npath: ../outside.js\nallowed_tools: []\n---\nEscape.\n",
        );
        let error = DirectoryProjection::load(dir.path()).unwrap_err();
        assert!(error.to_string().contains("stay inside"));
    }

    #[test]
    fn missing_instructions_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let error = DirectoryProjection::load(dir.path()).unwrap_err();
        assert!(error.to_string().contains("instructions.md"));
    }
}
