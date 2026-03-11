# Skill Tool Implementation - GitHub Issue #8

## Overview

Implemented the Skill tool mechanism that allows skills to be invoked as first-class tools with temporary permission grants. This enforces skill-based access patterns and prevents agents from bypassing skills to directly access underlying tools.

## Implementation Details

### Core Components

1. **SkillTool** (`crates/code/core/src/tools/skill.rs`)
   - New tool that implements the `Tool` trait
   - Takes skill name and optional prompt as arguments
   - Creates temporary permission policy from skill's `allowed-tools`
   - Spawns a new `AgentLoop` with the skill's permissions
   - Returns the skill execution result

2. **Permission Policy Creation**
   - Parses skill's `allowed-tools` field (e.g., "read(*), grep(*)")
   - Converts to `PermissionRule` objects
   - Creates `PermissionPolicy` with:
     - Allow rules: skill's allowed-tools
     - Deny rules: empty (default deny handles this)
     - Default decision: Deny (only allow what skill specifies)

3. **Temporary Permission Grant (RAII Pattern)**
   - Creates new `AgentConfig` with skill's permission policy
   - Sets `permission_checker` to the skill's policy
   - Creates new `AgentLoop` with this config
   - After execution completes, the AgentLoop is dropped
   - Permissions automatically revoked (RAII cleanup)

### Key Design Decisions

1. **No AgentLoop API Extension Needed**
   - Initially thought we needed to extend AgentLoop API
   - Discovered `AgentConfig` already has `permission_checker` field
   - Simply create new AgentLoop with modified config

2. **Skill System Prompt**
   - Injects skill content into `prompt_slots.role`
   - Provides context about skill's purpose and capabilities
   - Helps LLM understand its role during skill execution

3. **Temporary Skill Registry**
   - Creates isolated registry with only the invoked skill
   - Prevents skill from invoking other skills (unless explicitly allowed)
   - Clean separation of concerns

### Registration

Added `register_skill()` function in `tools/builtin/mod.rs`:
- Similar pattern to `register_task()`
- Requires: registry, LLM client, skill registry, tool executor, base config
- Registers SkillTool as a builtin tool

## Usage Example

```rust
// Agent has Skill(*) permission only
let config = AgentConfig {
    permission_checker: Some(Arc::new(PermissionPolicy {
        allow: vec![PermissionRule::new("Skill(*)")],
        deny: vec![PermissionRule::new("read(*)")],
        default_decision: PermissionDecision::Deny,
        enabled: true,
    })),
    ..Default::default()
};

// Agent invokes skill
// LLM calls: Skill("data-processor", prompt="Read README.md")
// Skill's allowed-tools (read, grep) are temporarily granted
// After execution, permissions are revoked
```

## Permission Isolation

The implementation achieves the desired permission isolation:

1. **Parent Agent**: Has `Skill(*)` permission, NOT direct tool access
2. **Skill Invocation**: Agent calls `Skill("data-processor")`
3. **Temporary Grant**: Skill's `allowed-tools` are granted
4. **Execution**: Skill can use read/grep tools
5. **Revocation**: After execution, permissions are automatically revoked
6. **Enforcement**: Agent cannot bypass skill to directly access tools

## Testing

Added unit test `test_skill_permission_policy()`:
- Creates skill with `allowed-tools: "read(*), grep(*)"`
- Verifies permission policy allows read/grep
- Verifies permission policy denies write

## Files Modified

1. `crates/code/core/src/tools/skill.rs` - New file
2. `crates/code/core/src/tools/mod.rs` - Added skill module
3. `crates/code/core/src/tools/builtin/mod.rs` - Added register_skill()
4. `crates/code/examples/skill_tool_example.py` - Usage example

## Next Steps

To fully integrate the Skill tool:

1. **Call register_skill() in agent_api.rs**
   - Similar to how register_task_with_mcp() is called
   - Pass required dependencies (registry, LLM client, etc.)

2. **Add Skill tool to default tools**
   - Decide if it should be enabled by default
   - Or require explicit opt-in via config

3. **Test nested skill invocations**
   - What happens if skill A invokes skill B?
   - Should this be allowed or blocked?

4. **Add integration tests**
   - Test full skill invocation flow
   - Test permission enforcement
   - Test error cases (skill not found, etc.)

5. **Update documentation**
   - Add Skill tool to README
   - Document skill-based access control pattern
   - Add examples to docs

## Architecture Alignment

This implementation follows the "Minimal Core + External Extensions" principle:
- Core: AgentLoop, ToolExecutor, PermissionPolicy (unchanged)
- Extension: SkillTool (new, pluggable)
- No changes to core components
- Clean separation of concerns
- RAII pattern for automatic cleanup
