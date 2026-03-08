---
name: delegate-task
description: Delegate complex or multi-step tasks to specialized sub-agents using the task tool. Use when work can be parallelized, requires focused exploration, or benefits from isolated execution.
---

# Delegate Task

Guide for using the `task` tool to delegate work to specialized sub-agents.

## When to Delegate

- Codebase exploration that requires reading many files
- Independent sub-tasks that can run in parallel
- Tasks requiring focused attention (planning, code review)
- Work that benefits from restricted permissions (read-only exploration)

## Built-in Agents

| Agent | Capabilities | Permissions | Use For |
|-------|-------------|-------------|---------|
| `explore` | Read files, grep, glob, ls | Read-only (no write/edit/bash) | Fast codebase search, understanding structure |
| `general` | Read, write, edit, bash | Full except task | Multi-step implementation, bug fixes |
| `plan` | Read files, grep, glob, ls | Read-only (no write/edit/bash) | Designing implementation approaches |

## Custom Agents

Custom agents can be loaded from `agent_dirs` configured in the session. These appear alongside built-in agents and can be referenced by name in the `task` tool.

## Tool Usage

### Single task

```json
{
  "agent": "explore",
  "description": "Find auth handlers",
  "prompt": "Search for files that handle user authentication. Look for login, logout, session management, and JWT token handling."
}
```

### Parallel tasks (via `parallel_task`)

```json
{
  "tasks": [
    { "agent": "explore", "description": "Find API routes", "prompt": "List all API route definitions..." },
    { "agent": "explore", "description": "Find DB models", "prompt": "List all database model definitions..." }
  ]
}
```

### Team workflow (via `run_team`)

Use `run_team` when the goal needs decomposition, parallel execution, and quality review:

```json
{
  "goal": "Audit the auth system for security issues and produce a remediation plan"
}
```

With custom agent types:

```json
{
  "goal": "Refactor the payment module for maintainability",
  "lead_agent": "plan",
  "worker_agent": "general",
  "reviewer_agent": "general"
}
```

When to prefer `run_team` over `task`/`parallel_task`:
- The goal has an unknown number of subtasks (Lead determines them dynamically)
- Results need verification before acceptance (Reviewer enforces quality)
- Tasks may fail and need retry with feedback

## Best Practices

- **Be specific in prompts**: Give sub-agents clear, focused instructions
- **Use explore for search**: It's faster and read-only — no risk of side effects
- **Parallelize independent work**: Use `parallel_task` when sub-tasks don't depend on each other
- **Use team coordination**: Use `run_team` when subtasks are unknown upfront or need review
- **Keep prompts self-contained**: Sub-agents don't share context with the parent session
- **Use general sparingly**: Only when the sub-task needs to write files or run commands
