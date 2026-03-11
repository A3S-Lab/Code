# Skill Tool SDK Usage

## Overview

The Skill tool is automatically registered in all agent sessions. It allows the LLM to invoke skills as first-class tools with temporary permission grants.

## Python SDK

### Basic Usage

```python
from a3s_code import Agent

# Create agent
agent = Agent.create("agent.hcl")

# Create session with skills
session = agent.session(
    ".",
    builtin_skills=True,  # Enable built-in skills
    skill_dirs=["./skills"]  # Load custom skills
)

# The LLM can now invoke skills as tools
# Example: LLM calls Skill("data-processor", prompt="Read README.md")
response = await session.send("Use the data-processor skill to analyze README.md")
print(response.text)
```

### Permission Isolation

```python
from a3s_code import Agent, SessionOptions, PermissionPolicy, PermissionRule

# Create agent with restricted permissions
config = AgentConfig(
    permission_policy=PermissionPolicy(
        allow=[PermissionRule("Skill(*)")],  # Only allow Skill tool
        deny=[PermissionRule("read(*)")],    # Deny direct read access
        default_decision="deny"
    )
)

agent = Agent(config=config)

# Register a skill with specific allowed-tools
agent.register_skill(
    name="data-processor",
    description="Process and analyze data files",
    allowed_tools="read(*), grep(*)",  # Skill can use read and grep
    content="You are a data processing specialist..."
)

session = agent.session(".")

# Agent can only access read/grep through the skill
# Direct read calls will be denied
response = await session.send("Use data-processor to read file.txt")
```

## Node.js SDK

### Basic Usage

```typescript
import { Agent } from 'a3s-code';

// Create agent
const agent = Agent.create('agent.hcl');

// Create session with skills
const session = agent.session('.', {
  builtinSkills: true,  // Enable built-in skills
  skillDirs: ['./skills']  // Load custom skills
});

// The LLM can now invoke skills as tools
const response = await session.send(
  'Use the data-processor skill to analyze README.md'
);
console.log(response.text);
```

### Permission Isolation

```typescript
import { Agent, SessionOptions, PermissionPolicy, PermissionRule } from 'a3s-code';

// Create agent with restricted permissions
const config = {
  permissionPolicy: new PermissionPolicy({
    allow: [new PermissionRule('Skill(*)')],  // Only allow Skill tool
    deny: [new PermissionRule('read(*)')],    // Deny direct read access
    defaultDecision: 'deny'
  })
};

const agent = new Agent(config);

// Register a skill with specific allowed-tools
agent.registerSkill({
  name: 'data-processor',
  description: 'Process and analyze data files',
  allowedTools: 'read(*), grep(*)',  // Skill can use read and grep
  content: 'You are a data processing specialist...'
});

const session = agent.session('.');

// Agent can only access read/grep through the skill
const response = await session.send('Use data-processor to read file.txt');
```

## How It Works

1. **Automatic Registration**: The Skill tool is automatically registered when you create a session
2. **LLM Invocation**: The LLM can call `Skill("skill-name", prompt="...")` as a tool
3. **Temporary Permissions**: The skill's `allowed-tools` are granted during execution
4. **Automatic Revocation**: Permissions are revoked after the skill completes
5. **Permission Isolation**: Parent agent cannot bypass skills to access underlying tools

## Skill Definition Format

Skills are defined with YAML frontmatter in `.md` files:

```markdown
---
name: data-processor
description: Process and analyze data files
allowed-tools: read(*), grep(*)
---

# Data Processor Skill

You are a data processing specialist. You can:
- Read files to analyze data
- Search for patterns using grep
- Process and summarize information

You CANNOT:
- Write files
- Execute bash commands
- Access the network
```

## Permission Patterns

### Pattern 1: Skill-Only Access

Agent can only use tools through skills:

```python
PermissionPolicy(
    allow=[PermissionRule("Skill(*)")],
    deny=[PermissionRule("*")],  # Deny all direct tool access
    default_decision="deny"
)
```

### Pattern 2: Mixed Access

Agent can use some tools directly, others through skills:

```python
PermissionPolicy(
    allow=[
        PermissionRule("Skill(*)"),
        PermissionRule("bash(*)"),  # Direct bash access
    ],
    deny=[PermissionRule("read(*)")],  # Must use skill for read
    default_decision="deny"
)
```

### Pattern 3: Skill-Specific Restrictions

Different skills have different permissions:

```python
# Skill 1: Read-only
agent.register_skill(
    name="reader",
    allowed_tools="read(*), grep(*)"
)

# Skill 2: Write access
agent.register_skill(
    name="writer",
    allowed_tools="read(*), write(*), edit(*)"
)
```

## Best Practices

1. **Principle of Least Privilege**: Only grant skills the minimum tools they need
2. **Skill Composition**: Break complex tasks into multiple focused skills
3. **Clear Descriptions**: Write clear skill descriptions so the LLM knows when to use them
4. **Permission Boundaries**: Use skills to enforce security boundaries
5. **Audit Logging**: Monitor which skills are invoked and what tools they use

## Troubleshooting

### Skill Not Found

```
Error: Skill 'data-processor' not found
```

**Solution**: Ensure the skill is registered or loaded from a skill directory:

```python
session = agent.session(".", skill_dirs=["./skills"])
```

### Permission Denied

```
Error: Tool 'read' is blocked by permission policy
```

**Solution**: Check that the skill's `allowed-tools` includes the tool:

```yaml
allowed-tools: read(*), grep(*)
```

### Nested Skill Invocation

Currently, skills cannot invoke other skills. If you need this, structure your skills to be independent or use the Task tool for delegation.

## See Also

- [Skill Tool Implementation](./SKILL_TOOL_IMPLEMENTATION.md)
- [GitHub Issue #8](https://github.com/A3S-Lab/Code/issues/8)
- [Permission System Documentation](../docs/permissions.md)
