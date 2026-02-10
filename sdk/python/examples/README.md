# A3S Code Python SDK Examples

Comprehensive examples demonstrating all features of the A3S Code Python SDK.

## Available Examples

| Example | Description | Features |
|---------|-------------|----------|
| `basic_usage.py` | Basic SDK usage | Health check, sessions, generation, streaming |
| `storage_configuration.py` | Storage types | Memory vs File storage, persistence |
| `hitl_confirmation.py` | HITL system | Auto-approve, require-confirm, timeout behavior |
| `external_tasks.py` | External task handling | Lane handlers, task delegation, sandbox execution |
| `provider_config.py` | Provider management | Add providers, configure models, switch models |
| `todo_tracking.py` | Task tracking | Create tasks, track status, priorities |
| `context_management.py` | Context management | Monitor usage, compact, clear context |
| `code_review_agent.py` | **Complete example** | Combines all features for production use |
| `permission_policy.py` | Permission control | Set policies, check permissions, add rules |
| `event_streaming.py` | Real-time events | Subscribe to events, monitor execution |
| `skill_management.py` | Skill system | List, load, use, and unload skills |

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
  "apiKey": "your-api-key-here",
  "baseUrl": "https://api.anthropic.com"
}
```

Or set environment variables:

```bash
export ANTHROPIC_API_KEY=your-api-key-here
export OPENAI_API_KEY=your-openai-key-here
export A3S_ADDRESS=localhost:4088
```

## Running Examples

```bash
cd sdk/python

# Basic examples
python examples/basic_usage.py
python examples/storage_configuration.py
python examples/hitl_confirmation.py          # Interactive
python examples/external_tasks.py
python examples/provider_config.py
python examples/todo_tracking.py
python examples/context_management.py

# Complete example
python examples/code_review_agent.py          # Combines all features

# Advanced examples
python examples/permission_policy.py
python examples/event_streaming.py
python examples/skill_management.py
```

## Example Details

### 1. Basic Usage (`basic_usage.py`)

**Demonstrates:**
- Creating a client with async context manager
- Health check and capabilities
- Session management (create, list, destroy)
- Context usage tracking
- Basic text generation
- Streaming generation
- Message history retrieval

**Key Code:**
```python
async with A3sClient(address="localhost:4088") as client:
    # Create session
    session = await client.create_session(
        name="demo",
        workspace="/tmp/test",
        system_prompt="You are helpful.",
    )

    # Generate response
    response = await client.generate(
        session_id=session["session_id"],
        messages=[{"role": "user", "content": "Hello!"}],
    )

    # Streaming
    async for chunk in client.stream_generate(session_id, messages):
        print(chunk.get("content", ""), end="")
```

### 2. Storage Configuration (`storage_configuration.py`)

**Demonstrates:**
- Memory storage (temporary, no persistence)
- File storage (persistent, survives restarts)
- Use cases for each storage type
- Session lifecycle management

### 3. HITL Confirmation (`hitl_confirmation.py`)

**Demonstrates:**
- Configure auto-approve and require-confirm tools
- Handle confirmation requests interactively
- Timeout behavior (reject/auto-approve)
- YOLO mode for specific lanes

**Note:** This example requires user interaction.

### 4. External Task Handling (`external_tasks.py`)

**Demonstrates:**
- Configure lane handlers (Internal/External/Hybrid)
- Poll and process external tasks
- Complete tasks with results
- Use case: Secure sandbox execution

### 5. Provider Configuration (`provider_config.py`)

**Demonstrates:**
- Add multiple providers (Anthropic, OpenAI)
- Configure model costs and limits
- Set default models
- Switch models per session
- List available providers

### 6. Todo/Task Tracking (`todo_tracking.py`)

**Demonstrates:**
- Create and manage task lists
- Track status (pending/in_progress/completed/cancelled)
- Set priorities (high/medium/low)
- Agent interaction with tasks
- Task statistics

### 7. Context Management (`context_management.py`)

**Demonstrates:**
- Monitor context usage
- Manual and automatic compaction
- Clear context for fresh starts
- Auto-compact configuration
- Context monitoring loop

### 8. Code Review Agent (`code_review_agent.py`) ⭐

**Complete example** combining multiple features:
- Persistent file storage
- Read-only permissions
- HITL confirmation
- Task tracking
- Context management

This demonstrates how to build a production-ready, secure code review agent.

### 9. Permission Policy (`permission_policy.py`)

**Demonstrates:**
- Setting permission policies
- Allow/deny specific tool executions
- Checking permissions before execution
- Adding permission rules dynamically

**Key Code:**
```python
# Set policy
await client.set_permission_policy(session_id, {
    "default_decision": "PERMISSION_DECISION_ASK",
    "rules": [
        {
            "pattern": "read(*)",
            "decision": "PERMISSION_DECISION_ALLOW",
        },
        {
            "pattern": "bash(rm:*)",
            "decision": "PERMISSION_DECISION_DENY",
        },
    ],
})

# Check permission
result = await client.check_permission(
    session_id, "bash", {"command": "rm -rf /"}
)
print(result["decision"])  # PERMISSION_DECISION_DENY
```

### 10. Event Streaming (`event_streaming.py`)

**Demonstrates:**
- Subscribing to real-time agent events
- Handling different event types
- Monitoring agent execution
- Tracking tool usage and progress

**Key Code:**
```python
async for event in client.subscribe_events(session_id):
    event_type = event.get("type")

    if event_type == "EVENT_TYPE_AGENT_START":
        print("Agent started")
    elif event_type == "EVENT_TYPE_TOOL_START":
        print(f"Tool: {event.get('tool_name')}")
    elif event_type == "EVENT_TYPE_TEXT_DELTA":
        print(event.get("text", ""), end="")
    elif event_type == "EVENT_TYPE_AGENT_END":
        print("Agent completed")
```

### 11. Skill Management (`skill_management.py`)

**Demonstrates:**
- Listing available skills
- Loading skills dynamically
- Using skill capabilities in generation
- Unloading skills when done

**Key Code:**
```python
# List skills
skills = await client.list_skills()

# Load a skill
await client.load_skill(session_id, "remotion-best-practices")

# Use the skill
response = await client.generate(session_id, [
    {"role": "user", "content": "How do I use Remotion?"}
])

# Unload the skill
await client.unload_skill(session_id, "remotion-best-practices")
```

## Expected Output

### Storage Configuration Example

```
==========================================================
Storage Configuration Example
==========================================================

1. Creating temporary session (memory storage)...
✓ Temporary session created: abc-123
  Storage: Memory (no persistence)

2. Creating persistent session (file storage)...
✓ Persistent session created: def-456
  Storage: File (persists across restarts)
  Sessions will be saved to: /tmp/workspace/sessions/
```

### Code Review Agent Example

```
==========================================================
Code Review Agent - Complete Example
==========================================================

Step 1: Creating persistent session...
✓ Session created: review-789
  Storage: File (persistent)
  Workspace: /tmp/code-review-workspace

Step 2: Configuring read-only permissions...
✓ Permissions configured:
  ✓ Read-only access to codebase
  ✓ Safe git commands allowed
  ✓ Write operations blocked

...
```

## Troubleshooting

### Connection Refused

```
Error: Connection refused to localhost:4088
```

**Solution**: Make sure the A3S Code Agent is running:

```bash
# In the a3s directory
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
├── basic_usage.py              # Basic SDK usage
├── storage_configuration.py    # Storage types
├── hitl_confirmation.py        # HITL system
├── external_tasks.py           # External task handling
├── provider_config.py          # Provider management
├── todo_tracking.py            # Task tracking
├── context_management.py       # Context management
├── code_review_agent.py        # Complete example ⭐
├── permission_policy.py        # Permission control
├── event_streaming.py          # Real-time events
├── skill_management.py         # Skill system
└── README.md                   # This file
```

## Learn More

- [SDK Documentation](../README.md)
- [Usage Guide](../../../../docs/usage-examples.md)
- [API Reference](../docs/api.md)

## License

MIT


## Example Details

### 1. Basic Usage (`basic_usage.py`)

**Demonstrates:**
- Creating a client with async context manager
- Health check and capabilities
- Session management (create, list, destroy)
- Context usage tracking
- Basic text generation
- Streaming generation
- Message history retrieval

**Key Code:**
```python
async with A3sClient(address="localhost:4088") as client:
    # Create session
    session = await client.create_session(
        name="demo",
        workspace="/tmp/test",
        system_prompt="You are helpful.",
    )

    # Generate response
    response = await client.generate(
        session_id=session["session_id"],
        messages=[{"role": "ROLE_USER", "content": "Hello!"}],
    )

    # Streaming
    async for chunk in client.stream_generate(session_id, messages):
        print(chunk.get("content", ""), end="")
```

### 2. Skill Management (`skill_management.py`)

**Demonstrates:**
- Listing available skills
- Loading skills dynamically
- Using skill capabilities in generation
- Unloading skills when done

**Key Code:**
```python
# List skills
skills = await client.list_skills()

# Load a skill
await client.load_skill(session_id, "remotion-best-practices")

# Use the skill
response = await client.generate(session_id, [
    {"role": "ROLE_USER", "content": "How do I use Remotion?"}
])

# Unload the skill
await client.unload_skill(session_id, "remotion-best-practices")
```

### 3. Permission Policy (`permission_policy.py`)

**Demonstrates:**
- Setting permission policies
- Allow/deny specific tool executions
- Checking permissions before execution
- Adding permission rules dynamically

**Key Code:**
```python
# Set policy
await client.set_permission_policy(session_id, {
    "default_decision": "PERMISSION_DECISION_ASK",
    "rules": [
        {
            "pattern": "read(*)",
            "decision": "PERMISSION_DECISION_ALLOW",
        },
        {
            "pattern": "bash(rm:*)",
            "decision": "PERMISSION_DECISION_DENY",
        },
    ],
})

# Check permission
result = await client.check_permission(
    session_id, "bash", {"command": "rm -rf /"}
)
print(result["decision"])  # PERMISSION_DECISION_DENY
```

### 4. Event Streaming (`event_streaming.py`)

**Demonstrates:**
- Subscribing to real-time agent events
- Handling different event types
- Monitoring agent execution
- Tracking tool usage and progress

**Key Code:**
```python
async for event in client.subscribe_events(session_id):
    event_type = event.get("type")

    if event_type == "EVENT_TYPE_AGENT_START":
        print("Agent started")
    elif event_type == "EVENT_TYPE_TOOL_START":
        print(f"Tool: {event.get('tool_name')}")
    elif event_type == "EVENT_TYPE_TEXT_DELTA":
        print(event.get("text", ""), end="")
    elif event_type == "EVENT_TYPE_AGENT_END":
        print("Agent completed")
```

## Event Types

| Event Type | Description |
|------------|-------------|
| `EVENT_TYPE_AGENT_START` | Agent started processing |
| `EVENT_TYPE_TURN_START` | LLM turn started |
| `EVENT_TYPE_TEXT_DELTA` | Streaming text chunk |
| `EVENT_TYPE_TOOL_START` | Tool execution started |
| `EVENT_TYPE_TOOL_END` | Tool execution completed |
| `EVENT_TYPE_TURN_END` | LLM turn completed |
| `EVENT_TYPE_AGENT_END` | Agent completed |
| `EVENT_TYPE_ERROR` | Error occurred |
| `EVENT_TYPE_CONTEXT_RESOLVING` | Context resolution started |
| `EVENT_TYPE_CONTEXT_RESOLVED` | Context resolution completed |
| `EVENT_TYPE_PERMISSION_DENIED` | Tool execution denied |
| `EVENT_TYPE_CONFIRMATION_REQUIRED` | HITL confirmation needed |

## Expected Output

```
============================================================
Basic Usage Example
============================================================

1. Creating A3S client...
✓ Client created
  Address: localhost:4088

2. Checking agent health...
✓ Health status: STATUS_HEALTHY
  Message: Agent is healthy

3. Getting agent capabilities...
✓ Capabilities retrieved:
  Agent: a3s-code v0.1.0
  Features: 15
  Tools: 10

4. Creating a session...
✓ Session created: session-xxx

...

============================================================
All tests passed! ✓
============================================================
```

## Troubleshooting

### Connection Refused

```
Error: Connection refused to localhost:4088
```

**Solution**: Make sure the A3S Code Agent is running:

```bash
# In the a3s directory
just run-code
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
├── basic_usage.py        # Basic SDK usage
├── skill_management.py   # Skill system
├── permission_policy.py  # Permission control
├── event_streaming.py    # Real-time events
└── README.md             # This file
```

## License

MIT
