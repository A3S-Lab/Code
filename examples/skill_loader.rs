//! Skill Loader Example
//!
//! Demonstrates loading and executing custom skills:
//! - A3S skill format (with tool definitions)
//! - Claude Code skill format (prompt-based)
//! - Dynamic tool registration and unregistration
//!
//! Run with:
//!   cargo run --example skill_loader

use a3s_box_code::config::CodeConfig;
use a3s_box_code::tools::{
    load_claude_code_skills, load_skills_from_dir, parse_skill_tools, ClaudeCodeSkill, ToolExecutor,
};
use serde_json::json;
use std::path::Path;

#[tokio::main]
async fn main() {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("a3s_box_code=info")
        .init();

    println!("╔══════════════════════════════════════════════════╗");
    println!("║         A3S Code - Skill Loader Demo             ║");
    println!("╚══════════════════════════════════════════════════╝\n");

    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let workspace = temp_dir.path().to_string_lossy().to_string();

    // =========================================================================
    // 1. Parse A3S skill format (inline)
    // =========================================================================
    println!("━━━ 1. A3S Skill Format (Tool Definitions) ━━━");

    let skill_content = r#"---
name: dev-tools
description: Development utility tools
version: 1.0.0
tools:
  - name: line-count
    description: Count lines in a file or directory
    backend:
      type: script
      interpreter: bash
      script: |
        if [ -d "$TOOL_ARG_PATH" ]; then
          find "$TOOL_ARG_PATH" -type f -name "*.${TOOL_ARG_EXT:-rs}" | xargs wc -l | tail -1
        else
          wc -l < "$TOOL_ARG_PATH"
        fi
    parameters:
      type: object
      properties:
        path:
          type: string
          description: File or directory path
        ext:
          type: string
          description: File extension to count (default: rs)
      required:
        - path

  - name: json-format
    description: Format and validate JSON data
    backend:
      type: script
      interpreter: python3
      script: |
        import json, os, sys
        args = json.loads(os.environ.get('TOOL_ARGS', '{}'))
        data = args.get('data', '')
        try:
            parsed = json.loads(data)
            print(json.dumps(parsed, indent=2, ensure_ascii=False))
        except json.JSONDecodeError as e:
            print(f"Invalid JSON: {e}", file=sys.stderr)
            sys.exit(1)
    parameters:
      type: object
      properties:
        data:
          type: string
          description: JSON string to format
      required:
        - data
---

# Dev Tools Skill

Provides development utility tools for code analysis and data formatting.
"#;

    let tools = parse_skill_tools(skill_content);
    println!("  Parsed {} tools from skill definition:", tools.len());
    for tool in &tools {
        println!("    • {} - {}", tool.name(), tool.description());
    }
    if tools.is_empty() {
        println!("  (Note: parse_skill_tools returns empty if YAML parsing fails)");
        println!("  Using register_skill_tools instead for execution demo below");
    }
    println!();

    // =========================================================================
    // 2. Register and execute skill tools
    // =========================================================================
    println!("━━━ 2. Register & Execute Skill Tools ━━━");

    let executor = ToolExecutor::new(workspace.clone());
    let initial_count = executor.definitions().len();
    println!("  Built-in tools: {}", initial_count);

    // Register skill tools (uses parse_skill_tools internally)
    let registered = executor.register_skill_tools(skill_content);
    println!("  Registered: {:?}", registered);
    println!("  Total tools: {}", executor.definitions().len());

    if registered.is_empty() {
        println!("  (Skill tools not registered - skipping execution demos)");
    }
    println!();

    // Execute the json-format tool (if registered)
    if registered.contains(&"json-format".to_string()) {
        println!("  Executing json-format:");
        let result = executor
            .execute(
                "json-format",
                &json!({
                    "data": r#"{"name":"A3S Code","version":"0.1.0","features":["tools","sessions","hitl"]}"#
                }),
            )
            .await
            .unwrap();

        println!("    exit_code: {}", result.exit_code);
        for line in result.output.lines() {
            println!("    │ {}", line);
        }
        println!();
    }

    // Execute line-count on the workspace (if registered)
    std::fs::write(
        temp_dir.path().join("main.rs"),
        "fn main() {\n    println!(\"hello\");\n}\n",
    )
    .unwrap();
    std::fs::write(
        temp_dir.path().join("lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();

    if registered.contains(&"line-count".to_string()) {
        println!("  Executing line-count:");
        let result = executor
            .execute("line-count", &json!({ "path": workspace, "ext": "rs" }))
            .await
            .unwrap();
        println!("    Result: {}", result.output.trim());
        println!();
    }

    // =========================================================================
    // 3. Claude Code skill format
    // =========================================================================
    println!("━━━ 3. Claude Code Skill Format ━━━");

    let claude_skill_content = r#"---
name: rust-best-practices
description: Best practices for Rust development. Use when writing or reviewing Rust code.
allowed-tools: Bash(cargo:*), Read(*), Grep(*), Edit(*.rs)
---

When writing Rust code, follow these best practices:

1. **Error Handling**: Use `thiserror` for library errors, `anyhow` for application errors
2. **Async**: Use `tokio` for async runtime, avoid blocking in async contexts
3. **Testing**: Write unit tests in the same file, integration tests in tests/
4. **Documentation**: Add doc comments (`///`) to all public items
5. **Naming**: Use snake_case for functions/variables, PascalCase for types
6. **Dependencies**: Minimize dependencies, prefer well-maintained crates
"#;

    let skill = ClaudeCodeSkill::parse(claude_skill_content).unwrap();
    println!("  Name:        {}", skill.name);
    println!("  Description: {}", skill.description);
    println!(
        "  Allowed:     {:?}",
        skill.allowed_tools.as_deref().unwrap_or("(all)")
    );
    println!("  Disable LLM: {}", skill.disable_model_invocation);
    println!("  Content preview:");
    for line in skill.content.lines().take(4) {
        println!("    │ {}", line);
    }
    println!();

    // Test tool permissions from Claude Code skill
    println!("  Tool Permission Checks:");
    let permission_tests = vec![
        ("Bash", "cargo build", true),
        ("Bash", "cargo test --lib", true),
        ("Read", "src/main.rs", true),
        ("Grep", "TODO", true),
        ("Edit", "src/lib.rs", true),
        ("Bash", "rm -rf /", false),
        ("Write", "output.txt", false),
    ];

    for (tool, arg, expected) in permission_tests {
        let allowed = skill.is_tool_allowed(tool, arg);
        let icon = if allowed == expected { "✓" } else { "✗" };
        let status = if allowed {
            "✅ allowed"
        } else {
            "🚫 denied"
        };
        println!("    {} {}({}) → {}", icon, tool, arg, status);
    }
    println!();

    // =========================================================================
    // 4. Load skills from directory
    // =========================================================================
    println!("━━━ 4. Load Skills from Directory ━━━");

    let skill_dir = temp_dir.path().join("skills");
    std::fs::create_dir(&skill_dir).unwrap();

    // Write A3S format skill
    std::fs::write(
        skill_dir.join("git-tools.md"),
        r#"---
name: git-tools
description: Git utility tools
tools:
  - name: git-summary
    description: Get a summary of git repository status
    backend:
      type: script
      interpreter: bash
      script: |
        echo "Branch: $(git branch --show-current 2>/dev/null || echo 'not a git repo')"
        echo "Status: $(git status --short 2>/dev/null | wc -l | tr -d ' ') changed files"
        echo "Last commit: $(git log --oneline -1 2>/dev/null || echo 'no commits')"
---
"#,
    )
    .unwrap();

    // Write Claude Code format skill
    std::fs::write(
        skill_dir.join("code-review.md"),
        r#"---
name: code-review
description: Code review a pull request
allowed-tools: Bash(gh issue view:*), Bash(gh search:*), Bash(gh pr comment:*)
disable-model-invocation: false
---

Provide a thorough code review for the given pull request.

Steps:
1. Check if the PR is still open
2. Review all changed files
3. Look for potential issues
4. Leave constructive comments
"#,
    )
    .unwrap();

    // Load A3S skills
    let a3s_skills = load_skills_from_dir(&skill_dir);
    println!("  A3S skills loaded: {}", a3s_skills.len());
    for tool in &a3s_skills {
        println!("    ��� {} - {}", tool.name(), tool.description());
    }

    // Load Claude Code skills
    let claude_skills = load_claude_code_skills(&skill_dir);
    println!("  Claude Code skills loaded: {}", claude_skills.len());
    for skill in &claude_skills {
        println!("    • {} - {}", skill.name, skill.description);
    }
    println!();

    // =========================================================================
    // 5. ToolExecutor with config (auto-loads from directories)
    // =========================================================================
    println!("━━━ 5. ToolExecutor with Config ━━━");

    let config = CodeConfig::new().add_skill_dir(&skill_dir);

    let configured_executor =
        ToolExecutor::with_config(temp_dir.path().to_string_lossy().to_string(), &config);

    let all_tools = configured_executor.definitions();
    println!("  Total tools (built-in + skills): {}", all_tools.len());
    println!("  Custom tools:");
    for tool in &all_tools {
        // Only show non-builtin tools
        if ![
            "bash",
            "read",
            "write",
            "edit",
            "grep",
            "glob",
            "ls",
            "web_fetch",
            "cron",
            "parse",
        ]
        .contains(&tool.name.as_str())
        {
            println!("    • {} - {}", tool.name, truncate(&tool.description, 50));
        }
    }
    println!();

    // =========================================================================
    // 6. Dynamic tool registration/unregistration
    // =========================================================================
    println!("━━━ 6. Dynamic Tool Management ━━━");

    let executor = ToolExecutor::new(workspace);
    println!("  Initial tools: {}", executor.definitions().len());

    // Register
    let names = executor.register_skill_tools(
        r#"---
name: temp-skill
tools:
  - name: temp-echo
    description: Temporary echo tool
    backend:
      type: script
      interpreter: bash
      script: echo "$TOOL_ARG_MSG"
    parameters:
      type: object
      properties:
        msg:
          type: string
---
"#,
    );
    println!(
        "  After register: {} (added {:?})",
        executor.definitions().len(),
        names
    );

    // Unregister
    let removed = executor.unregister_tools(&names);
    println!(
        "  After unregister: {} (removed {:?})",
        executor.definitions().len(),
        removed
    );
    println!();

    // =========================================================================
    // 7. Load real project skills (if available)
    // =========================================================================
    println!("━━━ 7. Project Skills ━━━");
    let project_skills_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("skills");

    if project_skills_dir.exists() {
        let skills = load_skills_from_dir(&project_skills_dir);
        let claude_skills = load_claude_code_skills(&project_skills_dir);

        println!("  📂 {}", project_skills_dir.display());
        println!("  A3S skills: {}", skills.len());
        for tool in &skills {
            println!("    • {} - {}", tool.name(), tool.description());
        }
        println!("  Claude Code skills: {}", claude_skills.len());
        for skill in &claude_skills {
            println!("    • {} - {}", skill.name, skill.description);
        }
    } else {
        println!(
            "  No project skills directory found at {}",
            project_skills_dir.display()
        );
    }
    println!();

    println!("✅ Skill loader demo complete!");
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}...", &s[..max])
    } else {
        s.to_string()
    }
}
