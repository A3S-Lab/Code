# A3S Code Python SDK

<p align="center">
  <strong>Python Client for A3S Code Agent</strong>
</p>

<p align="center">
  <em>Full-featured async gRPC client for building AI coding agent applications</em>
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#installation">Installation</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#api-reference">API Reference</a>
</p>

---

## Overview

**a3s-code** is the official Python SDK for [A3S Code](https://github.com/a3s-lab/a3s), providing a complete async gRPC client implementation for the CodeAgentService API. Build AI-powered coding assistants, automation tools, and integrations with full access to A3S Code's capabilities.

### Why This SDK?

- **Complete API Coverage**: All 28+ RPCs from CodeAgentService
- **Type-Safe**: Full type hints with dataclasses and enums
- **Async/Await**: Native asyncio support with context managers
- **Flexible Configuration**: Environment variables, config files, or programmatic setup

## Features

| Category | Features |
|----------|----------|
| **Lifecycle** | Health check, capabilities, initialization, shutdown |
| **Sessions** | Create, list, get, delete sessions with persistence |
| **Generation** | Streaming responses, context compaction, abort support |
| **Tools** | Register/unregister skills, list available tools |
| **Context** | Add/clear context, manage conversation history |
| **Control** | Abort operations, cancel confirmations |
| **Events** | Subscribe to real-time agent events |
| **HITL** | Human-in-the-loop confirmations and responses |
| **Permissions** | Fine-grained permission policies |
| **Todos** | Task tracking for multi-step workflows |
| **Providers** | Multi-provider LLM configuration |

## Installation

```bash
pip install a3s-code
```

For development:

```bash
pip install a3s-code[dev]
```

## Quick Start

```python
import asyncio
from a3s_code import A3sClient, ProviderInfo, ModelInfo, MessageRole

async def main():
    # Create client with default config
    async with A3sClient() as client:
        # Check health
        health = await client.health_check()
        print(f"Agent status: {health['status']}")

        # Create a session
        session = await client.create_session(
            workspace="/path/to/project",
            system_prompt="You are a helpful coding assistant.",
        )

        # Generate a response (streaming)
        async for chunk in client.stream_generate(
            session_id=session["id"],
            messages=[{"role": MessageRole.USER, "content": "Explain this codebase"}]
        ):
            if chunk.get("type") == "text":
                print(chunk["content"], end="", flush=True)

        # Clean up
        await client.delete_session(session["id"])

asyncio.run(main())
```

## Usage Examples

### Multi-Turn Conversations

```python
import asyncio
from a3s_code import A3sClient, MessageRole

async def multi_turn_chat():
    async with A3sClient(config_dir="~/.a3s") as client:
        # Create session
        result = await client.create_session(
            name="chat-session",
            workspace="/path/to/project",
        )
        session_id = result["session_id"]

        # First turn
        async for chunk in client.stream_generate(session_id, [
            {"role": MessageRole.USER, "content": "List all Python files in this project"}
        ]):
            if chunk.get("content"):
                print(chunk["content"], end="", flush=True)

        # Second turn - context is preserved
        async for chunk in client.stream_generate(session_id, [
            {"role": MessageRole.USER, "content": "Now analyze the main entry point"}
        ]):
            if chunk.get("content"):
                print(chunk["content"], end="", flush=True)

        # Get conversation history
        messages = await client.get_messages(session_id, limit=10)
        print(f"\nConversation has {len(messages)} messages")

        await client.destroy_session(session_id)

asyncio.run(multi_turn_chat())
```

### Event Subscription

```python
import asyncio
from a3s_code import A3sClient, MessageRole

async def subscribe_to_events():
    async with A3sClient() as client:
        result = await client.create_session(
            name="event-demo",
            workspace="/tmp/workspace",
        )
        session_id = result["session_id"]

        # Subscribe to events
        event_stream = client.subscribe_events(session_id)

        # Handle events in background
        async def handle_events():
            async for event in event_stream:
                print(f"[{event['type']}] {event.get('message', '')}")

                if event["type"] == "EVENT_TYPE_TOOL_CALLED":
                    print(f"  Tool: {event.get('metadata', {}).get('tool_name')}")

                if event["type"] == "EVENT_TYPE_CONFIRMATION_REQUIRED":
                    print(f"  Confirmation needed for: {event.get('metadata', {}).get('tool_name')}")

        # Start event handler
        event_task = asyncio.create_task(handle_events())

        # Generate with tool usage
        async for chunk in client.stream_generate(session_id, [
            {"role": MessageRole.USER, "content": "Read the README.md file"}
        ]):
            if chunk.get("content"):
                print(chunk["content"], end="", flush=True)

        await client.destroy_session(session_id)
        event_task.cancel()

asyncio.run(subscribe_to_events())
```

### Human-in-the-Loop (HITL)

```python
import asyncio
from a3s_code import A3sClient, MessageRole, ConfirmationPolicy, TimeoutAction

async def hitl_demo():
    async with A3sClient() as client:
        result = await client.create_session(
            name="hitl-demo",
            workspace="/path/to/project",
        )
        session_id = result["session_id"]

        # Set confirmation policy - require approval for bash commands
        await client.set_confirmation_policy(
            session_id,
            ConfirmationPolicy(
                default_action=TimeoutAction.REJECT,
                timeout_ms=30000,
                rules=[
                    {
                        "tool_pattern": "bash",
                        "action": TimeoutAction.REJECT,
                        "require_confirmation": True,
                    }
                ]
            )
        )

        # Subscribe to events
        event_stream = client.subscribe_events(session_id)

        async def handle_confirmations():
            async for event in event_stream:
                if event["type"] == "EVENT_TYPE_CONFIRMATION_REQUIRED":
                    tool_name = event.get("metadata", {}).get("tool_name")
                    tool_args = event.get("metadata", {}).get("tool_args")

                    print(f"\nConfirmation required:")
                    print(f"  Tool: {tool_name}")
                    print(f"  Args: {tool_args}")

                    # Auto-approve for demo (in real app, prompt user)
                    approved = True

                    await client.confirm_tool_execution(
                        session_id,
                        approved=approved,
                        reason="User approved" if approved else "User rejected",
                    )

        # Start confirmation handler
        confirm_task = asyncio.create_task(handle_confirmations())

        # This will trigger confirmation
        async for chunk in client.stream_generate(session_id, [
            {"role": MessageRole.USER, "content": 'Run "ls -la" command'}
        ]):
            if chunk.get("content"):
                print(chunk["content"], end="", flush=True)

        await client.destroy_session(session_id)
        confirm_task.cancel()

asyncio.run(hitl_demo())
```

### Permission Policies

```python
import asyncio
from a3s_code import A3sClient, MessageRole, PermissionPolicy, PermissionDecision

async def permission_demo():
    async with A3sClient() as client:
        result = await client.create_session(
            name="permission-demo",
            workspace="/path/to/project",
        )
        session_id = result["session_id"]

        # Set permission policy - read-only mode
        await client.set_permission_policy(
            session_id,
            PermissionPolicy(
                default_decision=PermissionDecision.DENY,
                rules=[
                    {"tool_pattern": "read", "decision": PermissionDecision.ALLOW},
                    {"tool_pattern": "grep", "decision": PermissionDecision.ALLOW},
                    {"tool_pattern": "glob", "decision": PermissionDecision.ALLOW},
                    {"tool_pattern": "ls", "decision": PermissionDecision.ALLOW},
                    {"tool_pattern": "write", "decision": PermissionDecision.DENY},
                    {"tool_pattern": "bash", "decision": PermissionDecision.ASK},
                ]
            )
        )

        # Check permission before operation
        result = await client.check_permission(
            session_id,
            tool_name="write",
            args={"file_path": "/tmp/test.txt"}
        )

        print(f"Can write: {result['decision'] == PermissionDecision.ALLOW}")

        # This will be allowed (read-only tools)
        async for chunk in client.stream_generate(session_id, [
            {"role": MessageRole.USER, "content": "List all files in the current directory"}
        ]):
            if chunk.get("content"):
                print(chunk["content"], end="", flush=True)

        await client.destroy_session(session_id)

asyncio.run(permission_demo())
```

### Provider Configuration

```python
import asyncio
from a3s_code import A3sClient, ProviderInfo, ModelInfo

async def provider_demo():
    async with A3sClient() as client:
        # List available providers
        result = await client.list_providers()
        providers = result.get("providers", [])
        print("Available providers:", [p["name"] for p in providers])

        # Add a new provider
        await client.add_provider(
            ProviderInfo(
                name="openai",
                api_key="sk-...",
                base_url="https://api.openai.com/v1",
                models=[
                    ModelInfo(
                        id="gpt-4",
                        name="GPT-4",
                        family="gpt",
                        tool_call=True,
                    )
                ]
            )
        )

        # Set default model
        await client.set_default_model("openai", "gpt-4")

        # Get current default
        default = await client.get_default_model()
        print(f"Default: {default['provider']}/{default['model']}")

        # Create session with specific model
        result = await client.create_session(
            name="openai-session",
            workspace="/tmp/workspace",
            llm_config={
                "provider": "openai",
                "model": "gpt-4",
                "temperature": 0.7,
            }
        )
        session_id = result["session_id"]

        await client.destroy_session(session_id)

asyncio.run(provider_demo())
```

### Context Management

```python
import asyncio
from a3s_code import A3sClient, MessageRole

async def context_demo():
    async with A3sClient() as client:
        result = await client.create_session(
            name="context-demo",
            workspace="/path/to/project",
        )
        session_id = result["session_id"]

        # Have a long conversation...
        for i in range(10):
            await client.generate(session_id, [
                {"role": MessageRole.USER, "content": f"Question {i + 1}: Tell me about this project"}
            ])

        # Check context usage
        usage = await client.get_context_usage(session_id)
        print(f"Context tokens: {usage['total_tokens']}/{usage['max_tokens']}")
        print(f"Messages: {usage['message_count']}")

        if usage["total_tokens"] > usage["max_tokens"] * 0.8:
            print("Context is getting full, compacting...")

            # Compact context using LLM summarization
            result = await client.compact_context(session_id)
            print(f"Compacted: {result['original_messages']} → {result['compacted_messages']} messages")
            print(f"Saved: {result['tokens_saved']} tokens")

        await client.destroy_session(session_id)

asyncio.run(context_demo())
```

### Skills Management

```python
import asyncio
from a3s_code import A3sClient, MessageRole

async def skills_demo():
    async with A3sClient() as client:
        result = await client.create_session(
            name="skills-demo",
            workspace="/path/to/project",
        )
        session_id = result["session_id"]

        # Load a custom skill from markdown file
        with open("./my-skill.md", "r") as f:
            skill_content = f.read()

        await client.load_skill(session_id, "my-custom-tool", skill_content)

        # List all available skills
        skills = await client.list_skills(session_id)
        print("Available skills:", [s["name"] for s in skills])

        # Use the custom skill
        async for chunk in client.stream_generate(session_id, [
            {"role": MessageRole.USER, "content": "Use my-custom-tool to process data"}
        ]):
            if chunk.get("content"):
                print(chunk["content"], end="", flush=True)

        # Unload the skill
        await client.unload_skill(session_id, "my-custom-tool")

        await client.destroy_session(session_id)

asyncio.run(skills_demo())
```

### Todo/Task Tracking

```python
import asyncio
from a3s_code import A3sClient, MessageRole, Todo

async def todo_demo():
    async with A3sClient() as client:
        result = await client.create_session(
            name="todo-demo",
            workspace="/path/to/project",
        )
        session_id = result["session_id"]

        # Set initial todos
        await client.set_todos(session_id, [
            Todo(
                id="1",
                title="Analyze codebase structure",
                description="Understand the project layout",
                completed=False,
            ),
            Todo(
                id="2",
                title="Fix bug in authentication",
                description="User login fails with invalid token",
                completed=False,
            )
        ])

        # Agent works on tasks...
        await client.generate(session_id, [
            {"role": MessageRole.USER, "content": "Complete the first todo item"}
        ])

        # Get updated todos
        todos = await client.get_todos(session_id)
        print("Todos:")
        for todo in todos:
            status = "✓" if todo.completed else "○"
            print(f"  {status} {todo.title}")

        await client.destroy_session(session_id)

asyncio.run(todo_demo())
```

### Operation Control

```python
import asyncio
from a3s_code import A3sClient, MessageRole

async def control_demo():
    async with A3sClient() as client:
        result = await client.create_session(
            name="control-demo",
            workspace="/path/to/project",
        )
        session_id = result["session_id"]

        # Start a long-running operation
        async def generate_task():
            async for chunk in client.stream_generate(session_id, [
                {"role": MessageRole.USER, "content": "Analyze all files in this large project"}
            ]):
                if chunk.get("content"):
                    print(chunk["content"], end="", flush=True)

        task = asyncio.create_task(generate_task())

        # Cancel after 5 seconds
        await asyncio.sleep(5)
        print("\nCancelling operation...")
        await client.cancel(session_id)

        try:
            await task
        except Exception:
            print("Operation was cancelled")

        # Pause and resume
        await client.pause(session_id)
        print("Session paused")

        await client.resume(session_id)
        print("Session resumed")

        await client.destroy_session(session_id)

asyncio.run(control_demo())
```

## Configuration

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `A3S_ADDRESS` | gRPC server address | `localhost:4088` |
| `A3S_API_KEY` | API key for LLM provider | - |
| `A3S_DEFAULT_PROVIDER` | Default LLM provider | - |
| `A3S_DEFAULT_MODEL` | Default model ID | - |

### Config File

```json
{
  "address": "localhost:4088",
  "defaultProvider": "anthropic",
  "defaultModel": "claude-sonnet-4-20250514",
  "providers": [
    {
      "name": "anthropic",
      "apiKey": "sk-ant-...",
      "models": [
        { "id": "claude-sonnet-4-20250514", "name": "Claude Sonnet 4" }
      ]
    }
  ]
}
```

### Client Initialization

```python
# Default (localhost:4088)
client = A3sClient()

# Explicit address
client = A3sClient(address="localhost:4088")

# From config file
client = A3sClient(config_path="/path/to/config.json")

# From config directory
client = A3sClient(config_dir="/path/to/.a3s")

# With TLS
client = A3sClient(address="api.example.com:443", use_tls=True)
```

## API Reference

### Lifecycle

| Method | Description |
|--------|-------------|
| `connect()` | Connect to the gRPC server |
| `close()` | Close the connection |
| `health_check()` | Check agent health status |
| `get_capabilities()` | Get agent capabilities |
| `initialize(workspace, env)` | Initialize the agent |
| `shutdown()` | Shutdown the agent |

### Sessions

| Method | Description |
|--------|-------------|
| `create_session(config)` | Create a new session |
| `list_sessions()` | List all sessions |
| `get_session(id)` | Get session by ID |
| `delete_session(id)` | Delete a session |
| `configure_session(id, config)` | Update session config |
| `get_messages(id, limit, offset)` | Get conversation history |

### Generation

| Method | Description |
|--------|-------------|
| `generate(session_id, messages)` | Generate response (non-streaming) |
| `stream_generate(session_id, messages)` | Generate response (streaming) |
| `compact_context(session_id)` | Compact session context |

### Tools & Skills

| Method | Description |
|--------|-------------|
| `load_skill(session_id, name, content)` | Load a skill |
| `unload_skill(session_id, name)` | Unload a skill |
| `list_skills(session_id)` | List loaded skills |

### Control

| Method | Description |
|--------|-------------|
| `abort(session_id)` | Abort running operation |
| `cancel_confirmation(session_id)` | Cancel pending confirmation |
| `pause(session_id)` | Pause a session |
| `resume(session_id)` | Resume a session |

### Providers

| Method | Description |
|--------|-------------|
| `list_providers()` | List all providers |
| `get_provider(name)` | Get provider by name |
| `add_provider(provider)` | Add a new provider |
| `update_provider(provider)` | Update a provider |
| `remove_provider(name)` | Remove a provider |
| `set_default_model(provider, model)` | Set default model |
| `get_default_model()` | Get default model |

### Types

See `a3s_code/types.py` for complete type definitions including:

- `SessionConfig`, `Session`
- `Message`, `MessageRole`
- `GenerateResponse`, `GenerateChunk`
- `HealthStatus`, `HealthStatusCode`
- `ProviderInfo`, `ModelInfo`
- `Todo`, `Skill`, `AgentEvent`

## Development

```bash
# Install dependencies
just install

# Run tests
just test

# Run tests with coverage
just test-cov

# Type check
just check

# Lint
just lint

# Format
just fmt

# All checks
just ci
```

## A3S Ecosystem

This SDK is part of the A3S ecosystem:

| Project | Package | Purpose |
|---------|---------|---------|
| [a3s](https://github.com/a3s-lab/a3s) | `a3s-code` | AI coding agent framework |
| [sdk/typescript](../typescript) | `@a3s-lab/code` | TypeScript SDK |
| [sdk/python](.) | `a3s-code` | Python SDK (this package) |

## License

MIT License - see [LICENSE](LICENSE) for details.

---

<p align="center">
  Built by <a href="https://github.com/a3s-lab">A3S Lab</a>
</p>
