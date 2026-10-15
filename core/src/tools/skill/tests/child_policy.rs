#![cfg(test)]

use super::*;

struct PolicyClient(Mutex<Vec<String>>);
#[async_trait]
impl LlmClient for PolicyClient {
    async fn complete(
        &self,
        _messages: &[Message],
        system: Option<&str>,
        _tools: &[ToolDefinition],
    ) -> Result<LlmResponse> {
        self.0
            .lock()
            .unwrap()
            .push(system.unwrap_or_default().to_owned());
        Ok(MockLlmClient::text_response("ready"))
    }
    async fn complete_streaming(
        &self,
        _messages: &[Message],
        _system: Option<&str>,
        _tools: &[ToolDefinition],
        _cancel: CancellationToken,
    ) -> Result<mpsc::Receiver<StreamEvent>> {
        anyhow::bail!("use complete")
    }
}

#[tokio::test]
async fn child_policy_skill_retains_role_and_locale_without_parent_prompt_or_stale_policy() {
    let workspace = tempfile::tempdir().unwrap();
    let registry = Arc::new(SkillRegistry::new());
    registry.register_unchecked(Arc::new(Skill {
        name: "policy-skill".into(),
        description: "Scoped reading".into(),
        allowed_tools: Some("read(*)".into()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "SKILL-ROLE-PRESERVED".into(),
        tags: Vec::new(),
        version: None,
    }));
    let client = Arc::new(PolicyClient(Mutex::new(Vec::new())));
    let config = AgentConfig {
        work_policy: Some(crate::WorkPolicyV1::new("stale", 1, "STALE-REGISTRATION").unwrap()),
        prompt_slots: crate::SystemPromptSlots {
            role: Some("PARENT-ONLY-ROLE".into()),
            guidelines: Some("PARENT-ONLY-GUIDELINES".into()),
            extra: Some("PARENT-ONLY-EXTRA".into()),
            output_language: Some("zh-CN".into()),
            response_style: Some("RESPONSE-STYLE-PRESERVED".into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let tool = SkillTool::new(
        registry,
        client.clone(),
        Arc::new(ToolExecutor::new(workspace.path().display().to_string())),
        config,
    );
    let mut context =
        ToolContext::new(workspace.path().to_path_buf()).with_run_governance(None, None);
    let policy = crate::WorkPolicyV1::new("current", 1, "CURRENT-RUN-POLICY").unwrap();
    context.run_work_policy = Some(policy.clone());
    let output = tool
        .execute(
            &serde_json::json!({"skill_name":"policy-skill","prompt":"Reply ready."}),
            &context,
        )
        .await
        .unwrap();
    assert!(output.success, "{}", output.content);
    let systems = client.0.lock().unwrap();
    assert_eq!(systems.len(), 1);
    assert_eq!(systems[0].matches(policy.instructions()).count(), 1);
    for marker in ["SKILL-ROLE-PRESERVED", "RESPONSE-STYLE-PRESERVED", "zh-CN"] {
        assert!(systems[0].contains(marker), "missing {marker}");
    }
    for marker in ["PARENT-ONLY", "STALE-REGISTRATION"] {
        assert!(!systems[0].contains(marker));
    }
}
