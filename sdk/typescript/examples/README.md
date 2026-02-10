# A3S Code SDK Examples

Comprehensive examples demonstrating all features of the A3S Code TypeScript SDK.

## 🚀 Quick Start

### 1. Start A3S Code Service

```bash
# Navigate to A3S Code directory
cd /path/to/a3s

# Start the service with configuration
./target/debug/a3s-code -d .a3s -w /tmp/a3s-workspace
```

### 2. Configure Model (Important!)

The examples use real LLM APIs configured in `a3s/.a3s/config.json`. You need to:

1. **Choose a model provider** - Edit `a3s/.a3s/config.json`:
   ```json
   {
     "defaultProvider": "openai",
     "defaultModel": "kimi-k2.5",
     "providers": [...]
   }
   ```

2. **Available providers in config**:
   - `anthropic` - Claude models (requires API key)
   - `openai` - KIMI K2.5 model (configured with API key)

3. **Restart A3S Code service** after changing configuration

### 3. Run Examples

```bash
cd sdk/typescript/examples
npm install
npm run kimi-test    # Test with KIMI model
npm run dev          # Simple test
```

## Available Examples

| Example | Description | Features |
|---------|-------------|----------|
| `kimi-test.ts` | **KIMI model test** | Test with KIMI K2.5 model, streaming, context usage |
| `simple-test.ts` | Basic SDK usage | Health check, sessions, generation, streaming |
| `chat-simulation.ts` | **Chat scenarios** | Multi-turn conversation, skills, streaming, context |
| `code-generation-interactive.ts` | **Code generation** | Interactive code generation with file operations |
| `skill-usage-demo.ts` | **Skill usage** | Skill loading, usage, and management |
| `storage-configuration.ts` | Storage types | Memory vs File storage, persistence |
| `hitl-confirmation.ts` | HITL system | Auto-approve, require-confirm, timeout behavior |
| `external-tasks.ts` | External task handling | Lane handlers, task delegation, sandbox execution |
| `provider-config.ts` | Provider management | Add providers, configure models, switch models |
| `todo-tracking.ts` | Task tracking | Create tasks, track status, priorities |
| `context-management.ts` | Context management | Monitor usage, compact, clear context |
| `code-review-agent.ts` | **Complete example** | Combines all features for production use |
| `permission-policy.ts` | Permission control | Set policies, check permissions, add rules |
| `event-streaming.ts` | Real-time events | Subscribe to events, monitor execution |
| `skill-management.ts` | Skill system | List, load, use, and unload skills |

## Prerequisites

1. **A3S Code Agent must be running** on `localhost:4088` (default)
   - Or set `A3S_ADDRESS` environment variable

2. **Node.js 18+**

3. **Configuration directory** - Examples use `a3s/.a3s` as the configuration directory
   - This directory should contain `config.json` with provider and model settings

## Installation

```bash
npm install
```

## Configuration

The examples automatically use the configuration from `a3s/.a3s/config.json`, which includes:

- Default provider and model settings
- API keys for LLM providers
- Model capabilities and limits

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `A3S_ADDRESS` | Agent gRPC address | `localhost:4088` |
| `ANTHROPIC_API_KEY` | Anthropic API key | (optional) |
| `OPENAI_API_KEY` | OpenAI API key | (optional) |

### Example

```bash
export A3S_ADDRESS=localhost:4088
export ANTHROPIC_API_KEY=sk-ant-xxx
npm run storage
```

## Running Examples

### Quick Run (with tsx, no build)

```bash
# Test with real models
npm run kimi-test        # Test with KIMI K2.5 model (recommended)
npm run dev              # Simple test (uses default model)

# Chat and code generation examples
npm run chat             # Multi-turn chat simulation
npm run code-gen         # Interactive code generation
npm run skill-demo       # Skill usage demonstration

# Basic examples
npm run storage          # Storage configuration
npm run hitl             # HITL confirmation (interactive)
npm run external-tasks   # External task handling
npm run provider         # Provider configuration
npm run todo             # Todo tracking
npm run context          # Context management

# Complete example
npm run code-review      # Code review agent (combines all features)

# Advanced examples
npm run permission       # Permission policy
npm run events           # Event streaming
npm run skills           # Skill management
```

### Build and Run

```bash
# Build all examples
npm run build

# Run built examples
npm test
```

## Example Details

### 🌟 KIMI Model Test (`kimi-test.ts`)

**Demonstrates:**
- Using KIMI K2.5 model (alternative to Anthropic)
- Loading configuration from `a3s/.a3s/config.json`
- Basic generation and streaming with Chinese prompts
- Context usage tracking

**Run:**
```bash
npm run kimi-test
```

**Expected output:**
```
KIMI Model Configuration:
  Model ID: kimi-k2.5
  Name: KIMI K2.5
  Base URL: http://35.220.164.252:3888/v1

✓ Response received:
  我是一个 AI 编程助手...

✓ Streaming complete
```

### 💬 Chat Simulation (`chat-simulation.ts`)

**Demonstrates:**
- Multi-turn conversation with context
- Skill loading and usage
- Streaming responses
- Tool execution handling
- Context management

**Run:**
```bash
npm run chat
```

### 🔧 Code Generation Interactive (`code-generation-interactive.ts`)

**Demonstrates:**
- Interactive code generation
- File operations (create, read, modify)
- Tool result handling
- Real-time streaming output

**Run:**
```bash
npm run code-gen
```

### 📦 Skill Usage Demo (`skill-usage-demo.ts`)

**Demonstrates:**
- Discovering available skills
- Loading skills into sessions
- Using skill-provided tools
- Skill management (load/unload)

**Run:**
```bash
npm run skill-demo
```

### 1. Simple Test (`simple-test.ts`)

**Demonstrates:**
- Client creation with config directory
- Health check and capabilities
- Session management (create, list, destroy)
- Context usage tracking
- Basic text generation
- Streaming generation
- Message history retrieval

**Run:**
```bash
npm run dev
```

### 2. Storage Configuration (`storage-configuration.ts`)

**Demonstrates:**
- Memory storage (temporary, no persistence)
- File storage (persistent, survives restarts)
- Use cases for each storage type
- Session lifecycle management

**Run:**
```bash
npm run storage
```

### 3. HITL Confirmation (`hitl-confirmation.ts`)

**Demonstrates:**
- Configure auto-approve and require-confirm tools
- Handle confirmation requests interactively
- Timeout behavior (reject/auto-approve)
- YOLO mode for specific lanes

**Run:**
```bash
npm run hitl
```

**Note:** This example requires user interaction to approve/reject tool executions.

### 4. External Task Handling (`external-tasks.ts`)

**Demonstrates:**
- Configure lane handlers (Internal/External/Hybrid)
- Poll and process external tasks
- Complete tasks with results
- Use case: Secure sandbox execution

**Run:**
```bash
npm run external-tasks
```

### 5. Provider Configuration (`provider-config.ts`)

**Demonstrates:**
- Add multiple providers (Anthropic, OpenAI)
- Configure model costs and limits
- Set default models
- Switch models per session
- List available providers

**Run:**
```bash
npm run provider
```

### 6. Todo/Task Tracking (`todo-tracking.ts`)

**Demonstrates:**
- Create and manage task lists
- Track status (pending/in_progress/completed/cancelled)
- Set priorities (high/medium/low)
- Agent interaction with tasks
- Task statistics

**Run:**
```bash
npm run todo
```

### 7. Context Management (`context-management.ts`)

**Demonstrates:**
- Monitor context usage
- Manual and automatic compaction
- Clear context for fresh starts
- Auto-compact configuration
- Context monitoring loop

**Run:**
```bash
npm run context
```

### 8. Code Review Agent (`code-review-agent.ts`) ⭐

**Complete example** combining multiple features:
- Persistent file storage
- Read-only permissions
- HITL confirmation
- Task tracking
- Context management

This demonstrates how to build a production-ready, secure code review agent.

**Run:**
```bash
npm run code-review
```

### 9. Permission Policy (`permission-policy.ts`)

**Demonstrates:**
- Setting permission policies
- Allow/deny specific tool executions
- Checking permissions before execution
- Adding permission rules dynamically

**Run:**
```bash
npm run permission
```

### 10. Event Streaming (`event-streaming.ts`)

**Demonstrates:**
- Subscribing to real-time agent events
- Handling different event types
- Monitoring agent execution
- Tracking tool usage and progress

**Run:**
```bash
npm run events
```

### 11. Skill Management (`skill-management.ts`)

**Demonstrates:**
- Listing available skills
- Loading skills dynamically
- Using skill capabilities in generation
- Unloading skills when done

**Run:**
```bash
npm run skills
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

Step 3: Configuring HITL confirmation...
✓ HITL configured:
  ✓ Auto-approve: Read, Glob, Grep
  ✓ Require confirm: Bash commands
  ✓ YOLO lanes: Query (read operations)

...
```

## Troubleshooting

### Connection Refused

Make sure the A3S Code Agent is running:

```bash
# In the a3s directory
./target/debug/a3s-code -d .a3s -w /tmp/workspace
```

### Module Not Found

```bash
npm install
```

### API Key Errors

Set the required API keys:

```bash
export ANTHROPIC_API_KEY=your-key-here
export OPENAI_API_KEY=your-key-here
```

## Project Structure

```
examples/
├── src/
│   ├── simple-test.ts              # Basic SDK usage
│   ├── storage-configuration.ts    # Storage types
│   ├── hitl-confirmation.ts        # HITL system
│   ├── external-tasks.ts           # External task handling
│   ├── provider-config.ts          # Provider management
│   ├── todo-tracking.ts            # Task tracking
│   ├── context-management.ts       # Context management
│   ├── code-review-agent.ts        # Complete example ⭐
│   ├── permission-policy.ts        # Permission system
│   ├── event-streaming.ts          # Event subscription
│   └── skill-management.ts         # Skill loading
├── dist/                            # Compiled output
├── package.json
├── tsconfig.json
└── README.md
```

## Learn More

- [SDK Documentation](../README.md)
- [Usage Guide](../../../../docs/usage-examples.md)
- [API Reference](../docs/api.md)

## License

MIT
