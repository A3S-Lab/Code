# A3S Code Python SDK Examples

Comprehensive examples demonstrating all features of the A3S Code Python SDK.

## Available Examples

| Example | Description | Features |
|---------|-------------|----------|
| `basic_usage.py` | Basic SDK usage | Health check, sessions, generation, streaming |
| `storage_configuration.py` | Storage types | Memory vs File storage, persistence |
| `skill_management.py` | Skill system | List, load, use, and unload skills |
| `claude_code_skills_example.py` | Claude Code skills | Frontmatter skills, allowed-tools, model invocation |
| `structured_generation_example.py` | Structured output | JSON Schema, unary and streaming structured generation |
| `permission_policy.py` | Permission control | Set policies, check permissions, add rules |
| `hitl_confirmation.py` | HITL system | Auto-approve, require-confirm, timeout behavior |
| `event_streaming.py` | Real-time events | Subscribe to events, monitor execution |
| `context_management.py` | Context management | Monitor usage, compact, clear context |
| `todo_tracking.py` | Task tracking | Create tasks, track status, priorities |
| `provider_config.py` | Provider management | Add providers, configure models, switch models |
| `external_tasks.py` | External task handling | Lane handlers, task delegation, sandbox execution |
| `planning_example.py` | Planning & goals | Execution plans, goal extraction, achievement tracking |
| `memory_example.py` | Memory system | Store/search/retrieve memories, memory tiers, statistics |
| `memory_events_example.py` | Memory events | Memory stored/searched/recalled/cleared events |
| `mcp_example.py` | MCP integration | Register/connect/disconnect servers, discover tools |
| `lsp_example.py` | LSP code intelligence | Hover, definition, references, symbols, diagnostics |
| `cron_example.py` | Cron scheduling | Create/update/pause/resume/delete jobs, execution history |
| `observability_example.py` | Observability | Tool metrics, LLM cost tracking, per-model/per-day breakdowns |
| `code_review_agent.py` | **Complete example** | Combines storage, permissions, HITL, todos, context |

## Prerequisites

1. **A3S Code Agent must be running** on `localhost:4088` (default)
   - Or set `A3S_ADDRESS` environment variable

2. **Python 3.9+**

3. **Install the SDK**:
   ```bash
   cd sdk/python
   pip install -e .
   ```

## Configuration

Create a configuration file at `~/.a3s/config.json`:

```json
{
  "defaultProvider": "anthropic",
  "defaultModel": "claude-sonnet-4-20250514",
  "providers": [{
    "name": "anthropic",
    "apiKey": "your-api-key-here",
    "models": [{"id": "claude-sonnet-4-20250514", "toolCall": true}]
  }]
}
```

Or set environment variables:

```bash
export ANTHROPIC_API_KEY=your-api-key-here
export A3S_ADDRESS=localhost:4088
```

## Running Examples

```bash
cd sdk/python

# Basic
python examples/basic_usage.py
python examples/storage_configuration.py
python examples/structured_generation_example.py

# Skills
python examples/skill_management.py
python examples/claude_code_skills_example.py

# Security & Control
python examples/permission_policy.py
python examples/hitl_confirmation.py          # Interactive
python examples/external_tasks.py

# Events & Context
python examples/event_streaming.py
python examples/context_management.py
python examples/todo_tracking.py

# Provider & Model
python examples/provider_config.py

# Planning & Memory
python examples/planning_example.py
python examples/memory_example.py
python examples/memory_events_example.py

# External Integrations
python examples/mcp_example.py
python examples/lsp_example.py
python examples/cron_example.py

# Observability
python examples/observability_example.py

# Complete example
python examples/code_review_agent.py
```

## Feature Coverage

All 65+ RPCs of the `CodeAgentService` are covered by examples:

| Category | RPCs | Example |
|----------|------|---------|
| Lifecycle | HealthCheck, GetCapabilities, Initialize, Shutdown | `basic_usage.py` |
| Sessions | CreateSession, DestroySession, ListSessions, GetSession, ConfigureSession, GetMessages | `basic_usage.py`, `storage_configuration.py` |
| Generation | Generate, StreamGenerate | `basic_usage.py` |
| Structured Generation | GenerateStructured, StreamGenerateStructured | `structured_generation_example.py` |
| Skills | LoadSkill, UnloadSkill, ListSkills, GetClaudeCodeSkills | `skill_management.py`, `claude_code_skills_example.py` |
| Context | GetContextUsage, CompactContext, ClearContext | `context_management.py` |
| Events | SubscribeEvents | `event_streaming.py`, `memory_events_example.py` |
| Control | Cancel, Pause, Resume | `event_streaming.py` |
| HITL | ConfirmToolExecution, SetConfirmationPolicy, GetConfirmationPolicy | `hitl_confirmation.py` |
| External Tasks | SetLaneHandler, GetLaneHandler, CompleteExternalTask, ListPendingExternalTasks | `external_tasks.py` |
| Permissions | SetPermissionPolicy, GetPermissionPolicy, CheckPermission, AddPermissionRule | `permission_policy.py` |
| Todos | GetTodos, SetTodos | `todo_tracking.py` |
| Providers | ListProviders, GetProvider, AddProvider, UpdateProvider, RemoveProvider, SetDefaultModel, GetDefaultModel | `provider_config.py` |
| Planning | CreatePlan, GetPlan, ExtractGoal, CheckGoalAchievement | `planning_example.py` |
| Memory | StoreMemory, RetrieveMemory, SearchMemories, GetMemoryStats, ClearMemories | `memory_example.py`, `memory_events_example.py` |
| MCP | RegisterMcpServer, ConnectMcpServer, DisconnectMcpServer, ListMcpServers, GetMcpTools | `mcp_example.py` |
| LSP | StartLspServer, StopLspServer, ListLspServers, LspHover, LspDefinition, LspReferences, LspSymbols, LspDiagnostics | `lsp_example.py` |
| Cron | ListCronJobs, CreateCronJob, GetCronJob, UpdateCronJob, PauseCronJob, ResumeCronJob, DeleteCronJob, GetCronHistory, RunCronJob, ParseCronSchedule | `cron_example.py` |
| Observability | GetToolMetrics, GetCostSummary | `observability_example.py` |

## Troubleshooting

### Connection Refused

```
Error: Connection refused to localhost:4088
```

**Solution**: Make sure the A3S Code Agent is running:

```bash
./target/debug/a3s-code -d .a3s -w /tmp/workspace
```

### Module Not Found

```
ModuleNotFoundError: No module named 'a3s_code'
```

**Solution**: Install the SDK:

```bash
cd sdk/python
pip install -e .
```

### API Key Not Set

```
Error: API key not configured
```

**Solution**: Set your API key in `~/.a3s/config.json` or environment variables.

## Project Structure

```
examples/
├── basic_usage.py                  # Basic SDK usage
├── storage_configuration.py        # Storage types
├── structured_generation_example.py # Structured output with JSON Schema
├── skill_management.py             # Skill system
├── claude_code_skills_example.py   # Claude Code skills
├── permission_policy.py            # Permission control
├── hitl_confirmation.py            # HITL system
├── event_streaming.py              # Real-time events
├── context_management.py           # Context management
├── todo_tracking.py                # Task tracking
├── provider_config.py              # Provider management
├── external_tasks.py               # External task handling
├── planning_example.py             # Planning & goals
├── memory_example.py               # Memory system
├── memory_events_example.py        # Memory events
├── mcp_example.py                  # MCP integration
├── lsp_example.py                  # LSP code intelligence
├── cron_example.py                 # Cron scheduling
├── observability_example.py        # Tool metrics & cost tracking
├── code_review_agent.py            # Complete example ⭐
└── README.md                       # This file
```

## Learn More

- [SDK Documentation](../README.md)
- [API Reference](../docs/api.md)

## License

MIT
