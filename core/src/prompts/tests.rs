use super::*;

#[test]
fn test_all_prompts_loaded() {
    // Verify all prompts are non-empty at compile time
    assert!(!SYSTEM_DEFAULT.is_empty());
    assert!(!CONTINUATION.is_empty());
    assert!(!BOUNDARIES.is_empty());
    assert!(!REPOSITORY_TOOL_CONTRACT.is_empty());
    assert!(!AGENT_EXPLORE.is_empty());
    assert!(!AGENT_PLAN.is_empty());
    assert!(!AGENT_CODE_REVIEW.is_empty());
    assert!(!CONTEXT_COMPACT.is_empty());
    assert!(!LLM_PLAN_SYSTEM.is_empty());
    assert!(!LLM_GOAL_EXTRACT_SYSTEM.is_empty());
    assert!(!LLM_GOAL_CHECK_SYSTEM.is_empty());
    assert!(!SKILLS_CATALOG_HEADER.is_empty());
    assert!(!PLAN_EXECUTE_GOAL.is_empty());
    assert!(!PLAN_EXECUTE_STEP.is_empty());
}

#[test]
fn test_render_template() {
    let result = render(
        PLAN_EXECUTE_GOAL,
        &[("goal", "Build app"), ("steps", "1. Init")],
    );
    assert!(result.contains("Build app"));
    assert!(!result.contains("{goal}"));
}

#[test]
fn test_render_multiple_placeholders() {
    let template = "Goal: {goal}\nCriteria: {criteria}\nState: {current_state}";
    let result = render(
        template,
        &[
            ("goal", "Build a REST API"),
            ("criteria", "- Endpoint works\n- Tests pass"),
            ("current_state", "API is deployed"),
        ],
    );
    assert!(result.contains("Build a REST API"));
    assert!(result.contains("Endpoint works"));
    assert!(result.contains("API is deployed"));
}

#[test]
fn test_delegated_agent_prompts_contain_guidelines() {
    assert!(AGENT_EXPLORE.contains("Guidelines"));
    assert!(AGENT_EXPLORE.contains("read-only"));
    assert!(AGENT_PLAN.contains("Guidelines"));
    assert!(AGENT_PLAN.contains("read-only"));
}

#[test]
fn test_context_summary_prefix() {
    assert!(CONTEXT_SUMMARY_PREFIX.contains("Context Summary"));
}

// ── SystemPromptSlots tests ──

#[test]
fn test_slots_default_builds_system_default() {
    let slots = SystemPromptSlots::default();
    let built = slots.build();
    assert!(built.contains("Core Behaviour"));
    assert!(built.contains("Tool Usage Strategy"));
    assert!(built.contains("Completion Criteria"));
    assert!(built.contains("Response Format"));
    assert!(built.contains("A3S Code"));
    // Safety boundaries are injected even though they no longer live inline
    // in system_default.md.
    assert!(built.contains("## Boundaries"));
    assert!(built.contains("untrusted data"));
}

#[test]
fn test_default_prompt_matches_current_runtime_contract() {
    let built = SystemPromptSlots::default().build();
    for required in [
        "## Operating Loop",
        "## Workspace, Sandbox, and Permissions",
        "tools exposed in the current turn",
        "configured A3S native workspace sandbox",
        "sandbox_permissions=\"require_escalated\"",
        "permissions, budgets, cancellation, or sandboxing",
        "private chain-of-thought",
        "high-impact mutations",
        "If a required tool is not exposed",
        "current role and task restrictions",
        "checkout with `force`",
    ] {
        assert!(
            built.contains(required),
            "default prompt missing: {required}"
        );
    }

    let lower = built.to_ascii_lowercase();
    assert!(
        !lower.contains("srt"),
        "default prompt retains removed SRT wording"
    );
    assert!(
        built.len() < 12_000,
        "default prompt grew beyond its budget"
    );
}

#[test]
fn test_default_prompt_preserves_core_tool_routing_guidance() {
    let prompt = SystemPromptSlots::default().build();
    for capability in [
        "`read`",
        "`ls`",
        "`search`",
        "`edit`",
        "`write`",
        "`patch`",
        "`bash`",
        "`batch`",
        "`program`",
        "`task`",
        "`git`",
        "`web_search`",
        "`web_fetch`",
    ] {
        assert!(
            prompt.contains(capability),
            "missing core capability: {capability}"
        );
    }
}

#[test]
fn test_boundaries_injected_for_every_style() {
    for style in [
        AgentStyle::GeneralPurpose,
        AgentStyle::Plan,
        AgentStyle::Verification,
        AgentStyle::Explore,
        AgentStyle::CodeReview,
    ] {
        let built = SystemPromptSlots::default().with_style(style).build();
        assert!(
            built.contains("## Boundaries"),
            "style {style:?} missing Boundaries section"
        );
    }
}

#[test]
fn test_boundaries_not_duplicated_in_general_purpose() {
    // system_default.md must NOT carry an inline copy (single source of truth
    // is boundaries.md, injected by build_with_style).
    assert!(!SYSTEM_DEFAULT.contains("## Boundaries"));
    let built = SystemPromptSlots::default().build();
    assert_eq!(built.matches("## Boundaries").count(), 1);
}

#[test]
fn test_repository_tool_contract_is_injected_for_every_style() {
    let required_contract = [
        "## Repository Tool Contract",
        "`file_path`",
        "`files`",
        "`max_output_bytes`",
        "`metadata.batch.continuation`",
        "`output_mode`",
        "`files_with_matches`",
        "`metadata.page.next_cursor`",
        "`sort: \"path\"`",
        "`dry_run`",
        "`expected_replacements`",
        "`max_replacements`",
    ];

    for style in [
        AgentStyle::GeneralPurpose,
        AgentStyle::Plan,
        AgentStyle::Verification,
        AgentStyle::Explore,
        AgentStyle::CodeReview,
    ] {
        let built = SystemPromptSlots::default().with_style(style).build();
        for expected in required_contract {
            assert!(
                built.contains(expected),
                "style {style:?} missing repository-tool guidance: {expected}"
            );
        }
        for capability in ["`read`", "`search`", "`edit`", "`bash`", "`task`"] {
            assert!(
                built.contains(capability),
                "style {style:?} lost core capability guidance: {capability}"
            );
        }
    }
}

#[test]
fn test_repository_tool_contract_is_not_duplicated() {
    let built = SystemPromptSlots::default().build();
    assert_eq!(built.matches("## Repository Tool Contract").count(), 1);
}

#[test]
fn test_repository_tool_contract_tracks_registered_schema_parameters() {
    for (tool, schema) in crate::tools::builtin::repository_tool_parameter_schemas() {
        assert!(
            REPOSITORY_TOOL_CONTRACT.contains(&format!("`{tool}`")),
            "repository-tool contract missing tool: {tool}"
        );
        let properties = schema["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("{tool} schema has no object properties"));
        for parameter in properties.keys() {
            assert!(
                REPOSITORY_TOOL_CONTRACT.contains(&format!("`{parameter}`")),
                "repository-tool contract missing {tool} parameter: {parameter}"
            );
        }
    }
}

#[test]
fn test_slots_custom_role_replaces_default() {
    let slots = SystemPromptSlots {
        role: Some("You are a senior Python developer".to_string()),
        ..Default::default()
    };
    let built = slots.build();
    assert!(built.contains("You are a senior Python developer"));
    assert!(!built.contains("You are A3S Code"));
    // Core sections still present
    assert!(built.contains("Core Behaviour"));
    assert!(built.contains("Tool Usage Strategy"));
    assert!(built.contains("tools exposed in the current turn"));
    assert!(built.contains("`read`"));
}

#[test]
fn test_slots_custom_guidelines_appended() {
    let slots = SystemPromptSlots {
        guidelines: Some("Always use type hints. Follow PEP 8.".to_string()),
        ..Default::default()
    };
    let built = slots.build();
    assert!(built.contains("## Guidelines"));
    assert!(built.contains("Always use type hints"));
    assert!(built.contains("Core Behaviour"));
}

#[test]
fn test_slots_custom_response_style_replaces_default() {
    let slots = SystemPromptSlots {
        response_style: Some("Be concise. Use bullet points.".to_string()),
        ..Default::default()
    };
    let built = slots.build();
    assert!(built.contains("Be concise. Use bullet points."));
    // Default response format content should be gone
    assert!(!built.contains("keep progress notes brief and useful"));
    // But core is still there
    assert!(built.contains("Core Behaviour"));
}

#[test]
fn test_slots_extra_appended() {
    let slots = SystemPromptSlots {
        extra: Some("Remember: always write tests first.".to_string()),
        ..Default::default()
    };
    let built = slots.build();
    assert!(built.contains("Remember: always write tests first."));
    assert!(built.contains("Core Behaviour"));
}

#[test]
fn test_slots_with_extra() {
    let slots = SystemPromptSlots::default().with_extra("You are a helpful assistant.");
    let built = slots.build();
    assert!(built.contains("You are a helpful assistant."));
    assert!(built.contains("Core Behaviour"));
    assert!(built.contains("Tool Usage Strategy"));
}

#[test]
fn test_slots_all_slots_combined() {
    let slots = SystemPromptSlots {
        style: None,
        role: Some("You are a Rust expert".to_string()),
        guidelines: Some("Use clippy. No unwrap.".to_string()),
        response_style: Some("Short answers only.".to_string()),
        extra: Some("Project uses tokio.".to_string()),
    };
    let built = slots.build();
    assert!(built.contains("You are a Rust expert"));
    assert!(built.contains("Core Behaviour"));
    assert!(built.contains("## Guidelines"));
    assert!(built.contains("Use clippy"));
    assert!(built.contains("Short answers only"));
    assert!(built.contains("Project uses tokio"));
    // Default response format replaced
    assert!(!built.contains("keep progress notes brief and useful"));
}

#[test]
fn test_slots_is_empty() {
    assert!(SystemPromptSlots::default().is_empty());
    assert!(!SystemPromptSlots {
        role: Some("test".to_string()),
        ..Default::default()
    }
    .is_empty());
    assert!(!SystemPromptSlots {
        style: Some(AgentStyle::Plan),
        ..Default::default()
    }
    .is_empty());
}

// ── AgentStyle tests ──

#[test]
fn test_agent_style_default_is_general_purpose() {
    assert_eq!(AgentStyle::default(), AgentStyle::GeneralPurpose);
}

#[test]
fn test_agent_style_base_prompt() {
    assert_eq!(AgentStyle::GeneralPurpose.base_prompt(), SYSTEM_DEFAULT);
    assert_eq!(AgentStyle::Plan.base_prompt(), AGENT_PLAN);
    assert_eq!(AgentStyle::Explore.base_prompt(), AGENT_EXPLORE);
    assert_eq!(AgentStyle::Verification.base_prompt(), AGENT_VERIFICATION);
    assert_eq!(AgentStyle::CodeReview.base_prompt(), AGENT_CODE_REVIEW);
}

#[test]
fn test_agent_style_guidelines() {
    assert!(AgentStyle::GeneralPurpose.guidelines().is_none());
    assert!(AgentStyle::Plan.guidelines().is_none()); // embedded in prompt
    assert!(AgentStyle::Explore.guidelines().is_none());
    assert!(AgentStyle::Verification.guidelines().is_none());
    assert!(AgentStyle::CodeReview.guidelines().is_none());
}

#[test]
fn test_agent_style_builtin_agent_name_mapping() {
    assert_eq!(AgentStyle::GeneralPurpose.builtin_agent_name(), "general");
    assert_eq!(AgentStyle::Plan.builtin_agent_name(), "plan");
    assert_eq!(AgentStyle::Explore.builtin_agent_name(), "explore");
    assert_eq!(
        AgentStyle::Verification.builtin_agent_name(),
        "verification"
    );
    assert_eq!(AgentStyle::CodeReview.builtin_agent_name(), "review");
}

#[test]
fn test_agent_style_runtime_mode_mapping() {
    assert_eq!(AgentStyle::GeneralPurpose.runtime_mode(), "general");
    assert_eq!(AgentStyle::Plan.runtime_mode(), "planning");
    assert_eq!(AgentStyle::Explore.runtime_mode(), "explore");
    assert_eq!(AgentStyle::Verification.runtime_mode(), "verification");
    assert_eq!(AgentStyle::CodeReview.runtime_mode(), "code_review");
}

#[test]
fn test_agent_style_detect_plan() {
    assert_eq!(
        AgentStyle::detect_from_message("Help me plan a new feature"),
        AgentStyle::Plan
    );
    assert_eq!(
        AgentStyle::detect_from_message("Design the architecture for this"),
        AgentStyle::Plan
    );
    assert_eq!(
        AgentStyle::detect_from_message("What's the implementation approach?"),
        AgentStyle::Plan
    );
}

#[test]
fn test_agent_style_detect_verification() {
    assert_eq!(
        AgentStyle::detect_from_message("Verify that this works correctly"),
        AgentStyle::Verification
    );
    assert_eq!(
        AgentStyle::detect_from_message("Test the login flow"),
        AgentStyle::Verification
    );
    assert_eq!(
        AgentStyle::detect_from_message("Check if the API handles edge cases"),
        AgentStyle::Verification
    );
}

#[test]
fn test_agent_style_detect_explore() {
    assert_eq!(
        AgentStyle::detect_from_message("Find all files related to auth"),
        AgentStyle::Explore
    );
    assert_eq!(
        AgentStyle::detect_from_message("Where is the user model defined?"),
        AgentStyle::Explore
    );
    assert_eq!(
        AgentStyle::detect_from_message("Search for password hashing code"),
        AgentStyle::Explore
    );
}

#[test]
fn test_agent_style_detect_code_review() {
    assert_eq!(
        AgentStyle::detect_from_message("Review the PR changes"),
        AgentStyle::CodeReview
    );
    assert_eq!(
        AgentStyle::detect_from_message("Analyze this code for best practices"),
        AgentStyle::CodeReview
    );
    assert_eq!(
        AgentStyle::detect_from_message("Assess code quality"),
        AgentStyle::CodeReview
    );
}

#[test]
fn test_agent_style_detect_default_is_general_purpose() {
    // "Implement" was removed from Plan keywords as too generic
    assert_eq!(
        AgentStyle::detect_from_message("Implement the new feature"),
        AgentStyle::GeneralPurpose
    );
    // "Write tests" contains "test" so it's detected as Verification
    assert_eq!(
        AgentStyle::detect_from_message("Write code for the API"),
        AgentStyle::GeneralPurpose
    );
}

#[test]
fn test_build_with_message_auto_detects_style() {
    let slots = SystemPromptSlots::default();
    let built = slots.build_with_message("Help me plan a new feature");
    // Should use Plan style
    assert!(built.contains("planning agent") || built.contains("READ-ONLY"));
}

#[test]
fn test_build_with_message_explicit_style_overrides() {
    let slots = SystemPromptSlots {
        style: Some(AgentStyle::Verification),
        ..Default::default()
    };
    let built = slots.build_with_message("Help me plan a new feature");
    // Should use Verification style, not Plan
    assert!(built.contains("adversarial verification specialist"));
}

#[test]
fn test_build_with_message_plan_style() {
    let slots = SystemPromptSlots::default();
    let built = slots.build_with_message("Design the system architecture");
    assert!(built.contains("planning agent") || built.contains("READ-ONLY"));
}

#[test]
fn test_build_with_message_explore_style() {
    let slots = SystemPromptSlots::default();
    let built = slots.build_with_message("Find all authentication files");
    assert!(built.contains("exploration agent") || built.contains("explore"));
}

#[test]
fn test_build_with_message_code_review_style() {
    let slots = SystemPromptSlots::default();
    let built = slots.build_with_message("Review this code");
    assert!(built.contains("code review agent"));
    assert!(built.contains("regressions"));
}

#[test]
fn test_builder_methods() {
    let slots = SystemPromptSlots::default()
        .with_style(AgentStyle::Plan)
        .with_role("You are a Python expert")
        .with_guidelines("Use type hints")
        .with_response_style("Be brief")
        .with_extra("Additional instructions");

    assert_eq!(slots.style, Some(AgentStyle::Plan));
    assert_eq!(slots.role, Some("You are a Python expert".to_string()));
    assert_eq!(slots.guidelines, Some("Use type hints".to_string()));
    assert_eq!(slots.response_style, Some("Be brief".to_string()));
    assert_eq!(slots.extra, Some("Additional instructions".to_string()));

    let built = slots.build();
    assert!(built.contains("Python expert"));
    assert!(built.contains("Use type hints"));
    assert!(built.contains("Be brief"));
    assert!(built.contains("Additional instructions"));
}

#[test]
fn test_code_review_guidelines_appended() {
    let slots = SystemPromptSlots {
        style: Some(AgentStyle::CodeReview),
        ..Default::default()
    };
    let built = slots.build();
    assert!(built.contains("code review agent"));
    assert!(built.contains("correctness"));
    assert!(built.contains("regressions"));
    assert!(built.contains("security"));
}

#[test]
fn test_prompts_do_not_reference_removed_surfaces() {
    let prompts = [
        SYSTEM_DEFAULT,
        REPOSITORY_TOOL_CONTRACT,
        AGENT_VERIFICATION,
        PRE_ANALYSIS_SYSTEM,
        AGENT_EXPLORE,
        AGENT_PLAN,
        AGENT_CODE_REVIEW,
        SKILLS_CATALOG_HEADER,
        CONTINUATION,
    ]
    .join("\n")
    .to_lowercase();

    for removed in [
        "orchestrator",
        "plugin",
        "agentic_search",
        "agentic_parse",
        "agentic-search",
        "agentic-parse",
        "manage_skill",
        "claude.md",
    ] {
        assert!(
            !prompts.contains(removed),
            "prompt still references removed surface: {removed}"
        );
    }

    assert!(SYSTEM_DEFAULT.contains("program"));
    assert!(SYSTEM_DEFAULT.contains("task"));
    assert!(SYSTEM_DEFAULT.contains("tasks"));
    assert!(!SYSTEM_DEFAULT.contains("parallel_task"));
    assert!(!PRE_ANALYSIS_SYSTEM.contains("parallel_task"));
}
