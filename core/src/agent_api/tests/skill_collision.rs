use super::*;

#[tokio::test]
async fn skill_named_read_does_not_replace_the_builtin_tool() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "READ-BUILTIN-91").unwrap();
    let agent = Agent::from_config(test_config()).await.unwrap();
    let session = agent
        .session_async(dir.path().to_string_lossy().to_string(), None)
        .await
        .unwrap();
    let read_before = session
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "read")
        .expect("builtin read")
        .description;

    let added = session.add_skill(Arc::new(crate::skills::Skill {
        name: "read".to_string(),
        description: "skill that must not shadow read".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: crate::skills::SkillKind::Instruction,
        content: "Pinned instructions for the colliding skill.".to_string(),
        tags: Vec::new(),
        version: None,
    }));
    match &added {
        Ok(()) => assert!(
            session.skill_names().iter().any(|name| name == "read"),
            "accepted skill must stay in the skill catalog"
        ),
        Err(_) => assert!(
            !session.skill_names().iter().any(|name| name == "read"),
            "rejected skill must not appear in the skill catalog"
        ),
    }

    let read_after = session
        .tool_definitions()
        .into_iter()
        .find(|tool| tool.name == "read")
        .expect("builtin read after skill install");
    assert_eq!(read_after.description, read_before);
    let result = session
        .tool(
            "read",
            serde_json::json!({
                "file_path": dir.path().join("note.txt").to_string_lossy()
            }),
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0, "{}", result.output);
    assert!(result.output.contains("READ-BUILTIN-91"));
    assert!(!result.output.contains("Pinned instructions"));
}
