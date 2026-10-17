//! Skill Tool - Invoke skills as callable tools with temporary permission grants
//!
//! A skill that does not declare `allowed-tools` returns its instructions to the
//! calling turn. The parent model applies them and the turn closes through the
//! normal completion gate. A skill that declares tools, or a tool-kind skill,
//! runs one child with that narrower policy and then returns.
//!
//! ## Usage
//!
//! ```text
//! // Agent calls: Skill("data-processor")
//! // The skill's allowed-tools are temporarily granted
//! // After execution, permissions are restored
//! ```

use crate::agent::{AgentConfig, AgentLoop};
use crate::llm::LlmClient;
use crate::permissions::{
    PermissionDecision, PermissionEnforcement, PermissionPolicy, PermissionRule,
};
use crate::skills::{Skill, SkillRegistry};
use crate::tools::{Tool, ToolContext, ToolExecutor, ToolOutput};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Weak};

/// Arguments for the Skill tool
#[derive(Debug, Serialize, Deserialize)]
pub struct SkillArgs {
    /// Name of the skill to invoke
    pub skill_name: String,
    /// Optional prompt/query to pass to the skill
    #[serde(default)]
    pub prompt: Option<String>,
}

impl SkillArgs {
    fn from_tool_args(args: &Value) -> Result<Self> {
        fn parse_from_value(value: &Value) -> Option<SkillArgs> {
            match value {
                Value::String(skill_name) => Some(SkillArgs {
                    skill_name: skill_name.clone(),
                    prompt: None,
                }),
                Value::Object(map) => {
                    if let Some(skill_name) = map
                        .get("skill_name")
                        .or_else(|| map.get("skillName"))
                        .or_else(|| map.get("name"))
                        .and_then(|v| v.as_str())
                    {
                        let prompt = map
                            .get("prompt")
                            .or_else(|| map.get("query"))
                            .and_then(|v| v.as_str())
                            .map(ToOwned::to_owned);
                        return Some(SkillArgs {
                            skill_name: skill_name.to_string(),
                            prompt,
                        });
                    }

                    if let Some(nested) = map.get("input").or_else(|| map.get("arguments")) {
                        if let Some(parsed) = parse_from_value(nested) {
                            return Some(parsed);
                        }
                    }

                    None
                }
                _ => None,
            }
        }

        parse_from_value(args).ok_or_else(|| anyhow!("missing field 'skill_name'"))
    }
}

/// Arguments for the search_skills tool
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchSkillsArgs {
    /// Query describing the desired skill
    pub query: String,
    /// Maximum number of results to return
    #[serde(default)]
    pub limit: Option<usize>,
}

impl SearchSkillsArgs {
    fn from_tool_args(args: &Value) -> Result<Self> {
        match args {
            Value::String(query) => Ok(Self {
                query: query.clone(),
                limit: None,
            }),
            Value::Object(map) => {
                let query = map
                    .get("query")
                    .or_else(|| map.get("q"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("missing field 'query'"))?
                    .to_string();
                let limit = map
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                Ok(Self { query, limit })
            }
            _ => Err(anyhow!(
                "search_skills expects an object with a 'query' field"
            )),
        }
    }
}

fn search_skills_lead(count: usize, language: Option<&str>) -> String {
    match language {
        Some("zh-CN") | Some("zh") => {
            format!("找到 {count} 个匹配的技能。请用 Skill 调用，并传入它的 skill_name。")
        }
        Some("ja") => {
            format!(
                "{count} 件のスキルが見つかりました。Skill に skill_name を渡して呼び出してください。"
            )
        }
        Some("ko") => {
            format!("일치하는 스킬 {count}개를 찾았습니다. Skill 에 skill_name 을 넘겨 호출하세요.")
        }
        _ => {
            format!("Found {count} matching skill(s). Invoke one with Skill using its skill_name.")
        }
    }
}

fn search_skills_empty(language: Option<&str>) -> String {
    match language {
        Some("zh-CN") | Some("zh") => "没有匹配的技能。请继续使用现有工具。".to_string(),
        Some("ja") => "一致するスキルはありません。既存のツールで続けてください。".to_string(),
        Some("ko") => "일치하는 스킬이 없습니다. 기존 도구로 계속하세요.".to_string(),
        _ => "No matching skills found. Continue with the core tools.".to_string(),
    }
}

/// Search available skills without injecting all skill descriptions into context.
pub struct SearchSkillsTool {
    skill_registry: Arc<SkillRegistry>,
}

impl SearchSkillsTool {
    pub fn new(skill_registry: Arc<SkillRegistry>) -> Self {
        Self { skill_registry }
    }
}

#[async_trait]
impl Tool for SearchSkillsTool {
    fn name(&self) -> &str {
        "search_skills"
    }

    fn description(&self) -> &str {
        "Search available skills by name, tag, description, or content. \
Use this before invoking Skill when specialized instructions may help."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Short search query for the skill you need."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "description": "Maximum number of skills to return. Defaults to 5."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args = SearchSkillsArgs::from_tool_args(args)?;
        // A package can land after the catalog prompt was built.
        self.skill_registry.refresh_watched_dirs();
        let limit = args.limit.unwrap_or(5).clamp(1, 20);
        let matches = self.skill_registry.search(&args.query, limit);
        let language = ctx.reply_language();

        if matches.is_empty() {
            return Ok(ToolOutput::success(search_skills_empty(
                language.as_deref(),
            )));
        }

        let mut lines = vec![search_skills_lead(matches.len(), language.as_deref())];
        let metadata: Vec<_> = matches
            .iter()
            .map(|skill| {
                let kind = format!("{:?}", skill.kind).to_lowercase();
                let allowed_tools = skill.allowed_tools.as_deref().unwrap_or("not specified");
                lines.push(format!(
                    "- {} ({kind}): {} Allowed tools: {}.",
                    skill.name, skill.description, allowed_tools
                ));
                serde_json::json!({
                    "name": skill.name,
                    "description": skill.description,
                    "kind": kind,
                    "tags": skill.tags,
                    "allowed_tools": skill.allowed_tools,
                })
            })
            .collect();

        Ok(ToolOutput {
            content: lines.join("\n"),
            success: true,
            metadata: Some(serde_json::json!({ "skills": metadata })),
            images: Vec::new(),
            trust: crate::tools::ToolResultTrustV1::WorkspaceData,
            error_kind: None,
        })
    }
}

/// Skill tool - invokes skills with temporary permission grants
pub struct SkillTool {
    skill_registry: Arc<SkillRegistry>,
    llm_client: Arc<dyn LlmClient>,
    tool_executor: SkillToolExecutor,
    base_config: AgentConfig,
}

enum SkillToolExecutor {
    #[cfg(test)]
    Standalone(Arc<ToolExecutor>),
    RegistryBound(Weak<ToolExecutor>),
}

impl SkillToolExecutor {
    fn resolve(&self) -> Option<Arc<ToolExecutor>> {
        match self {
            #[cfg(test)]
            Self::Standalone(executor) => Some(executor.clone()),
            Self::RegistryBound(executor) => executor.upgrade(),
        }
    }
}

impl SkillTool {
    #[cfg(test)]
    pub(crate) fn new(
        skill_registry: Arc<SkillRegistry>,
        llm_client: Arc<dyn LlmClient>,
        tool_executor: Arc<ToolExecutor>,
        base_config: AgentConfig,
    ) -> Self {
        Self {
            skill_registry,
            llm_client,
            tool_executor: SkillToolExecutor::Standalone(tool_executor),
            base_config,
        }
    }

    /// Construct the Skill tool installed inside the supplied executor's own
    /// registry. The back-reference must be weak or registration would create
    /// an executor-registry-tool ownership cycle.
    pub(crate) fn new_registry_bound(
        skill_registry: Arc<SkillRegistry>,
        llm_client: Arc<dyn LlmClient>,
        tool_executor: Arc<ToolExecutor>,
        base_config: AgentConfig,
    ) -> Self {
        Self {
            skill_registry,
            llm_client,
            tool_executor: SkillToolExecutor::RegistryBound(Arc::downgrade(&tool_executor)),
            base_config,
        }
    }

    fn invocable_skill(&self, name: &str) -> Result<Arc<Skill>> {
        // A withheld name has no body in this run. Say so before a directory
        // refresh can load either file.
        if let Some(reason) = self.skill_registry.withheld_reason(name) {
            anyhow::bail!("{reason}");
        }

        // A package can land after the catalog prompt was built. Unchanged
        // directories are a stat. The missing-skill error stays when the
        // name is still absent after that refresh.
        self.skill_registry.refresh_watched_dirs();
        let skill = self
            .skill_registry
            .get(name)
            .ok_or_else(|| anyhow!("Skill '{}' not found", name))?;

        if skill.disable_model_invocation {
            anyhow::bail!(
                "Skill '{}' disables model invocation and cannot be invoked via the Skill tool",
                skill.name
            );
        }
        Ok(skill)
    }

    /// Create a temporary permission policy that grants the skill's allowed-tools
    fn create_skill_permission_policy(skill: &Skill) -> PermissionPolicy {
        let permissions = skill.parse_allowed_tools();

        if permissions.is_empty() {
            tracing::warn!(
                skill = %skill.name,
                "Skill has no allowed-tools grants; Skill invocation remains fail-secure and will deny tool use"
            );
            return PermissionPolicy {
                deny: Vec::new(),
                allow: Vec::new(),
                ask: Vec::new(),
                default_decision: PermissionDecision::Deny,
                enforcement: PermissionEnforcement::Enforced,
            };
        }

        // Convert skill permissions to PermissionRules
        let mut allow_rules = Vec::new();
        for perm in permissions {
            // Create a rule string in the format "Tool(pattern)"
            let rule_str = if perm.pattern == "*" {
                perm.tool.clone()
            } else {
                format!("{}({})", perm.tool, perm.pattern)
            };
            allow_rules.push(PermissionRule::new(&rule_str));
        }

        PermissionPolicy {
            deny: Vec::new(),
            allow: allow_rules,
            ask: Vec::new(),
            default_decision: PermissionDecision::Deny, // Deny by default - only allow what skill specifies
            enforcement: PermissionEnforcement::Enforced,
        }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn description(&self) -> &str {
        "Load a skill for this turn. \
Use a JSON object with the canonical shape {\"skill_name\":\"<skill-name>\",\"prompt\":\"<optional prompt>\"}. \
Always send the skill name in the 'skill_name' field. Do not use aliases such as 'name' or 'skillName', and do not wrap the payload in 'input' or 'arguments'. \
A skill with no allowed-tools returns its instructions; follow them here and answer the user. \
A skill that declares allowed-tools runs inside this call with only those tools, then returns."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "Required. Canonical skill identifier to invoke. Always provide this exact field name: 'skill_name'."
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional prompt or query to pass to the skill after it is loaded."
                }
            },
            "required": ["skill_name"],
            "examples": [
                {
                    "skill_name": "code-review"
                },
                {
                    "skill_name": "code-review",
                    "prompt": "Review this patch for correctness and regressions."
                }
            ]
        })
    }

    // A refusal here is final and effect-free, so admission reports it as a
    // denial instead of an execution error with an unknown outcome.
    fn validate_execution_available(&self, args: &Value, _ctx: &ToolContext) -> Result<()> {
        let args = SkillArgs::from_tool_args(args)?;
        self.invocable_skill(&args.skill_name).map(|_| ())
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args = SkillArgs::from_tool_args(args)?;
        let skill = self.invocable_skill(&args.skill_name)?;

        // An instruction or persona skill with no allowed-tools is guidance for
        // the calling turn. A nested agent would hide its model call from this
        // turn, leave the tool on "running", and open a second planner under an
        // active goal. The parent applies the instructions and the existing
        // completion gate decides when the turn is done.
        if skill.inherits_session_tools() {
            return Ok(instruction_skill_output(&skill, args.prompt.as_deref()));
        }

        // Create a modified config with the skill's permissions.
        let mut skill_config = self.base_config.clone();
        skill_config.durable_source = None;
        if ctx.has_run_governance() {
            skill_config.work_policy = ctx.run_work_policy.clone();
            skill_config.permission_checker = ctx.run_permission_checker();
            skill_config.confirmation_manager = ctx.run_confirmation_manager();
        }

        // The parent already chose this skill. A second planner, including one
        // forced on by an active goal, spends another model call before the
        // skill body runs and can stall this tool. Task children use the same
        // bound.
        skill_config.planning_mode = crate::prompts::PlanningMode::Disabled;

        // A declared list, or a tool skill, narrows the child run. The host
        // checker stays in the composition so TUI denials remain authoritative.
        let skill_permission_policy = Self::create_skill_permission_policy(&skill);
        let parent_checker = skill_config.permission_checker.take();
        let parent_policy = skill_config.permission_policy.clone();
        skill_config.permission_checker = Some(crate::child_run::compose_permission_checker(
            Arc::new(skill_permission_policy.clone()),
            Some(skill_permission_policy.clone()),
            parent_checker,
            parent_policy,
        ));
        skill_config.permission_policy = Some(skill_permission_policy);
        skill_config.enforce_active_skill_tool_restrictions = true;

        // Create a temporary skill registry with only this skill
        let temp_registry = Arc::new(SkillRegistry::new());
        temp_registry.register(skill.clone())?;
        skill_config.skill_registry = Some(temp_registry);

        if skill_config.work_policy.is_some() {
            skill_config.prompt_slots.guidelines = None;
            skill_config.prompt_slots.extra = None;
        }
        // Build the system prompt with skill content
        skill_config.prompt_slots.role = Some(format!(
            "You are executing the '{}' skill.\n\n{}\n\n{}",
            skill.name, skill.description, skill.content
        ));

        let capability_subtask = ctx
            .capability_context()
            .map(|context| context.admit_subtask(format!("skill-{}", uuid::Uuid::new_v4()), false))
            .transpose()?;
        let cancellation = capability_subtask.as_ref().map_or_else(
            || ctx.cancellation_token(),
            crate::capability::AgentCapabilitySubtask::cancellation,
        );

        // Create the child Agent loop inside the Skill's spatial Subtask. Its
        // provider/tool iterations will recursively create temporal Turns.
        let tool_executor = self
            .tool_executor
            .resolve()
            .ok_or_else(|| anyhow!("Skill tool executor is closed"))?;
        let mut agent_loop = AgentLoop::new(
            self.llm_client.clone(),
            tool_executor,
            ctx.clone(),
            skill_config,
        );
        if let Some(subtask) = capability_subtask.as_ref() {
            agent_loop = agent_loop.with_capability_runtime(subtask.runtime());
        }

        // Execute the skill with the prompt
        let prompt = args
            .prompt
            .unwrap_or_else(|| format!("Execute the '{}' skill", skill.name));

        // The skill remains inside the owning Run's cancellation and budget
        // scope. Its Subtask token is derived from the invoking Turn, so parent
        // cancellation reaches every provider/tool call without allowing the
        // child to cancel its parent.
        let watch = crate::porcelain::Watch::start(&ctx.workspace).await;
        let child_id = format!("skill-run-{}", uuid::Uuid::new_v4());
        let _child_claims =
            crate::external_observation::ChildWriteClaimGuard::new(child_id.clone());
        let execution = crate::agent_api::ChildRun {
            agent: agent_loop,
            child_id: child_id.clone(),
            budget_session: ctx.session_id.clone(),
            events: None,
            cancel: cancellation.clone(),
        }
        .run(&prompt)
        .await;
        let cancelled = cancellation.is_cancelled();
        let close = close_skill_capability_subtask(capability_subtask.as_ref()).await;
        let changed = watch.finish(&ctx.workspace).await;
        crate::external_observation::settle_child_write_claims(
            ctx.session_id.as_deref(),
            &child_id,
            ctx.workspace.as_path(),
            &changed,
        );
        let result = match (execution, close) {
            (Ok(result), Ok(())) => result,
            (Ok(_), Err(close_error)) => {
                return skill_effect_or_error(&skill.name, 0, &close_error.to_string(), &changed);
            }
            (Err(error), Ok(())) => {
                return skill_effect_or_error(&skill.name, 0, &error.to_string(), &changed);
            }
            (Err(error), Err(close_error)) => {
                tracing::warn!(
                    error = %close_error,
                    "Capability Skill Subtask close also failed after execution failure"
                );
                return skill_effect_or_error(&skill.name, 0, &error.to_string(), &changed);
            }
        };
        if cancelled {
            return skill_effect_or_error(
                &skill.name,
                result.tool_calls_count,
                &format!("Skill '{}' cancelled by caller", skill.name),
                &changed,
            );
        }

        // A child narrative success is not a parent success if the skill
        // wrote files. Publish the workspace delta so the parent gate opens.
        Ok(skill_output(
            &skill.name,
            result.tool_calls_count,
            result.text,
            Some(result.usage),
            &changed,
        ))
    }
}

fn instruction_skill_output(skill: &Skill, prompt: Option<&str>) -> ToolOutput {
    let request = prompt
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("Follow the skill.");
    let content = format!(
        "Skill `{name}` is active for this turn. Follow its instructions and answer here. Do not invoke `{name}` again for this request.\n\n# {name}\n\n{description}\n\n{body}\n\nUser request:\n{request}",
        name = skill.name,
        description = skill.description.trim(),
        body = skill.content.trim(),
        request = request,
    );
    ToolOutput {
        content,
        success: true,
        metadata: Some(serde_json::json!({
            "skill_name": skill.name,
            "tool_calls": 0,
            "delivery": "instructions",
        })),
        images: Vec::new(),
        trust: crate::tools::ToolResultTrustV1::WorkspaceData,
        error_kind: None,
    }
}

fn skill_output(
    skill_name: &str,
    tool_calls: usize,
    content: String,
    usage: Option<crate::llm::TokenUsage>,
    changed: &[String],
) -> ToolOutput {
    let mut metadata = serde_json::json!({
        "skill_name": skill_name,
        "tool_calls": tool_calls,
    });
    if let Some(usage) = usage {
        if let Some(object) = metadata.as_object_mut() {
            object.insert("usage".to_string(), serde_json::json!(usage));
        }
    }
    let mut metadata = Some(metadata);
    crate::porcelain::attach(&mut metadata, changed);
    ToolOutput {
        content,
        success: true,
        metadata,
        images: Vec::new(),
        trust: crate::tools::ToolResultTrustV1::WorkspaceData,
        error_kind: None,
    }
}

fn skill_effect_or_error(
    skill_name: &str,
    tool_calls: usize,
    error: &str,
    changed: &[String],
) -> Result<ToolOutput> {
    if changed.is_empty() {
        anyhow::bail!("{error}");
    }
    Ok(skill_output(
        skill_name,
        tool_calls,
        error.to_string(),
        None,
        changed,
    ))
}

async fn close_skill_capability_subtask(
    subtask: Option<&crate::capability::AgentCapabilitySubtask>,
) -> Result<()> {
    let Some(subtask) = subtask else {
        return Ok(());
    };
    let report = subtask.close().await?;
    if !report.is_clean() {
        anyhow::bail!(
            "Capability Skill Subtask close was incomplete (tasks failed: {}, tasks timed out: {}, child scopes failed: {}, child scopes timed out: {}, effects failed: {}, effects timed out: {})",
            report.tasks_failed,
            report.tasks_timed_out,
            report.child_scopes_failed,
            report.child_scopes_timed_out,
            report.effects_failed,
            report.effects_timed_out,
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests;
