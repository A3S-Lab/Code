use super::*;
use crate::skills::SkillKind;
use std::io::Write;
use tempfile::TempDir;

#[test]
fn test_new_registry() {
    let registry = SkillRegistry::new();
    assert_eq!(registry.len(), 0);
    assert!(registry.is_empty());
}

#[test]
fn test_with_builtins_is_empty_compatibility_registry() {
    let registry = SkillRegistry::with_builtins();
    assert_eq!(registry.len(), 0);
    assert!(registry.is_empty());
}

#[test]
fn test_register_and_get() {
    let registry = SkillRegistry::new();

    let skill = Arc::new(Skill {
        name: "test-skill".to_string(),
        description: "A test skill".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Test content".to_string(),
        tags: vec![],
        version: None,
    });

    registry.register(skill.clone()).unwrap();

    assert_eq!(registry.len(), 1);
    let retrieved = registry.get("test-skill").unwrap();
    assert_eq!(retrieved.name, "test-skill");
}

#[test]
fn owned_registration_restores_only_the_exact_installed_skill() {
    fn skill(description: &str) -> Arc<Skill> {
        Arc::new(Skill {
            name: "shared-skill".to_string(),
            description: description.to_string(),
            allowed_tools: None,
            disable_model_invocation: false,
            kind: SkillKind::Instruction,
            content: description.to_string(),
            tags: vec![],
            version: None,
        })
    }

    let registry = SkillRegistry::new();
    let original = skill("original");
    let installed = skill("installed");
    registry.register_unchecked(Arc::clone(&original));

    let (accepted, shadowed) = registry
        .register_with_shadow(Arc::clone(&installed))
        .unwrap();
    assert!(accepted);
    assert!(Arc::ptr_eq(shadowed.as_ref().unwrap(), &original));
    assert!(registry.restore_if_same("shared-skill", &installed, shadowed));
    assert!(Arc::ptr_eq(
        &registry.get("shared-skill").unwrap(),
        &original
    ));

    let (_, shadowed) = registry
        .register_with_shadow(Arc::clone(&installed))
        .unwrap();
    let later = skill("later");
    registry.register_unchecked(Arc::clone(&later));
    assert!(!registry.restore_if_same("shared-skill", &installed, shadowed));
    assert!(Arc::ptr_eq(&registry.get("shared-skill").unwrap(), &later));
}

#[test]
fn owned_registration_cannot_shadow_a_builtin_skill() {
    let registry = SkillRegistry::new();
    let builtin = Arc::new(Skill {
        name: "protected-skill".to_string(),
        description: "built in".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "built in".to_string(),
        tags: vec![],
        version: None,
    });
    registry.register_builtin(Arc::clone(&builtin));

    let replacement = Arc::new(Skill {
        description: "replacement".to_string(),
        content: "replacement".to_string(),
        ..(*builtin).clone()
    });
    let (accepted, shadowed) = registry.register_with_shadow(replacement).unwrap();

    assert!(!accepted);
    assert!(shadowed.is_none());
    assert!(Arc::ptr_eq(
        &registry.get("protected-skill").unwrap(),
        &builtin
    ));
}

#[test]
fn test_list() {
    let registry = SkillRegistry::with_builtins();
    let names = registry.list();

    assert!(names.is_empty());
}

#[test]
fn test_remove() {
    let registry = SkillRegistry::with_builtins();
    registry.register_unchecked(Arc::new(Skill {
        name: "code-search".to_string(),
        description: "External skill".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: Vec::new(),
        version: None,
    }));
    assert_eq!(registry.len(), 1);

    let removed = registry.remove("code-search");
    assert!(removed.is_some());
    assert_eq!(registry.len(), 0);
    assert!(registry.get("code-search").is_none());
}

#[test]
fn test_clear() {
    let registry = SkillRegistry::with_builtins();
    registry.register_unchecked(Arc::new(Skill {
        name: "temporary-skill".to_string(),
        description: "Temporary".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: Vec::new(),
        version: None,
    }));
    assert_eq!(registry.len(), 1);

    registry.clear();
    assert_eq!(registry.len(), 0);
    assert!(registry.is_empty());
}

#[test]
fn test_by_kind() {
    let registry = SkillRegistry::with_builtins();
    let instruction_skills = registry.by_kind(SkillKind::Instruction);

    assert!(instruction_skills.is_empty());

    let persona_skills = registry.by_kind(SkillKind::Persona);
    assert_eq!(persona_skills.len(), 0);
}

#[test]
fn test_empty_compatibility_builtins_do_not_restrict_global_tools() {
    let registry = SkillRegistry::with_builtins();
    assert!(registry.global_tool_restricting_skills().is_empty());
}

#[test]
fn test_old_builtin_names_are_normal_external_skills() {
    let registry = SkillRegistry::with_builtins();
    registry.register_unchecked(Arc::new(Skill {
        name: "code-review".to_string(),
        description: "External skill".to_string(),
        allowed_tools: Some("read(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: Vec::new(),
        version: None,
    }));

    let restricting = registry.global_tool_restricting_skills();
    assert_eq!(restricting.len(), 1);
    assert_eq!(restricting[0].name, "code-review");
}

#[test]
fn test_global_tool_restricting_skills_are_sorted() {
    let registry = SkillRegistry::new();
    registry.register_unchecked(Arc::new(Skill {
        name: "zeta".to_string(),
        description: "Zeta".to_string(),
        allowed_tools: Some("read(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: Vec::new(),
        version: None,
    }));
    registry.register_unchecked(Arc::new(Skill {
        name: "alpha".to_string(),
        description: "Alpha".to_string(),
        allowed_tools: Some("grep(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: Vec::new(),
        version: None,
    }));

    let names: Vec<String> = registry
        .global_tool_restricting_skills()
        .into_iter()
        .map(|skill| skill.name.clone())
        .collect();
    assert_eq!(names, vec!["alpha".to_string(), "zeta".to_string()]);
}

#[test]
fn test_by_tag() {
    let registry = SkillRegistry::new();
    registry.register_unchecked(Arc::new(Skill {
        name: "code-search".to_string(),
        description: "External search skill".to_string(),
        allowed_tools: Some("read(*), grep(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: vec!["search".to_string()],
        version: None,
    }));
    registry.register_unchecked(Arc::new(Skill {
        name: "find-bugs".to_string(),
        description: "External bug skill".to_string(),
        allowed_tools: Some("read(*), grep(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: String::new(),
        tags: vec!["bugs".to_string(), "security".to_string()],
        version: None,
    }));
    let search_skills = registry.by_tag("search");

    assert_eq!(search_skills.len(), 1);
    let names: Vec<&str> = search_skills.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"code-search"));

    let security_skills = registry.by_tag("security");
    assert_eq!(security_skills.len(), 1);
    assert_eq!(security_skills[0].name, "find-bugs");
}

#[test]
fn test_load_from_dir() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;

    // Create a valid skill file
    let skill_path = temp_dir.path().join("test-skill.md");
    let mut file = std::fs::File::create(&skill_path)?;
    writeln!(file, "---")?;
    writeln!(file, "name: test-skill")?;
    writeln!(file, "description: A test skill")?;
    writeln!(file, "kind: instruction")?;
    writeln!(file, "---")?;
    writeln!(file, "# Test Skill")?;
    writeln!(file, "This is a test skill.")?;
    drop(file);

    // Create a non-skill .md file (should be skipped)
    let readme_path = temp_dir.path().join("README.md");
    std::fs::write(&readme_path, "# README\nNot a skill")?;

    // Create a non-.md file (should be skipped)
    let txt_path = temp_dir.path().join("notes.txt");
    std::fs::write(&txt_path, "Some notes")?;

    let registry = SkillRegistry::new();
    let loaded = registry.load_from_dir(temp_dir.path())?;

    assert_eq!(loaded, 1);
    assert_eq!(registry.len(), 1);
    assert!(registry.get("test-skill").is_some());

    Ok(())
}

#[test]
fn test_load_from_dir_recurses_into_nested_skill_dirs() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let nested = temp_dir.path().join("nested").join("code-review-helper");
    std::fs::create_dir_all(&nested)?;

    let skill_path = nested.join("SKILL.md");
    let mut file = std::fs::File::create(&skill_path)?;
    writeln!(file, "---")?;
    writeln!(file, "name: nested-skill")?;
    writeln!(file, "description: A nested skill")?;
    writeln!(file, "kind: instruction")?;
    writeln!(file, "---")?;
    writeln!(file, "# Nested Skill")?;
    writeln!(file, "This skill lives in a nested SKILL.md.")?;
    drop(file);

    let registry = SkillRegistry::new();
    let loaded = registry.load_from_dir(temp_dir.path())?;

    assert_eq!(loaded, 1);
    assert!(registry.get("nested-skill").is_some());
    Ok(())
}

#[test]
fn test_load_from_dir_accepts_yaml_list_allowed_tools() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let skill_path = temp_dir.path().join("ci-review.md");
    std::fs::write(
        &skill_path,
        r#"---
name: ci-review
description: Review CI failures
allowed-tools:
  - Read
  - Grep
  - Bash(cargo test -p a3s-code-core:*)
kind: instruction
---
# CI Review
"#,
    )?;

    let registry = SkillRegistry::new();
    let loaded = registry.load_from_dir(temp_dir.path())?;

    assert_eq!(loaded, 1);
    let skill = registry.get("ci-review").unwrap();
    assert_eq!(
        skill.allowed_tools.as_deref(),
        Some("Read, Grep, Bash(cargo test -p a3s-code-core:*)")
    );
    let permissions = skill.parse_allowed_tools();
    assert!(permissions
        .iter()
        .any(|perm| perm.tool == "Read" && perm.pattern == "*"));
    assert!(permissions
        .iter()
        .any(|perm| { perm.tool == "Bash" && perm.pattern == "cargo test -p a3s-code-core:*" }));
    Ok(())
}

#[test]
fn test_load_from_file() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let skill_path = temp_dir.path().join("my-skill.md");

    let mut file = std::fs::File::create(&skill_path)?;
    writeln!(file, "---")?;
    writeln!(file, "name: my-skill")?;
    writeln!(file, "description: My custom skill")?;
    writeln!(file, "---")?;
    writeln!(file, "# My Skill")?;
    drop(file);

    let registry = SkillRegistry::new();
    let skill = registry.load_from_file(&skill_path)?;

    assert_eq!(skill.name, "my-skill");
    assert_eq!(registry.len(), 1);

    Ok(())
}

#[test]
fn test_to_system_prompt() {
    let registry = SkillRegistry::with_builtins();
    let prompt = registry.to_system_prompt();

    assert!(prompt.is_empty());
}

#[test]
fn test_load_from_nonexistent_dir() {
    let registry = SkillRegistry::new();
    let result = registry.load_from_dir("/nonexistent/path");

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 0);
}

#[test]
fn test_load_from_dir_rejects_file_path() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;
    let path = temp_dir.path().join("not-a-directory.md");
    std::fs::write(&path, "# not a directory")?;

    let registry = SkillRegistry::new();
    let err = registry.load_from_dir(&path).unwrap_err();
    assert!(err.to_string().contains("Path is not a directory"));
    Ok(())
}

#[test]
fn test_load_from_dir_duplicate_name_overrides_previous_definition() -> anyhow::Result<()> {
    let temp_dir = TempDir::new()?;

    let first = temp_dir.path().join("first.md");
    std::fs::write(
        &first,
        "---\nname: duplicate-skill\ndescription: First copy\n---\n# First\nalpha\n",
    )?;

    let nested = temp_dir.path().join("nested");
    std::fs::create_dir_all(&nested)?;
    let second = nested.join("SKILL.md");
    std::fs::write(
        &second,
        "---\nname: duplicate-skill\ndescription: Second copy\n---\n# Second\nbeta\n",
    )?;

    let registry = SkillRegistry::new();
    let loaded = registry.load_from_dir(temp_dir.path())?;

    assert_eq!(loaded, 2);
    assert_eq!(registry.len(), 1);
    assert_eq!(
        registry.get("duplicate-skill").unwrap().description,
        "Second copy"
    );
    Ok(())
}

// --- Validator integration ---

#[test]
fn test_register_with_validator_accepts_old_builtin_name() {
    use crate::skills::validator::DefaultSkillValidator;

    let registry = SkillRegistry::new();
    registry.set_validator(Arc::new(DefaultSkillValidator::default()));

    let skill = Arc::new(Skill {
        name: "code-search".to_string(),
        description: "External skill using a formerly built-in name".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Search code carefully.".to_string(),
        tags: vec![],
        version: None,
    });

    let result = registry.register(skill);
    assert!(result.is_ok());
    assert_eq!(registry.len(), 1);
}

#[test]
fn test_register_with_validator_accepts_valid() {
    use crate::skills::validator::DefaultSkillValidator;

    let registry = SkillRegistry::new();
    registry.set_validator(Arc::new(DefaultSkillValidator::default()));

    let skill = Arc::new(Skill {
        name: "my-custom-skill".to_string(),
        description: "A valid skill".to_string(),
        allowed_tools: Some("read(*), grep(*)".to_string()),
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Help with code review.".to_string(),
        tags: vec![],
        version: None,
    });

    assert!(registry.register(skill).is_ok());
    assert_eq!(registry.len(), 1);
}

#[test]
fn test_register_without_validator_accepts_anything() {
    let registry = SkillRegistry::new();
    // No validator set

    let skill = Arc::new(Skill {
        name: "code-search".to_string(),
        description: "test".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "test".to_string(),
        tags: vec![],
        version: None,
    });

    assert!(registry.register(skill).is_ok());
}

#[test]
fn test_all_and_personas() {
    let registry = SkillRegistry::new();

    registry.register_unchecked(Arc::new(Skill {
        name: "persona-skill".to_string(),
        description: "Persona".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Persona,
        content: "Persona content".to_string(),
        tags: vec!["voice".to_string()],
        version: None,
    }));
    registry.register_unchecked(Arc::new(Skill {
        name: "instruction-skill".to_string(),
        description: "Instruction".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Instruction content".to_string(),
        tags: vec!["workflow".to_string()],
        version: None,
    }));

    let all_names: Vec<String> = registry
        .all()
        .into_iter()
        .map(|skill| skill.name.clone())
        .collect();
    assert_eq!(
        all_names,
        vec!["instruction-skill".to_string(), "persona-skill".to_string()]
    );
    assert_eq!(registry.personas().len(), 1);
    assert_eq!(registry.personas()[0].name, "persona-skill");
}

#[test]
fn test_load_from_file_with_validator_accepts_old_builtin_name() {
    use crate::skills::validator::DefaultSkillValidator;

    let temp_dir = TempDir::new().unwrap();
    let skill_path = temp_dir.path().join("code-search.md");

    let mut file = std::fs::File::create(&skill_path).unwrap();
    writeln!(file, "---").unwrap();
    writeln!(file, "name: code-search").unwrap();
    writeln!(file, "description: External search skill").unwrap();
    writeln!(file, "---").unwrap();
    writeln!(file, "# Search").unwrap();
    drop(file);

    let registry = SkillRegistry::new();
    registry.set_validator(Arc::new(DefaultSkillValidator::default()));

    let result = registry.load_from_file(&skill_path);
    assert!(result.is_ok());
    assert_eq!(registry.len(), 1);
}

#[test]
fn test_fork_is_independent() {
    let original = SkillRegistry::with_builtins();
    let fork = original.fork();

    // Fork has same skills as original
    assert_eq!(fork.len(), original.len());

    // Adding to fork does not affect original
    fork.register_unchecked(Arc::new(Skill {
        name: "session-only".to_string(),
        description: "Only in fork".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "content".to_string(),
        tags: vec![],
        version: None,
    }));

    assert_eq!(fork.len(), original.len() + 1);
    assert!(fork.get("session-only").is_some());
    assert!(original.get("session-only").is_none());
}

#[test]
fn test_fork_inherits_empty_compatibility_builtins() {
    let fork = SkillRegistry::with_builtins().fork();
    assert!(fork.is_empty());
}

#[test]
fn test_fork_preserves_validator() {
    use crate::skills::validator::DefaultSkillValidator;

    let original = SkillRegistry::new();
    original.set_validator(Arc::new(DefaultSkillValidator::default()));

    let fork = original.fork();
    let invalid = Arc::new(Skill {
        name: "BadName".to_string(),
        description: "invalid".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "content".to_string(),
        tags: vec![],
        version: None,
    });

    assert!(fork.register(invalid).is_err());
}

#[test]
fn test_search_skills_ranks_matches() {
    let registry = SkillRegistry::new();

    registry.register_unchecked(Arc::new(Skill {
        name: "build-planner".to_string(),
        description: "Plan complex builds".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Planner instructions".to_string(),
        tags: vec!["architecture".to_string()],
        version: None,
    }));
    let matches = registry.search("architecture plan", 5);
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].name, "build-planner");
}

#[test]
fn test_match_skills_matches_name_tag_and_description() {
    let registry = SkillRegistry::new();

    registry.register_unchecked(Arc::new(Skill {
        name: "build-planner".to_string(),
        description: "Plan complex builds".to_string(),
        allowed_tools: None,
        disable_model_invocation: false,
        kind: SkillKind::Instruction,
        content: "Planner instructions".to_string(),
        tags: vec!["architecture".to_string()],
        version: None,
    }));
    let by_name = registry.match_skills("please use build-planner for this task");
    assert!(by_name.contains("Planner instructions"));

    let by_tag = registry.match_skills("need architecture guidance");
    assert!(by_tag.contains("Planner instructions"));

    let by_description = registry.match_skills("help me plan the release");
    assert!(by_description.contains("Planner instructions"));

    assert!(registry
        .match_skills("totally unrelated request")
        .is_empty());
}
