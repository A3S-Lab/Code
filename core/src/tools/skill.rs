//! Skill Tool - Invoke skills as callable tools with temporary permission grants
//!
//! This tool allows agents to invoke skills as first-class tools, with the skill's
//! allowed-tools temporarily granted during execution. This enforces skill-based
//! access patterns and prevents agents from bypassing skills to directly access
//! underlying tools.
//!
//! ## Usage
//!
//! ```rust
//! // Agent calls: Skill("data-processor")
//! // The skill's allowed-tools are temporarily granted
//! // After execution, permissions are restored
//! ```

use crate::agent::{AgentConfig, AgentLoop};
use crate::llm::LlmClient;
use crate::permissions::{PermissionDecision, PermissionPolicy, PermissionRule};
use crate::skills::{Skill, SkillRegistry};
use crate::tools::{Tool, ToolContext, ToolExecutor, ToolOutput};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// Arguments for the Skill tool
#[derive(Debug, Serialize, Deserialize)]
pub struct SkillArgs {
    /// Name of the skill to invoke
    pub skill_name: String,
    /// Optional prompt/query to pass to the skill
    #[serde(default)]
    pub prompt: Option<String>,
}

/// Skill tool - invokes skills with temporary permission grants
pub struct SkillTool {
    skill_registry: Arc<SkillRegistry>,
    llm_client: Arc<dyn LlmClient>,
    tool_executor: Arc<ToolExecutor>,
    base_config: AgentConfig,
}

impl SkillTool {
    pub fn new(
        skill_registry: Arc<SkillRegistry>,
        llm_client: Arc<dyn LlmClient>,
        tool_executor: Arc<ToolExecutor>,
        base_config: AgentConfig,
    ) -> Self {
        Self {
            skill_registry,
            llm_client,
            tool_executor,
            base_config,
        }
    }

    /// Create a temporary permission policy that grants the skill's allowed-tools
    fn create_skill_permission_policy(skill: &Skill) -> PermissionPolicy {
        let permissions = skill.parse_allowed_tools();

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
            enabled: true,
        }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "Skill"
    }

    fn description(&self) -> &str {
        "Invoke a skill with temporary permission grants. The skill's allowed-tools are granted during execution and revoked after completion."
    }

    fn parameters(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "Name of the skill to invoke"
                },
                "prompt": {
                    "type": "string",
                    "description": "Optional prompt or query to pass to the skill"
                }
            },
            "required": ["skill_name"]
        })
    }

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args: SkillArgs = serde_json::from_value(args.clone())?;

        // Get the skill
        let skill = self
            .skill_registry
            .get(&args.skill_name)
            .ok_or_else(|| anyhow!("Skill '{}' not found", args.skill_name))?;

        // Create temporary permission policy with skill's allowed-tools
        let skill_permission_policy = Self::create_skill_permission_policy(&skill);

        // Create a modified config with the skill's permissions
        let mut skill_config = self.base_config.clone();

        // Set the skill's permission policy as the permission checker
        skill_config.permission_checker = Some(Arc::new(skill_permission_policy));

        // Create a temporary skill registry with only this skill
        let temp_registry = Arc::new(SkillRegistry::new());
        temp_registry.register(skill.clone())?;
        skill_config.skill_registry = Some(temp_registry);

        // Build the system prompt with skill content
        skill_config.prompt_slots.role = Some(format!(
            "You are executing the '{}' skill.\n\n{}\n\n{}",
            skill.name, skill.description, skill.content
        ));

        // Create agent loop with skill permissions
        let agent_loop = AgentLoop::new(
            self.llm_client.clone(),
            self.tool_executor.clone(),
            ctx.clone(),
            skill_config,
        );

        // Execute the skill with the prompt
        let prompt = args
            .prompt
            .unwrap_or_else(|| format!("Execute the '{}' skill", skill.name));

        // Execute the agent loop with skill permissions
        let result = agent_loop.execute(&[], &prompt, None).await?;

        // Return the final response as tool output
        Ok(ToolOutput {
            content: result.text,
            success: true,
            metadata: Some(serde_json::json!({
                "skill_name": skill.name,
                "tool_calls": result.tool_calls_count,
                "usage": result.usage,
            })),
            images: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillKind;

    #[test]
    fn test_skill_permission_policy() {
        let skill = Skill {
            name: "test-skill".to_string(),
            description: "Test".to_string(),
            allowed_tools: Some("read(*), grep(*)".to_string()),
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: String::new(),
            tags: Vec::new(),
            version: None,
        };

        let policy = SkillTool::create_skill_permission_policy(&skill);

        // Should allow tools in allowed-tools
        assert_eq!(
            policy.check("read", &serde_json::json!({})),
            PermissionDecision::Allow
        );
        assert_eq!(
            policy.check("grep", &serde_json::json!({})),
            PermissionDecision::Allow
        );

        // Should deny tools not in allowed-tools
        assert_eq!(
            policy.check("write", &serde_json::json!({})),
            PermissionDecision::Deny
        );
    }
}
