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

        let acl = load_agent_acl(&root.join("agent.acl"))?;
        let skill_candidate = root.join("skills");
        let skill_dir =
            (skill_candidate.is_dir() && !skill_candidate.is_symlink()).then_some(skill_candidate);

        // ACL is the declaration of record (CFG-YAML1, ratified
        // 2026-10-09): agent.acl's `mcp_servers`, `scripts`, and
        // `schedules` blocks declare servers, script tools, and scheduled
        // prompts; the markdown frontmatter shapes ride a deprecation
        // period and yield to an ACL declaration of the same name.
        let mut mcp_servers = acl.mcp_servers;
        for script in &acl.scripts {
            validate_script_path(&root, &script.name, &script.path)?;
        }
        let mut scripts = acl.scripts;
        load_tools(&root, &mut mcp_servers, &mut scripts)?;

        let mut schedules = Vec::new();
        for declaration in acl.schedules {
            let prompt = read_bounded(
                &root.join(&declaration.prompt_path),
                MAX_SCHEDULE_PROMPT_BYTES,
                "schedule prompt",
            )
            .with_context(|| {
                format!(
                    "schedule {} prompt file {} is unreadable",
                    declaration.name, declaration.prompt_path
                )
            })?;
            let prompt = prompt.trim().to_owned();
            if prompt.is_empty() {
                bail!("schedule {} prompt is empty", declaration.name);
            }
            schedules.push(DirectorySchedule {
                name: declaration.name,
                prompt,
                cron: declaration.cron,
            });
        }
        load_schedules(&root, &mut schedules)?;

        Ok(Self {
            instructions,
            model: acl.model,
            tool_budget: acl.tool_budget,
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

/// One `schedules "name" { cron, prompt }` declaration from agent.acl;
/// the prompt file resolves at load time.
struct AclScheduleDeclaration {
    name: String,
    cron: CronExpr,
    prompt_path: String,
}

/// The agent.acl declaration of record: model/tool_budget plus the
/// `mcp_servers`, `scripts`, and `schedules` block families.
struct AgentAcl {
    model: Option<String>,
    tool_budget: Option<u32>,
    mcp_servers: Vec<McpServerConfig>,
    scripts: Vec<DirectoryScript>,
    schedules: Vec<AclScheduleDeclaration>,
}

fn load_agent_acl(path: &Path) -> Result<AgentAcl> {
    if !path.is_file() {
        return Ok(AgentAcl {
            model: None,
            tool_budget: None,
            mcp_servers: Vec::new(),
            scripts: Vec::new(),
            schedules: Vec::new(),
        });
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
    // The `mcp_servers` block family (the same names the config loader
    // accepts), plus the `scripts` and `schedules` families, declare this
    // directory's wiring in ACL — the declaration of record since
    // CFG-YAML1. Cron validates here, fail closed; script paths and
    // allowed_tools carry the same rules the markdown shapes enforced.
    let mut mcp_servers = Vec::new();
    let mut scripts = Vec::new();
    let mut schedules = Vec::new();
    for block in &document.blocks {
        match block.name.as_str() {
            "mcp_servers" | "mcpServers" | "mcp_server" => {
                mcp_servers.push(crate::config::parse_mcp_server_block(block)?);
            }
            "scripts" => scripts.push(acl_script_block(block)?),
            "schedules" => schedules.push(acl_schedule_block(block)?),
            _ => {}
        }
    }
    Ok(AgentAcl {
        model,
        tool_budget,
        mcp_servers,
        scripts,
        schedules,
    })
}

fn acl_string_attr(block: &a3s_acl::Block, key: &str) -> Result<String> {
    match block.attributes.get(key) {
        Some(a3s_acl::Value::String(text)) if !text.trim().is_empty() => Ok(text.clone()),
        _ => bail!(
            "agent.acl {} block requires a non-empty {key} string",
            block.name
        ),
    }
}

fn acl_string_list_attr(block: &a3s_acl::Block, key: &str) -> Result<Vec<String>> {
    match block.attributes.get(key) {
        Some(a3s_acl::Value::List(items)) => {
            let names = items
                .iter()
                .map(|item| match item {
                    a3s_acl::Value::String(text) if !text.trim().is_empty() => Ok(text.clone()),
                    _ => bail!(
                        "agent.acl {} block {key} entries must be non-empty strings",
                        block.name
                    ),
                })
                .collect::<Result<Vec<_>>>()?;
            if names.is_empty() {
                bail!(
                    "agent.acl {} block requires a non-empty {key} list",
                    block.name
                );
            }
            Ok(names)
        }
        _ => bail!(
            "agent.acl {} block requires a non-empty {key} list",
            block.name
        ),
    }
}

fn acl_block_name(block: &a3s_acl::Block) -> Result<String> {
    match block.labels.first() {
        Some(label) if !label.trim().is_empty() => Ok(label.clone()),
        _ => bail!("agent.acl {} block requires a name label", block.name),
    }
}

/// `scripts "name" { path, allowed_tools, description? }` — the script
/// file stays content; its wiring and authority declare in ACL.
fn acl_script_block(block: &a3s_acl::Block) -> Result<DirectoryScript> {
    let name = acl_block_name(block)?;
    let relative = acl_string_attr(block, "path")?;
    // Path containment and existence validate against the root in
    // `load()` (via `validate_script_path`), where the root is known.
    Ok(DirectoryScript {
        name,
        description: block
            .attributes
            .get("description")
            .and_then(|value| match value {
                a3s_acl::Value::String(text) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default(),
        path: relative.replace('\\', "/"),
        allowed_tools: acl_string_list_attr(block, "allowed_tools")?,
    })
}

/// `schedules "name" { cron, prompt }` — cron validates here (fail
/// closed, the parse error names the field); the prompt file's content
/// is read bounded at load time.
fn acl_schedule_block(block: &a3s_acl::Block) -> Result<AclScheduleDeclaration> {
    let name = acl_block_name(block)?;
    let cron_text = acl_string_attr(block, "cron")?;
    let cron = CronExpr::parse(&cron_text)
        .with_context(|| format!("agent.acl schedules {name} cron is invalid"))?;
    let prompt_path = acl_string_attr(block, "prompt")?;
    Ok(AclScheduleDeclaration {
        name,
        cron,
        prompt_path,
    })
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
        let already_declared =
            |names: &[McpServerConfig]| names.iter().any(|server| server.name == name);
        match kind.as_str() {
            "mcp" => {
                if already_declared(mcp_servers) {
                    tracing::warn!(
                        tool = %name,
                        "tools/{name}.md is superseded by the agent.acl mcp_servers \
                         declaration of the same name"
                    );
                    continue;
                }
                // CFG-YAML1 deprecation period: the shape still loads, with
                // a warning naming the declaration of record.
                tracing::warn!(
                    tool = %path.display(),
                    "tool markdown `kind: mcp` is deprecated — declare the server in \
                     agent.acl's mcp_servers block; the markdown shape will be refused \
                     at the end of the deprecation period"
                );
                mcp_servers.push(mcp_server(&name, &value)?);
            }
            "script" => {
                if scripts.iter().any(|script| script.name == name) {
                    tracing::warn!(
                        tool = %name,
                        "tools/{name}.md is superseded by the agent.acl scripts \
                         declaration of the same name"
                    );
                    continue;
                }
                tracing::warn!(
                    tool = %path.display(),
                    "tool markdown `kind: script` is deprecated — declare the wiring in \
                     agent.acl's scripts block (path plus allowed_tools); the markdown \
                     shape will be refused at the end of the deprecation period"
                );
                scripts.push(script_tool(root, &name, &value, body.trim())?);
            }
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
    let normalized = validate_script_path(root, name, &relative)?;
    let allowed_tools =
        yaml_string_list(value, "allowed_tools").context("script tool requires allowed_tools")?;
    Ok(DirectoryScript {
        name: name.to_owned(),
        description: description.to_owned(),
        path: normalized,
        allowed_tools,
    })
}

/// A script tool's path must stay inside the agent directory, be a
/// .js/.mjs file, and exist — the same rules for the ACL declaration
/// and the deprecated markdown shape. Returns the normalized
/// forward-slash relative path.
fn validate_script_path(root: &Path, name: &str, relative: &str) -> Result<String> {
    let relative_path = Path::new(relative);
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
    Ok(relative.replace('\\', "/"))
}

fn load_schedules(root: &Path, schedules: &mut Vec<DirectorySchedule>) -> Result<()> {
    let dir = root.join("schedules");
    if !dir.is_dir() {
        return Ok(());
    }
    let mut paths = markdown_files(&dir)?;
    if paths.len() > MAX_SCHEDULES {
        bail!("schedules/ has more than {MAX_SCHEDULES} markdown files");
    }
    paths.sort();
    for path in paths {
        let name = file_stem(&path)?;
        if schedules.iter().any(|schedule| schedule.name == name) {
            tracing::warn!(
                schedule = %name,
                "schedules/{name}.md is superseded by the agent.acl declaration of the same name"
            );
            continue;
        }
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
        // CFG-YAML1 deprecation period: the shape still loads, with a
        // warning naming the declaration of record.
        tracing::warn!(
            schedule = %name,
            "schedules/*.md frontmatter is deprecated — declare it in agent.acl's \
             schedules block (cron plus a prompt file path); the markdown shape will \
             be refused at the end of the deprecation period"
        );
        schedules.push(DirectorySchedule {
            name,
            prompt,
            cron: CronExpr::parse(&cron)?,
        });
    }
    Ok(())
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

pub(crate) fn split_frontmatter(text: &str) -> Result<(String, String)> {
    // Fence lines accept LF, CRLF, and CR. The YAML slice is joined with LF
    // so a Windows checkout cannot leave `\r` on `kind`, `command`, `args`,
    // or `url` — `yaml_string` keeps the scalar bytes it parsed.
    let text = text.trim_start_matches('\u{feff}');
    let Some((opening, mut offset)) = next_logical_line(text, 0) else {
        bail!("frontmatter must start with ---");
    };
    if opening != "---" {
        bail!("frontmatter must start with ---");
    }
    let mut lines = Vec::new();
    loop {
        let Some((line, next)) = next_logical_line(text, offset) else {
            bail!("frontmatter must end with ---");
        };
        offset = next;
        if line == "---" {
            break;
        }
        lines.push(line);
    }
    let body = text[offset..].trim_start_matches(['\r', '\n']);
    Ok((lines.join("\n"), body.to_owned()))
}

/// One line with its ending removed. The returned index is the next line.
/// A final line with no ending is still a line: the file ended.
fn next_logical_line(text: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = text.as_bytes();
    if start >= bytes.len() {
        return None;
    }
    let mut index = start;
    while index < bytes.len() && bytes[index] != b'\n' && bytes[index] != b'\r' {
        index += 1;
    }
    let line = &text[start..index];
    if index == bytes.len() {
        return Some((line, index));
    }
    let next = if bytes[index] == b'\r' && index + 1 < bytes.len() && bytes[index + 1] == b'\n' {
        index + 2
    } else {
        index + 1
    };
    Some((line, next))
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
    use chrono::TimeZone;

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

    #[test]
    fn agent_acl_mcp_servers_are_the_declaration_of_record() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "instructions.md",
            "Run the checklist and report.",
        );
        write(
            dir.path(),
            "agent.acl",
            r#"
agent {
  model = "mock/checklist"
}

mcp_servers "probe" {
  transport = "stdio"
  command = "node"
  args = ["probe-server.js"]
}
"#,
        );
        let projection = DirectoryProjection::load(dir.path()).expect("projection loads");
        let servers = projection.mcp_servers();
        assert_eq!(servers.len(), 1, "the ACL block declares the server");
        assert_eq!(servers[0].name, "probe");
        match &servers[0].transport {
            McpTransportConfig::Stdio { command, args } => {
                assert_eq!(command, "node");
                assert_eq!(args, &vec!["probe-server.js".to_string()]);
            }
            other => panic!("stdio transport expected, got {other:?}"),
        }
    }

    #[test]
    fn crlf_tool_markdown_mcp_loads_command_without_carriage_return() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "instructions.md", "Run the probe.\r\n");
        write(
            dir.path(),
            "agent.acl",
            "agent { model = \"mock/probe\" }\r\n",
        );
        write(
            dir.path(),
            "tools/probe.md",
            "---\r\nkind: mcp\r\ncommand: node\r\nargs:\r\n  - probe.js\r\n---\r\nProbe body.\r\n",
        );
        let projection = DirectoryProjection::load(dir.path()).expect("crlf mcp tool loads");
        let server = projection
            .mcp_servers()
            .iter()
            .find(|server| server.name == "probe")
            .expect("probe server");
        match &server.transport {
            McpTransportConfig::Stdio { command, args } => {
                assert_eq!(command, "node");
                assert_eq!(args, &vec!["probe.js".to_string()]);
                assert!(!command.contains('\r'));
                assert!(args.iter().all(|arg| !arg.contains('\r')));
            }
            other => panic!("stdio transport expected, got {other:?}"),
        }
    }

    #[test]
    fn crlf_agent_acl_mcp_server_loads() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "instructions.md", "Run the checklist.\r\n");
        write(
            dir.path(),
            "agent.acl",
            "agent {\r\n  model = \"mock/checklist\"\r\n}\r\n\r\nmcp_servers \"probe\" {\r\n  transport = \"stdio\"\r\n  command = \"node\"\r\n  args = [\"probe-server.js\"]\r\n}\r\n",
        );
        let projection = DirectoryProjection::load(dir.path()).expect("crlf agent.acl loads");
        let server = &projection.mcp_servers()[0];
        assert_eq!(server.name, "probe");
        match &server.transport {
            McpTransportConfig::Stdio { command, args } => {
                assert_eq!(command, "node");
                assert_eq!(args, &vec!["probe-server.js".to_string()]);
            }
            other => panic!("stdio transport expected, got {other:?}"),
        }
    }

    #[test]
    fn tool_markdown_mcp_shape_still_loads_during_the_deprecation_period() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "instructions.md", "Run the probe.");
        write(
            dir.path(),
            "agent.acl",
            "agent { model = \"mock/probe\" }\n",
        );
        write(
            dir.path(),
            "tools/probe.md",
            "---\nkind: mcp\ncommand: node\nargs:\n  - probe.js\n---\nProbe body.\n",
        );
        // CFG-YAML1: the shape warns and still loads during the period;
        // the refusal lands at its end.
        let projection = DirectoryProjection::load(dir.path())
            .expect("deprecated shape still loads during the period");
        assert!(projection
            .mcp_servers()
            .iter()
            .any(|server| server.name == "probe"));
    }

    #[test]
    fn agent_acl_schedules_and_scripts_are_the_declaration_of_record() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "instructions.md", "Run the checklist.");
        write(
            dir.path(),
            "agent.acl",
            r#"
agent { model = "mock/checklist" }

schedules "daily" {
  cron = "0 9 * * *"
  prompt = "prompts/daily.md"
}

scripts "fmt" {
  path = "tools/fmt.mjs"
  allowed_tools = ["bash"]
}
"#,
        );
        write(dir.path(), "prompts/daily.md", "Run the morning checks.\n");
        write(dir.path(), "tools/fmt.mjs", "export default 1;\n");
        let projection = DirectoryProjection::load(dir.path()).expect("projection loads");
        let scripts = projection.scripts();
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].name, "fmt");
        assert_eq!(scripts[0].path, "tools/fmt.mjs");
        assert_eq!(scripts[0].allowed_tools, vec!["bash".to_string()]);
        // The schedule's prompt resolves from its file at load time.
        let due = projection.due(Utc.with_ymd_and_hms(2026, 10, 10, 9, 0, 0).unwrap());
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "daily");
        assert_eq!(due[0].prompt.trim(), "Run the morning checks.");
    }

    #[test]
    fn markdown_schedules_and_scripts_yield_to_acl_names() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "instructions.md", "Run the checklist.");
        write(
            dir.path(),
            "agent.acl",
            "agent { model = \"mock/checklist\" }\n\nschedules \"daily\" {\n  cron = \"0 9 * * *\"\n  prompt = \"prompts/daily.md\"\n}\n\nscripts \"fmt\" {\n  path = \"tools/fmt.mjs\"\n  allowed_tools = [\"bash\"]\n}\n",
        );
        write(dir.path(), "prompts/daily.md", "ACL morning checks.\n");
        write(dir.path(), "tools/fmt.mjs", "export default 1;\n");
        // The deprecated markdown shapes carry DIFFERENT payloads under
        // the SAME names — the ACL declarations must win.
        write(
            dir.path(),
            "schedules/daily.md",
            "---\ncron: \"0 23 * * *\"\n---\nMarkdown evening checks.\n",
        );
        write(
            dir.path(),
            "tools/fmt.md",
            "---\nkind: script\npath: fmt.mjs\ndescription: markdown shape\nallowed_tools:\n  - web_search\n---\nbody\n",
        );
        write(dir.path(), "fmt.mjs", "export default 2;\n");
        let projection = DirectoryProjection::load(dir.path()).expect("projection loads");
        let scripts = projection.scripts();
        assert_eq!(scripts.len(), 1, "the ACL script wins the name");
        assert_eq!(scripts[0].allowed_tools, vec!["bash".to_string()]);
        let due = projection.due(Utc.with_ymd_and_hms(2026, 10, 10, 9, 0, 0).unwrap());
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].prompt.trim(), "ACL morning checks.");
    }
}
