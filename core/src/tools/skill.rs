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
        "Invoke a skill with temporary permission grants. \
Use a JSON object with the canonical shape {\"skill_name\":\"<skill-name>\",\"prompt\":\"<optional prompt>\"}. \
Always send the skill name in the 'skill_name' field. Do not use aliases such as 'name' or 'skillName', and do not wrap the payload in 'input' or 'arguments'. \
The skill's allowed-tools are granted during execution and revoked after completion."
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

    async fn execute(&self, args: &Value, ctx: &ToolContext) -> Result<ToolOutput> {
        let args = SkillArgs::from_tool_args(args)?;

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
    use crate::llm::{
        ContentBlock, LlmClient, LlmResponse, Message, StreamEvent, TokenUsage, ToolDefinition,
    };
    use crate::skills::SkillKind;
    use crate::tools::ToolContext;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tokio::sync::mpsc;

    struct MockLlmClient {
        responses: Mutex<Vec<LlmResponse>>,
    }

    impl MockLlmClient {
        fn new(responses: Vec<LlmResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }

        fn text_response(text: &str) -> LlmResponse {
            LlmResponse {
                message: Message {
                    role: "assistant".to_string(),
                    content: vec![ContentBlock::Text {
                        text: text.to_string(),
                    }],
                    reasoning_content: None,
                },
                usage: TokenUsage {
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    total_tokens: 15,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                },
                stop_reason: Some("end_turn".to_string()),
                meta: None,
            }
        }
    }

    #[async_trait]
    impl LlmClient for MockLlmClient {
        async fn complete(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> Result<LlmResponse> {
            let mut responses = self.responses.lock().unwrap();
            if responses.is_empty() {
                anyhow::bail!("No more mock responses available");
            }
            Ok(responses.remove(0))
        }

        async fn complete_streaming(
            &self,
            _messages: &[Message],
            _system: Option<&str>,
            _tools: &[ToolDefinition],
        ) -> Result<mpsc::Receiver<StreamEvent>> {
            anyhow::bail!("streaming not used in SkillTool tests")
        }
    }

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

    #[test]
    fn test_skill_args_accepts_documented_shape() {
        let args =
            SkillArgs::from_tool_args(&serde_json::json!({"skill_name": "code-review"})).unwrap();
        assert_eq!(args.skill_name, "code-review");
        assert_eq!(args.prompt, None);
    }

    #[test]
    fn test_skill_args_accepts_common_aliases_and_wrappers() {
        let camel =
            SkillArgs::from_tool_args(&serde_json::json!({"skillName": "code-review"})).unwrap();
        assert_eq!(camel.skill_name, "code-review");

        let name = SkillArgs::from_tool_args(&serde_json::json!({
            "name": "code-review",
            "query": "review this patch"
        }))
        .unwrap();
        assert_eq!(name.skill_name, "code-review");
        assert_eq!(name.prompt.as_deref(), Some("review this patch"));

        let nested = SkillArgs::from_tool_args(&serde_json::json!({
            "input": {
                "skill_name": "code-review",
                "prompt": "review this patch"
            }
        }))
        .unwrap();
        assert_eq!(nested.skill_name, "code-review");
        assert_eq!(nested.prompt.as_deref(), Some("review this patch"));

        let direct = SkillArgs::from_tool_args(&serde_json::json!("code-review")).unwrap();
        assert_eq!(direct.skill_name, "code-review");
    }

    #[test]
    fn test_skill_args_missing_skill_name_errors() {
        let err =
            SkillArgs::from_tool_args(&serde_json::json!({"prompt": "do something"})).unwrap_err();
        assert!(err.to_string().contains("missing field 'skill_name'"));
    }

    #[test]
    fn test_skill_tool_schema_enforces_canonical_shape() {
        let registry = Arc::new(SkillRegistry::new());
        let llm = Arc::new(MockLlmClient::new(vec![]));
        let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let tool = SkillTool::new(registry, llm, executor, AgentConfig::default());

        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert_eq!(params["additionalProperties"], serde_json::json!(false));
        assert_eq!(params["required"], serde_json::json!(["skill_name"]));

        let examples = params["examples"].as_array().unwrap();
        assert_eq!(examples[0]["skill_name"], "code-review");
        assert!(examples[0].get("name").is_none());
        assert!(examples[0].get("skillName").is_none());
    }

    #[tokio::test]
    async fn test_skill_tool_execute_runs_skill_and_returns_metadata() {
        use crate::prompts::PlanningMode;

        let registry = Arc::new(SkillRegistry::new());
        registry.register_unchecked(Arc::new(Skill {
            name: "test-skill".to_string(),
            description: "Run a focused skill".to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: "Reply with the skill result.".to_string(),
            tags: vec!["focus".to_string()],
            version: None,
        }));

        let llm = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "skill completed",
        )]));
        let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        // Disable planning mode since the mock only has one response
        let mut config = AgentConfig::default();
        config.planning_mode = PlanningMode::Disabled;
        let tool = SkillTool::new(registry, llm, executor, config);

        let result = tool
            .execute(
                &serde_json::json!({
                    "skill_name": "test-skill",
                    "prompt": "run the skill"
                }),
                &ToolContext::new(PathBuf::from("/tmp")),
            )
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.content, "skill completed");
        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["skill_name"], "test-skill");
        assert_eq!(metadata["tool_calls"], 0);
    }

    #[tokio::test]
    async fn test_skill_tool_execute_errors_for_unknown_skill() {
        let llm = Arc::new(MockLlmClient::new(vec![MockLlmClient::text_response(
            "unused",
        )]));
        let executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
        let tool = SkillTool::new(
            Arc::new(SkillRegistry::new()),
            llm,
            executor,
            AgentConfig::default(),
        );

        let err = tool
            .execute(
                &serde_json::json!({"skill_name": "missing-skill"}),
                &ToolContext::new(PathBuf::from("/tmp")),
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Skill 'missing-skill' not found"));
    }
}
