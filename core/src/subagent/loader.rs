use super::*;

/// Parse an agent definition from YAML content.
///
/// The YAML can describe either a full [`AgentDefinition`] or a cattle-style
/// [`WorkerAgentSpec`] by including a `kind` field.
pub fn parse_agent_yaml(content: &str) -> anyhow::Result<AgentDefinition> {
    let value: serde_yaml::Value = serde_yaml::from_str(content)
        .map_err(|e| anyhow::anyhow!("Failed to parse agent YAML: {}", e))?;

    parse_agent_yaml_value(value, "agent YAML")
}

fn parse_agent_yaml_value(
    value: serde_yaml::Value,
    context: &str,
) -> anyhow::Result<AgentDefinition> {
    let tools = yaml_get_any(&value, &["tools", "allowedTools", "allowed_tools"])
        .map(parse_tools_field)
        .unwrap_or_default();
    let disallowed_tools = yaml_get_any(
        &value,
        &["disallowedTools", "disallowed-tools", "disallowed_tools"],
    )
    .map(parse_tools_field)
    .unwrap_or_default();

    if yaml_value_has_key(&value, "kind") {
        let mut spec: WorkerAgentSpec = serde_yaml::from_value(value)
            .map_err(|e| anyhow::anyhow!("Failed to parse worker {}: {}", context, e))?;
        validate_agent_name(&spec.name)?;
        apply_claude_style_tools_to_spec(&mut spec, &tools, &disallowed_tools);
        return Ok(spec.into_agent_definition());
    }

    let mut agent: AgentDefinition = serde_yaml::from_value(value)
        .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", context, e))?;
    validate_agent_name(&agent.name)?;
    apply_claude_style_tools_to_agent(&mut agent, &tools, &disallowed_tools);
    Ok(agent)
}

fn apply_claude_style_tools_to_agent(
    agent: &mut AgentDefinition,
    tools: &[String],
    disallowed_tools: &[String],
) {
    if !tools.is_empty() {
        agent.permissions = allow_only_permission_policy(tools);
    }
    if !disallowed_tools.is_empty() {
        let base = std::mem::take(&mut agent.permissions);
        agent.permissions = add_denied_tools(base, disallowed_tools);
    }
    if (!tools.is_empty() || !disallowed_tools.is_empty())
        && agent.confirmation_inheritance.is_none()
    {
        agent.confirmation_inheritance = Some(ConfirmationInheritance::AutoApprove);
    }
}

fn apply_claude_style_tools_to_spec(
    spec: &mut WorkerAgentSpec,
    tools: &[String],
    disallowed_tools: &[String],
) {
    if tools.is_empty() && disallowed_tools.is_empty() {
        return;
    }

    let base = if tools.is_empty() {
        spec.permissions
            .clone()
            .unwrap_or_else(|| spec.kind.default_permissions())
    } else {
        allow_only_permission_policy(tools)
    };
    spec.permissions = Some(add_denied_tools(base, disallowed_tools));
    if spec.confirmation_inheritance.is_none() {
        spec.confirmation_inheritance = Some(ConfirmationInheritance::AutoApprove);
    }
}

fn parse_worker_yaml_value(
    value: serde_yaml::Value,
    context: &str,
) -> anyhow::Result<WorkerAgentSpec> {
    let spec: WorkerAgentSpec = serde_yaml::from_value(value)
        .map_err(|e| anyhow::anyhow!("Failed to parse worker {}: {}", context, e))?;
    validate_agent_name(&spec.name)?;
    Ok(spec)
}

fn yaml_value_has_key(value: &serde_yaml::Value, key: &str) -> bool {
    value
        .as_mapping()
        .map(|mapping| mapping.contains_key(serde_yaml::Value::String(key.to_string())))
        .unwrap_or(false)
}

fn yaml_get<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    value
        .as_mapping()
        .and_then(|mapping| mapping.get(serde_yaml::Value::String(key.to_string())))
}

fn yaml_get_any<'a>(value: &'a serde_yaml::Value, keys: &[&str]) -> Option<&'a serde_yaml::Value> {
    keys.iter().find_map(|key| yaml_get(value, key))
}

fn parse_tools_field(value: &serde_yaml::Value) -> Vec<String> {
    match value {
        serde_yaml::Value::String(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
            .map(str::to_string)
            .collect(),
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .filter_map(|item| item.as_str())
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn tool_name_to_permission(tool: &str) -> String {
    let normalized = tool.trim();
    match normalized.to_ascii_lowercase().as_str() {
        "*" => "*".to_string(),
        "read" => "read(*)".to_string(),
        "write" => "write(*)".to_string(),
        "edit" => "edit(*)".to_string(),
        "grep" => "grep(*)".to_string(),
        "glob" => "glob(*)".to_string(),
        "ls" => "ls(*)".to_string(),
        "bash" => "bash(*)".to_string(),
        "task" => "task(*)".to_string(),
        "parallel_task" | "parallel-task" => "parallel_task(*)".to_string(),
        _ if normalized.contains('(') => normalized.to_string(),
        _ => format!("{normalized}(*)"),
    }
}

fn permission_policy_from_tools(tools: &[String]) -> PermissionPolicy {
    tools.iter().fold(PermissionPolicy::new(), |policy, tool| {
        policy.allow(&tool_name_to_permission(tool))
    })
}

fn allow_only_permission_policy(tools: &[String]) -> PermissionPolicy {
    let mut policy = permission_policy_from_tools(tools);
    policy.default_decision = PermissionDecision::Deny;
    policy
}

fn add_denied_tools(mut policy: PermissionPolicy, tools: &[String]) -> PermissionPolicy {
    for tool in tools {
        policy = policy.deny(&tool_name_to_permission(tool));
    }
    policy
}

fn validate_agent_name(name: &str) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        return Err(anyhow::anyhow!("Agent name is required"));
    }
    Ok(())
}

/// Parse an agent definition from Markdown with YAML frontmatter
///
/// The frontmatter contains agent metadata, and the body becomes the prompt.
pub fn parse_agent_md(content: &str) -> anyhow::Result<AgentDefinition> {
    // Parse frontmatter (YAML between --- markers)
    let parts: Vec<&str> = content.splitn(3, "---").collect();

    if parts.len() < 3 {
        return Err(anyhow::anyhow!(
            "Invalid markdown format: missing YAML frontmatter"
        ));
    }

    let frontmatter = parts[1].trim();
    let body = parts[2].trim();

    // Parse the frontmatter as YAML. A `kind` field selects WorkerAgentSpec.
    let value: serde_yaml::Value = serde_yaml::from_str(frontmatter)
        .map_err(|e| anyhow::anyhow!("Failed to parse agent frontmatter: {}", e))?;

    if yaml_value_has_key(&value, "kind") {
        let tools = yaml_get_any(&value, &["tools", "allowedTools", "allowed_tools"])
            .map(parse_tools_field)
            .unwrap_or_default();
        let disallowed_tools = yaml_get_any(
            &value,
            &["disallowedTools", "disallowed-tools", "disallowed_tools"],
        )
        .map(parse_tools_field)
        .unwrap_or_default();
        let mut spec = parse_worker_yaml_value(value, "frontmatter")?;
        if spec.prompt.is_none() && !body.is_empty() {
            spec.prompt = Some(body.to_string());
        }
        apply_claude_style_tools_to_spec(&mut spec, &tools, &disallowed_tools);
        return Ok(spec.into_agent_definition());
    }

    let mut agent = parse_agent_yaml_value(value, "agent frontmatter")?;

    // Use body as prompt if not already set in frontmatter.
    if agent.prompt.is_none() && !body.is_empty() {
        agent.prompt = Some(body.to_string());
    }

    Ok(agent)
}

/// Load all agent definitions from a directory
///
/// Scans for *.yaml and *.md files and parses them as agent definitions.
/// Invalid files are logged and skipped.
pub fn load_agents_from_dir(dir: &Path) -> Vec<AgentDefinition> {
    let mut agents = Vec::new();
    load_agents_from_dir_inner(dir, &mut agents);
    agents
}

fn load_agents_from_dir_inner(dir: &Path, agents: &mut Vec<AgentDefinition>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        tracing::warn!("Failed to read agent directory: {}", dir.display());
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            load_agents_from_dir_inner(&path, agents);
            continue;
        }
        if !path.is_file() {
            continue;
        }

        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };

        // Read file content
        let Ok(content) = std::fs::read_to_string(&path) else {
            tracing::warn!("Failed to read agent file: {}", path.display());
            continue;
        };

        // Parse based on extension
        let result = match ext {
            "yaml" | "yml" => parse_agent_yaml(&content),
            "md" => parse_agent_md(&content),
            _ => continue,
        };

        match result {
            Ok(agent) => {
                tracing::debug!("Loaded agent '{}' from {}", agent.name, path.display());
                agents.push(agent);
            }
            Err(e) => {
                tracing::warn!("Failed to parse agent file {}: {}", path.display(), e);
            }
        }
    }
}
