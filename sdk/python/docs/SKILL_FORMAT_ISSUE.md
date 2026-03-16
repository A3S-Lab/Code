# Skill File Format Issue Analysis

## User's Skill File (Reported)

```yaml
name: scoring-video-adapter
description:"视频评分适配器
kind:tool
allowed-tools: "mcp_video-processor_(*),mcp_longvt__(*),Bash(*),Read(*),Write(*)"
```

## Issues Identified

### 1. ❌ Unclosed Quote in `description`
```yaml
description:"视频评分适配器   # ← Missing closing quote
```

**Fix**:
```yaml
description: "视频评分适配器"
```

### 2. ❌ Invalid Field: `kind: tool`
The valid values for `kind` are:
- `instruction` (default)
- `persona`

**NOT** `tool`.

**Fix**:
```yaml
kind: instruction
```

### 3. ⚠️  Spacing Issue in `kind:tool`
Missing space after colon.

**Fix**:
```yaml
kind: instruction
```

### 4. ✓ `allowed-tools` Format is Correct
The format is correct, but note the hyphen in the field name.

## Corrected Skill File

```markdown
---
name: scoring-video-adapter
description: "视频评分适配器"
kind: instruction
allowed-tools: "mcp_video-processor_(*),mcp_longvt__(*),Bash(*),Read(*),Write(*)"
---
# Scoring Video Adapter

Your skill instructions here...
```

## Valid Skill Format Reference

According to `crates/code/core/src/skills/mod.rs`:

```rust
pub struct Skill {
    pub name: String,                    // Required
    pub description: String,             // Optional
    pub allowed_tools: Option<String>,   // Optional, field name: "allowed-tools"
    pub disable_model_invocation: bool,  // Optional, field name: "disable-model-invocation"
    pub kind: SkillKind,                 // Optional, values: "instruction" | "persona"
    pub content: String,                 // Body after frontmatter
    pub tags: Vec<String>,               // Optional
    pub version: Option<String>,         // Optional
}
```

### Valid Fields

| Field | Type | Required | Valid Values | Example |
|-------|------|----------|--------------|---------|
| `name` | string | ✅ Yes | Any string | `my-skill` |
| `description` | string | No | Any string | `"What it does"` |
| `allowed-tools` | string | No | Tool patterns | `"read(*), grep(*)"` |
| `disable-model-invocation` | bool | No | true/false | `false` |
| `kind` | string | No | `instruction`, `persona` | `instruction` |
| `tags` | array | No | List of strings | `["tag1", "tag2"]` |
| `version` | string | No | Any string | `"1.0.0"` |

### Invalid Fields

These fields are **NOT** recognized and will cause parsing issues:
- ❌ `kind: tool` (should be `instruction` or `persona`)
- ❌ Any custom fields not listed above

## Why the Skill Wasn't Found

The YAML parsing likely failed due to:

1. **Unclosed quote** in `description` → YAML parser error
2. **Invalid `kind` value** → Deserialization error

When parsing fails, the skill is silently skipped with a warning log:

```rust
// From registry.rs:143-150
match Skill::from_file(&path) {
    Ok(skill) => { /* register */ },
    Err(e) => {
        tracing::warn!("Failed to parse skill file {}: {}", path.display(), e);
        // Skill is skipped, not registered
    }
}
```

## How to Fix

1. **Fix the frontmatter**:
   ```yaml
   ---
   name: scoring-video-adapter
   description: "视频评分适配器"
   kind: instruction
   allowed-tools: "mcp_video-processor_(*),mcp_longvt__(*),Bash(*),Read(*),Write(*)"
   ---
   ```

2. **Verify with the diagnostic script**:
   ```bash
   python3 diagnose_skill_dirs.py
   ```

3. **Enable debug logging** to see parsing errors:
   ```python
   import logging
   logging.basicConfig(level=logging.DEBUG)
   ```

4. **Check for warning messages** in the output:
   - "Failed to parse skill file"
   - "Skill validation failed"

## Testing the Fix

After fixing the skill file, test with:

```python
from a3s_code import Agent, Orchestrator, SubAgentConfig
import logging

logging.basicConfig(level=logging.DEBUG)

agent = Agent.create("config.hcl")
orchestrator = Orchestrator.create(agent=agent)

handle = orchestrator.spawn_subagent(SubAgentConfig(
    agent_type="test",
    prompt="Call Skill('scoring-video-adapter')",
    workspace="/your/workspace",
    permissive=True,
    skill_dirs=["/absolute/path/to/skills"],
))

result = handle.wait()
print(result)
```

Look for debug messages like:
- `"Loaded skill 'scoring-video-adapter' from ..."`
- `"Failed to parse skill file ..."`
