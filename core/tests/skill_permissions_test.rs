//! Test skill-based tool permission enforcement

use a3s_code_core::{
    agent::ToolCommand,
    queue::SessionCommand,
    skills::{Skill, SkillKind, SkillRegistry},
    tools::{ToolContext, ToolExecutor},
};
use std::sync::Arc;

#[tokio::test]
async fn test_skill_tool_permission_allowed() {
    // Create a skill that allows only read and grep
    let skill = Skill {
        name: "test-skill".to_string(),
        description: "Test skill".to_string(),
        allowed_tools: Some("read(*), grep(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Test content".to_string(),
        tags: vec![],
        version: None,
    };

    let registry = SkillRegistry::new();
    registry.register(Arc::new(skill)).unwrap();

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let tool_context = ToolContext::new("/tmp".into());

    // Test allowed tool (read)
    let cmd = ToolCommand::new(
        tool_executor.clone(),
        "read".to_string(),
        serde_json::json!({"file_path": "test.txt"}),
        tool_context.clone(),
        Some(Arc::new(registry)),
    );

    // This should not fail due to skill permissions
    // (it might fail for other reasons like file not found, but that's OK)
    let result = cmd.execute().await;
    if let Err(e) = result {
        let err_msg = e.to_string();
        // Make sure it's not a skill permission error
        assert!(!err_msg.contains("not allowed by any active skill"));
    }
}

#[tokio::test]
async fn test_skill_tool_permission_denied() {
    // Create a skill that allows only read and grep
    let skill = Skill {
        name: "test-skill".to_string(),
        description: "Test skill".to_string(),
        allowed_tools: Some("read(*), grep(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Test content".to_string(),
        tags: vec![],
        version: None,
    };

    let registry = SkillRegistry::new();
    registry.register(Arc::new(skill)).unwrap();

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let tool_context = ToolContext::new("/tmp".into());

    // Test disallowed tool (write)
    let cmd = ToolCommand::new(
        tool_executor.clone(),
        "write".to_string(),
        serde_json::json!({"file_path": "test.txt", "content": "hello"}),
        tool_context.clone(),
        Some(Arc::new(registry)),
    );

    // This should fail with permission error
    let result = cmd.execute().await;
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("not allowed by any active skill"));
    assert!(err_msg.contains("write"));
}

#[tokio::test]
async fn test_skill_no_restrictions() {
    // Create a skill without tool restrictions
    let skill = Skill {
        name: "test-skill".to_string(),
        description: "Test skill".to_string(),
        allowed_tools: None, // No restrictions
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Test content".to_string(),
        tags: vec![],
        version: None,
    };

    let registry = SkillRegistry::new();
    registry.register(Arc::new(skill)).unwrap();

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let tool_context = ToolContext::new("/tmp".into());

    // Test any tool (write) - should be allowed because no restrictions
    let cmd = ToolCommand::new(
        tool_executor.clone(),
        "write".to_string(),
        serde_json::json!({"file_path": "/tmp/test.txt", "content": "hello"}),
        tool_context.clone(),
        Some(Arc::new(registry)),
    );

    // This should succeed (no restrictions)
    let result = cmd.execute().await;
    // It might fail for other reasons (permissions, etc.) but not skill restrictions
    if let Err(e) = result {
        let err_msg = e.to_string();
        assert!(!err_msg.contains("not allowed by any active skill"));
    }
}

#[tokio::test]
async fn test_no_skill_registry() {
    // No skill registry - all tools should be allowed
    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let tool_context = ToolContext::new("/tmp".into());

    let cmd = ToolCommand::new(
        tool_executor.clone(),
        "write".to_string(),
        serde_json::json!({"file_path": "/tmp/test.txt", "content": "hello"}),
        tool_context.clone(),
        None, // No registry
    );

    // This should not fail due to skill restrictions
    let result = cmd.execute().await;
    if let Err(e) = result {
        let err_msg = e.to_string();
        assert!(!err_msg.contains("not allowed by any active skill"));
    }
}

#[tokio::test]
async fn test_multiple_skills_any_allows() {
    // Create two skills with different restrictions
    let skill1 = Skill {
        name: "skill1".to_string(),
        description: "Skill 1".to_string(),
        allowed_tools: Some("read(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Content 1".to_string(),
        tags: vec![],
        version: None,
    };

    let skill2 = Skill {
        name: "skill2".to_string(),
        description: "Skill 2".to_string(),
        allowed_tools: Some("write(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Content 2".to_string(),
        tags: vec![],
        version: None,
    };

    let registry = SkillRegistry::new();
    registry.register(Arc::new(skill1)).unwrap();
    registry.register(Arc::new(skill2)).unwrap();

    let tool_executor = Arc::new(ToolExecutor::new("/tmp".to_string()));
    let tool_context = ToolContext::new("/tmp".into());

    // Test write - should be allowed by skill2
    let cmd = ToolCommand::new(
        tool_executor.clone(),
        "write".to_string(),
        serde_json::json!({"file_path": "/tmp/test.txt", "content": "hello"}),
        tool_context.clone(),
        Some(Arc::new(registry)),
    );

    // This should succeed (skill2 allows write)
    let result = cmd.execute().await;
    if let Err(e) = result {
        let err_msg = e.to_string();
        assert!(!err_msg.contains("not allowed by any active skill"));
    }
}
