use a3s_code_core::orchestrator::{AgentOrchestrator, OrchestratorEvent, SubAgentConfig};
use a3s_code_core::Agent;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let workspace = temp.path().join("workspace");
    let skills_dir = temp.path().join("skills");
    std::fs::create_dir_all(&workspace)?;
    std::fs::create_dir_all(&skills_dir)?;

    let skill_path = skills_dir.join("scoring-video-adapter.md");
    std::fs::write(
        &skill_path,
        r#"---
name: scoring-video-adapter
description: "Subagent skill invocation smoke test"
kind: tool
---
# Scoring Video Adapter

You are a smoke test skill.
Reply with exactly: SKILL_OK
"#,
    )?;

    let config_path = temp.path().join("agent_kimi_env.hcl");
    std::fs::write(
        &config_path,
        r#"default_model = "openai/kimi-k2.5"

providers {
  name     = "openai"
  api_key  = env("KIMI_API_KEY")
  base_url = env("KIMI_BASE_URL")

  models {
    id   = "kimi-k2.5"
    name = "KIMI K2.5"
  }
}

storage_backend = "memory"
max_tool_rounds = 8
"#,
    )?;

    let agent = Arc::new(Agent::create(config_path.display().to_string()).await?);
    let orchestrator = AgentOrchestrator::from_agent(agent);
    let handle = orchestrator
        .spawn_subagent(
            SubAgentConfig::new(
                "general",
                "You must call the Skill tool for scoring-video-adapter. \
                 Use the tool rather than answering directly. \
                 Set skill_name to scoring-video-adapter. \
                 After the tool returns, output only the final answer.",
            )
            .with_description("Kimi subagent skill smoke test")
            .with_workspace(workspace.display().to_string())
            .with_skill_dirs(vec![skills_dir.display().to_string()])
            .with_permissive(true)
            .with_max_steps(6),
        )
        .await?;

    let mut events = handle.events();
    let event_task = tokio::spawn(async move {
        let mut saw_skill_tool = false;
        let mut saw_skill_ok = false;

        while let Some(event) =
            tokio::time::timeout(std::time::Duration::from_secs(60), events.recv()).await?
        {
            match event {
                OrchestratorEvent::ToolExecutionStarted { tool_name, .. } => {
                    println!("EVENT tool_start {}", tool_name);
                    if tool_name == "Skill" {
                        saw_skill_tool = true;
                    }
                }
                OrchestratorEvent::ToolExecutionCompleted {
                    tool_name, result, ..
                } => {
                    println!(
                        "EVENT tool_end {} => {}",
                        tool_name,
                        result.replace('\n', " ")
                    );
                    if tool_name == "Skill" && result.contains("SKILL_OK") {
                        saw_skill_ok = true;
                    }
                }
                OrchestratorEvent::SubAgentCompleted {
                    success, output, ..
                } => {
                    println!(
                        "EVENT subagent_completed success={} output={}",
                        success,
                        output.replace('\n', " ")
                    );
                    break;
                }
                _ => {}
            }
        }

        Ok::<(bool, bool), anyhow::Error>((saw_skill_tool, saw_skill_ok))
    });

    let output = tokio::time::timeout(std::time::Duration::from_secs(90), handle.wait()).await??;
    let (saw_skill_tool, saw_skill_ok) = event_task.await??;

    println!("FINAL_OUTPUT={}", output.replace('\n', " "));
    println!("SAW_SKILL_TOOL={}", saw_skill_tool);
    println!("SAW_SKILL_OK={}", saw_skill_ok);

    if !saw_skill_tool {
        anyhow::bail!("Skill tool was not called");
    }
    if !output.contains("SKILL_OK") {
        anyhow::bail!("Final output did not contain SKILL_OK: {}", output);
    }

    Ok(())
}
