//! Built-in skills
//!
//! Provides a set of default skills that are compatible with Claude Code format.
//!
//! Built-in skills are loaded from the `core/skills/` directory and embedded at compile time.

use super::Skill;
use std::sync::Arc;

/// Get all built-in skills
///
/// Returns a vector of Arc-wrapped skills that are ready to use.
pub fn builtin_skills() -> Vec<Arc<Skill>> {
    vec![
        // Code assistance skills
        Arc::new(code_search_skill()),
        Arc::new(code_review_skill()),
        Arc::new(explain_code_skill()),
        Arc::new(find_bugs_skill()),
        // Tool documentation skills
        Arc::new(builtin_tools_skill()),
        Arc::new(delegate_task_skill()),
        Arc::new(find_skills_skill()),
    ]
}

/// Code search skill
fn code_search_skill() -> Skill {
    Skill {
        name: "code-search".to_string(),
        description: "Search codebase for patterns, functions, or types".to_string(),
        allowed_tools: Some("read(*), grep(*), glob(*)".to_string()),
        disable_model_invocation: false,
        kind: super::SkillKind::Instruction,
        content: r#"# Code Search

You are a specialized code search assistant. Your job is to help users find code in their codebase.

## Capabilities

- Search for function definitions, type definitions, or variable usage
- Find patterns across multiple files
- Locate imports and dependencies
- Search by file extension or directory

## Tools Available

- **grep**: Search file contents with regex patterns
- **glob**: Find files by pattern
- **read**: Read file contents to verify matches

## Best Practices

1. Use `grep` with appropriate patterns for code search
2. Use `glob` to narrow down file types (e.g., `**/*.rs` for Rust files)
3. Always verify matches by reading the file context
4. Provide file paths and line numbers in results
5. Show relevant code snippets with context

## Example Workflow

1. User asks: "Find all functions named `process_data`"
2. Use `grep -n "fn process_data" --glob "**/*.rs"`
3. Read the files to get context around matches
4. Present results with file paths, line numbers, and code snippets
"#
        .to_string(),
        tags: vec!["search".to_string(), "code".to_string()],
        version: Some("1.0.0".to_string()),
    }
}

/// Code review skill
fn code_review_skill() -> Skill {
    Skill {
        name: "code-review".to_string(),
        description: "Review code for best practices, bugs, and improvements".to_string(),
        allowed_tools: Some("read(*), grep(*), glob(*)".to_string()),
        disable_model_invocation: false,
        kind: super::SkillKind::Instruction,
        content: r#"# Code Review

You are a code review assistant. Analyze code and provide constructive feedback.

## Review Checklist

### Code Quality
- [ ] Clear and descriptive naming
- [ ] Appropriate function/method length
- [ ] Single Responsibility Principle
- [ ] DRY (Don't Repeat Yourself)
- [ ] Proper error handling

### Best Practices
- [ ] Follows language idioms
- [ ] Appropriate use of design patterns
- [ ] Efficient algorithms and data structures
- [ ] Proper resource management
- [ ] Thread safety (if applicable)

### Security
- [ ] Input validation
- [ ] No hardcoded secrets
- [ ] Proper authentication/authorization
- [ ] SQL injection prevention
- [ ] XSS prevention (for web code)

### Testing
- [ ] Unit tests exist
- [ ] Edge cases covered
- [ ] Error paths tested
- [ ] Integration tests (if needed)

### Documentation
- [ ] Public APIs documented
- [ ] Complex logic explained
- [ ] README updated (if needed)
- [ ] Examples provided

## Review Format

For each issue found:
1. **Location**: File path and line number
2. **Severity**: Critical / High / Medium / Low
3. **Issue**: What's wrong
4. **Recommendation**: How to fix it
5. **Example**: Show improved code (if applicable)

## Tone

- Be constructive and respectful
- Explain the "why" behind recommendations
- Acknowledge good practices when you see them
- Prioritize issues by severity
"#
        .to_string(),
        tags: vec!["review".to_string(), "quality".to_string()],
        version: Some("1.0.0".to_string()),
    }
}

/// Explain code skill
fn explain_code_skill() -> Skill {
    Skill {
        name: "explain-code".to_string(),
        description: "Explain how code works in clear, simple terms".to_string(),
        allowed_tools: Some("read(*), grep(*), glob(*)".to_string()),
        disable_model_invocation: false,
        kind: super::SkillKind::Instruction,
        content: r#"# Explain Code

You are a code explanation assistant. Break down complex code into understandable explanations.

## Explanation Structure

### 1. High-Level Overview
Start with what the code does at a conceptual level.

Example: "This function processes user input by validating it, transforming it, and storing it in the database."

### 2. Step-by-Step Breakdown
Walk through the code line by line or block by block.

Example:
```
Lines 1-5: Input validation
  - Checks if input is not null
  - Validates format using regex
  - Returns error if invalid

Lines 6-10: Data transformation
  - Converts to lowercase
  - Trims whitespace
  - Applies business rules
```

### 3. Key Concepts
Explain any important patterns, algorithms, or techniques used.

Example: "This uses the Builder pattern to construct complex objects step by step."

### 4. Dependencies
Mention important libraries, modules, or external dependencies.

### 5. Edge Cases
Point out how the code handles special cases or errors.

## Explanation Style

- Use simple, clear language
- Avoid jargon unless necessary (and explain it when used)
- Use analogies when helpful
- Provide examples
- Break complex concepts into smaller pieces
- Use diagrams or ASCII art for complex flows (if helpful)

## Target Audience

Adjust explanation depth based on context:
- **Beginner**: Explain basic concepts, language features
- **Intermediate**: Focus on design patterns, architecture
- **Expert**: Highlight subtle details, performance implications
"#.to_string(),
        tags: vec!["explain".to_string(), "documentation".to_string()],
        version: Some("1.0.0".to_string()),
    }
}

/// Find bugs skill
fn find_bugs_skill() -> Skill {
    Skill {
        name: "find-bugs".to_string(),
        description: "Identify potential bugs, vulnerabilities, and code smells".to_string(),
        allowed_tools: Some("read(*), grep(*), glob(*)".to_string()),
        disable_model_invocation: false,
        kind: super::SkillKind::Instruction,
        content: r#"# Find Bugs

You are a bug detection assistant. Identify potential issues in code.

## Bug Categories

### 1. Logic Errors
- Off-by-one errors
- Incorrect conditionals
- Wrong operator usage
- Missing edge case handling

### 2. Memory Issues
- Memory leaks
- Use after free
- Buffer overflows
- Dangling pointers

### 3. Concurrency Issues
- Race conditions
- Deadlocks
- Data races
- Missing synchronization

### 4. Error Handling
- Unchecked errors
- Silent failures
- Improper exception handling
- Missing cleanup in error paths

### 5. Security Vulnerabilities
- SQL injection
- XSS vulnerabilities
- Path traversal
- Insecure deserialization
- Hardcoded credentials

### 6. Performance Issues
- Inefficient algorithms (O(n²) when O(n) possible)
- Unnecessary allocations
- Repeated expensive operations
- Missing caching

### 7. Code Smells
- Dead code
- Duplicated code
- God objects/functions
- Tight coupling
- Magic numbers

## Detection Process

1. **Read the code** thoroughly
2. **Trace execution paths** mentally
3. **Check edge cases**: null, empty, max values
4. **Look for patterns** known to cause bugs
5. **Verify error handling** at each step
6. **Check resource management** (files, connections, memory)

## Report Format

For each bug found:

**Bug #N: [Brief Description]**
- **Location**: File:Line
- **Severity**: Critical / High / Medium / Low
- **Category**: [Logic/Memory/Concurrency/etc.]
- **Description**: What's wrong and why it's a problem
- **Impact**: What could happen if this bug is triggered
- **Fix**: How to resolve it
- **Example**: Show corrected code

## Severity Guidelines

- **Critical**: Security vulnerability, data loss, crash
- **High**: Incorrect behavior, memory leak, race condition
- **Medium**: Performance issue, code smell, maintainability
- **Low**: Minor inefficiency, style issue
"#
        .to_string(),
        tags: vec![
            "bugs".to_string(),
            "security".to_string(),
            "quality".to_string(),
        ],
        version: Some("1.0.0".to_string()),
    }
}

/// Built-in tools documentation skill
fn builtin_tools_skill() -> Skill {
    Skill {
        name: "builtin-tools".to_string(),
        description: "Built-in file operation and shell tools".to_string(),
        allowed_tools: None, // This is a documentation skill, not a restriction
        disable_model_invocation: false,
        kind: super::SkillKind::Instruction,
        content: include_str!("../../skills/builtin-tools.md").to_string(),
        tags: vec!["tools".to_string(), "documentation".to_string()],
        version: Some("1.0.0".to_string()),
    }
}

/// Delegate task skill
fn delegate_task_skill() -> Skill {
    Skill {
        name: "delegate-task".to_string(),
        description:
            "Delegate complex or multi-step tasks to specialized sub-agents using the task tool"
                .to_string(),
        allowed_tools: Some("task(*), parallel_task(*), run_team(*)".to_string()),
        disable_model_invocation: false,
        kind: super::SkillKind::Instruction,
        content: include_str!("../../skills/delegate-task.md").to_string(),
        tags: vec!["delegation".to_string(), "subagents".to_string()],
        version: Some("1.0.0".to_string()),
    }
}

/// Find skills skill
fn find_skills_skill() -> Skill {
    Skill {
        name: "find-skills".to_string(),
        description: "Helps users discover and install agent skills when they ask questions like \"how do I do X\", \"find a skill for X\", or express interest in extending capabilities".to_string(),
        allowed_tools: Some("search_skills(*), install_skill(*), load_skill(*)".to_string()),
        disable_model_invocation: false,
        kind: super::SkillKind::Instruction,
        content: include_str!("../../skills/find-skills.md").to_string(),
        tags: vec!["skills".to_string(), "discovery".to_string(), "installation".to_string()],
        version: Some("1.0.0".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillKind;

    #[test]
    fn test_builtin_skills_count() {
        let skills = builtin_skills();
        assert_eq!(
            skills.len(),
            7,
            "Expected 7 built-in skills (4 code assistance + 3 tool documentation)"
        );
    }

    #[test]
    fn test_code_search_skill() {
        let skill = code_search_skill();
        assert_eq!(skill.name, "code-search");
        assert!(skill.content.contains("Code Search"));
        assert!(skill.is_tool_allowed("grep"));
        assert!(skill.is_tool_allowed("read"));
        assert!(skill.is_tool_allowed("glob"));
        assert!(!skill.is_tool_allowed("write"));
    }

    #[test]
    fn test_code_review_skill() {
        let skill = code_review_skill();
        assert_eq!(skill.name, "code-review");
        assert!(skill.content.contains("Code Review"));
        assert_eq!(skill.kind, SkillKind::Instruction);
    }

    #[test]
    fn test_explain_code_skill() {
        let skill = explain_code_skill();
        assert_eq!(skill.name, "explain-code");
        assert!(skill.tags.contains(&"explain".to_string()));
    }

    #[test]
    fn test_find_bugs_skill() {
        let skill = find_bugs_skill();
        assert_eq!(skill.name, "find-bugs");
        assert!(skill.tags.contains(&"bugs".to_string()));
        assert!(skill.tags.contains(&"security".to_string()));
    }

    #[test]
    fn test_builtin_tools_skill() {
        let skill = builtin_tools_skill();
        assert_eq!(skill.name, "builtin-tools");
        assert!(skill.content.contains("Built-in Tools"));
        assert!(skill.content.contains("read"));
        assert!(skill.content.contains("write"));
        assert!(skill.content.contains("bash"));
    }

    #[test]
    fn test_delegate_task_skill() {
        let skill = delegate_task_skill();
        assert_eq!(skill.name, "delegate-task");
        assert!(skill.content.contains("Delegate Task"));
        assert!(skill.is_tool_allowed("task"));
        assert!(!skill.is_tool_allowed("write"));
    }

    #[test]
    fn test_find_skills_skill() {
        let skill = find_skills_skill();
        assert_eq!(skill.name, "find-skills");
        assert!(skill.content.contains("Find Skills"));
        assert!(skill.is_tool_allowed("search_skills"));
        assert!(skill.is_tool_allowed("install_skill"));
        assert!(skill.is_tool_allowed("load_skill"));
    }
}
