# A3S Code TypeScript SDK

<p align="center">
  <strong>TypeScript/JavaScript Client for A3S Code Agent</strong>
</p>

<p align="center">
  <em>Full-featured gRPC client for building AI coding agent applications</em>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/API_Coverage-100%25-brightgreen" alt="API Coverage">
  <img src="https://img.shields.io/badge/Methods-53-blue" alt="Methods">
  <img src="https://img.shields.io/badge/TypeScript-5.3+-blue" alt="TypeScript">
  <img src="https://img.shields.io/badge/License-MIT-green" alt="License">
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#installation">Installation</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#api-reference">API Reference</a> •
  <a href="./examples">Examples</a>
</p>

---

## Overview

**@a3s-lab/code** is the official TypeScript SDK for [A3S Code](https://github.com/a3s-lab/a3s), providing a complete gRPC client implementation for the CodeAgentService API. Build AI-powered coding assistants, IDE integrations, and automation tools with full access to A3S Code's capabilities.

### Why This SDK?

- **100% API Coverage**: All 53 RPCs from CodeAgentService fully implemented
- **Type-Safe**: Full TypeScript definitions for all types and enums
- **Async/Await**: Modern Promise-based API with streaming support
- **Flexible Configuration**: Environment variables, config files, or programmatic setup
- **Real-World Examples**: Comprehensive examples with real LLM API integration

## Features

| Category | Features |
|----------|----------|
| **Lifecycle** | Health check, capabilities, initialization, shutdown |
| **Sessions** | Create, list, get, configure, destroy with persistence |
| **Generation** | Streaming/non-streaming responses, structured output, context compaction |
| **Tools** | Load/unload skills, list available tools, custom tool registration |
| **Context** | Add/clear context, manage conversation history, usage monitoring |
| **Control** | Abort operations, pause/resume, cancel confirmations |
| **Events** | Subscribe to real-time agent events, tool execution tracking |
| **HITL** | Human-in-the-loop confirmations, approval workflows |
| **Permissions** | Fine-grained permission policies, tool access control |
| **Todos** | Task tracking for multi-step workflows, goal management |
| **Providers** | Multi-provider LLM configuration (Anthropic, OpenAI, KIMI, etc.) |
| **Planning** | Execution plans, goal extraction, achievement checking |
| **Memory** | Episodic/semantic/procedural memory storage and retrieval |
| **Storage** | Configurable session storage (memory, file, custom) |

## Installation

```bash
npm install @a3s-lab/code
# or
yarn add @a3s-lab/code
# or
pnpm add @a3s-lab/code
```

## Quick Start

### Basic Usage

```typescript
import { A3sClient } from '@a3s-lab/code';

// Create client with default config
const client = new A3sClient();

// Or with explicit address
const client = new A3sClient({ address: 'localhost:4088' });

// Or load from config file
const client = new A3sClient({ configDir: '/path/to/.a3s' });

async function main() {
  // Check health
  const health = await client.healthCheck();
  console.log('Agent status:', health.status);

  // Create a session
  const session = await client.createSession({
    name: 'my-session',
    workspace: '/path/to/project',
    systemPrompt: 'You are a helpful coding assistant.',
  });

  // Generate a response (streaming)
  for await (const chunk of client.streamGenerate(session.sessionId, [
    { role: 'user', content: 'Explain this codebase structure' }
  ])) {
    if (chunk.type === 'CHUNK_TYPE_CONTENT' && chunk.content) {
      process.stdout.write(chunk.content);
    }
  }

  // Clean up
  await client.destroySession(session.sessionId);
  client.close();
}

main().catch(console.error);
```

### 📚 Complete Examples

See the [examples](./examples) directory for comprehensive, runnable examples:

| Example | Description | Run |
|---------|-------------|-----|
| [kimi-test.ts](./examples/src/kimi-test.ts) | Test with KIMI K2.5 model | `npm run kimi-test` |
| [chat-simulation.ts](./examples/src/chat-simulation.ts) | Multi-turn chat with skills | `npm run chat` |
| [code-generation-interactive.ts](./examples/src/code-generation-interactive.ts) | Interactive code generation | `npm run code-gen` |
| [skill-usage-demo.ts](./examples/src/skill-usage-demo.ts) | Skill loading and usage | `npm run skill-demo` |
| [simple-test.ts](./examples/src/simple-test.ts) | Basic SDK usage | `npm run dev` |
| [storage-configuration.ts](./examples/src/storage-configuration.ts) | Memory vs file storage | `npm run storage` |
| [hitl-confirmation.ts](./examples/src/hitl-confirmation.ts) | Human-in-the-loop | `npm run hitl` |
| [provider-config.ts](./examples/src/provider-config.ts) | Provider management | `npm run provider` |
| [context-management.ts](./examples/src/context-management.ts) | Context monitoring | `npm run context` |
| [code-review-agent.ts](./examples/src/code-review-agent.ts) | Complete production example | `npm run code-review` |

**Quick start with examples:**

```bash
cd examples
npm install

# Test with KIMI model (recommended)
npm run kimi-test

# Try chat simulation
npm run chat
```

See [examples/README.md](./examples/README.md) for detailed documentation and [TESTING_WITH_REAL_MODELS.md](./examples/TESTING_WITH_REAL_MODELS.md) for API configuration guide.

## Usage Examples

### Multi-Turn Conversations

```typescript
import { A3sClient } from '@a3s-lab/code';

const client = new A3sClient({ configDir: '~/.a3s' });

async function multiTurnChat() {
  await client.connect();

  const { sessionId } = await client.createSession({
    name: 'chat-session',
    workspace: '/path/to/project',
  });

  // First turn
  for await (const chunk of client.streamGenerate(sessionId, [
    { role: 'user', content: 'List all TypeScript files in this project' }
  ])) {
    if (chunk.content) process.stdout.write(chunk.content);
  }

  // Second turn - context is preserved
  for await (const chunk of client.streamGenerate(sessionId, [
    { role: 'user', content: 'Now analyze the main entry point' }
  ])) {
    if (chunk.content) process.stdout.write(chunk.content);
  }

  // Get conversation history
  const { messages } = await client.getMessages(sessionId, { limit: 10 });
  console.log(`\nConversation has ${messages.length} messages`);

  await client.destroySession(sessionId);
  client.close();
}
```

### Event Subscription

```typescript
import { A3sClient } from '@a3s-lab/code';

const client = new A3sClient();

async function subscribeToEvents() {
  await client.connect();

  const { sessionId } = await client.createSession({
    name: 'event-demo',
    workspace: '/tmp/workspace',
  });

  // Subscribe to all events
  const eventStream = client.subscribeEvents(sessionId);

  // Handle events in background
  (async () => {
    for await (const event of eventStream) {
      console.log(`[${event.type}] ${event.message}`);

      if (event.type === 'EVENT_TYPE_TOOL_CALLED') {
        console.log(`  Tool: ${event.metadata?.tool_name}`);
      }

      if (event.type === 'EVENT_TYPE_CONFIRMATION_REQUIRED') {
        console.log(`  Confirmation needed for: ${event.metadata?.tool_name}`);
      }
    }
  })();

  // Generate with tool usage
  for await (const chunk of client.streamGenerate(sessionId, [
    { role: 'user', content: 'Read the README.md file' }
  ])) {
    if (chunk.content) process.stdout.write(chunk.content);
  }

  await client.destroySession(sessionId);
  client.close();
}
```

### Human-in-the-Loop (HITL)

```typescript
import { A3sClient } from '@a3s-lab/code';

const client = new A3sClient();

async function hitlDemo() {
  await client.connect();

  const { sessionId } = await client.createSession({
    name: 'hitl-demo',
    workspace: '/path/to/project',
  });

  // Set confirmation policy - require approval for bash commands
  await client.setConfirmationPolicy(sessionId, {
    defaultAction: 'TIMEOUT_ACTION_REJECT',
    timeoutMs: 30000,
    rules: [
      {
        toolPattern: 'bash',
        action: 'TIMEOUT_ACTION_REJECT',
        requireConfirmation: true,
      }
    ]
  });

  // Subscribe to events to detect confirmation requests
  const eventStream = client.subscribeEvents(sessionId);

  (async () => {
    for await (const event of eventStream) {
      if (event.type === 'EVENT_TYPE_CONFIRMATION_REQUIRED') {
        const toolName = event.metadata?.tool_name;
        const toolArgs = event.metadata?.tool_args;

        console.log(`\nConfirmation required:`);
        console.log(`  Tool: ${toolName}`);
        console.log(`  Args: ${toolArgs}`);

        // Auto-approve for demo (in real app, prompt user)
        const approved = true;

        await client.confirmToolExecution(sessionId, {
          approved,
          reason: approved ? 'User approved' : 'User rejected',
        });
      }
    }
  })();

  // This will trigger confirmation
  for await (const chunk of client.streamGenerate(sessionId, [
    { role: 'user', content: 'Run "ls -la" command' }
  ])) {
    if (chunk.content) process.stdout.write(chunk.content);
  }

  await client.destroySession(sessionId);
  client.close();
}
```

### Permission Policies

```typescript
import { A3sClient } from '@a3s-lab/code';

const client = new A3sClient();

async function permissionDemo() {
  await client.connect();

  const { sessionId } = await client.createSession({
    name: 'permission-demo',
    workspace: '/path/to/project',
  });

  // Set permission policy - read-only mode
  await client.setPermissionPolicy(sessionId, {
    defaultDecision: 'PERMISSION_DECISION_DENY',
    rules: [
      {
        toolPattern: 'read',
        decision: 'PERMISSION_DECISION_ALLOW',
      },
      {
        toolPattern: 'grep',
        decision: 'PERMISSION_DECISION_ALLOW',
      },
      {
        toolPattern: 'glob',
        decision: 'PERMISSION_DECISION_ALLOW',
      },
      {
        toolPattern: 'ls',
        decision: 'PERMISSION_DECISION_ALLOW',
      },
      {
        toolPattern: 'write',
        decision: 'PERMISSION_DECISION_DENY',
      },
      {
        toolPattern: 'bash',
        decision: 'PERMISSION_DECISION_ASK',
      }
    ]
  });

  // Check permission before operation
  const canWrite = await client.checkPermission(sessionId, {
    toolName: 'write',
    args: { file_path: '/tmp/test.txt' }
  });

  console.log(`Can write: ${canWrite.decision === 'PERMISSION_DECISION_ALLOW'}`);

  // This will be allowed (read-only tools)
  for await (const chunk of client.streamGenerate(sessionId, [
    { role: 'user', content: 'List all files in the current directory' }
  ])) {
    if (chunk.content) process.stdout.write(chunk.content);
  }

  await client.destroySession(sessionId);
  client.close();
}
```

### Provider Configuration

```typescript
import { A3sClient } from '@a3s-lab/code';

const client = new A3sClient();

async function providerDemo() {
  await client.connect();

  // List available providers
  const { providers } = await client.listProviders();
  console.log('Available providers:', providers.map(p => p.name));

  // Add a new provider
  await client.addProvider({
    name: 'openai',
    apiKey: 'sk-...',
    baseUrl: 'https://api.openai.com/v1',
    models: [
      {
        id: 'gpt-4',
        name: 'GPT-4',
        family: 'gpt',
        toolCall: true,
      }
    ]
  });

  // Set default model
  await client.setDefaultModel('openai', 'gpt-4');

  // Get current default
  const { provider, model } = await client.getDefaultModel();
  console.log(`Default: ${provider}/${model}`);

  // Create session with specific model
  const { sessionId } = await client.createSession({
    name: 'openai-session',
    workspace: '/tmp/workspace',
    llmConfig: {
      provider: 'openai',
      model: 'gpt-4',
      temperature: 0.7,
    }
  });

  await client.destroySession(sessionId);
  client.close();
}
```

### Context Management

```typescript
import { A3sClient } from '@a3s-lab/code';

const client = new A3sClient();

async function contextDemo() {
  await client.connect();

  const { sessionId } = await client.createSession({
    name: 'context-demo',
    workspace: '/path/to/project',
  });

  // Have a long conversation...
  for (let i = 0; i < 10; i++) {
    await client.generate(sessionId, [
      { role: 'user', content: `Question ${i + 1}: Tell me about this project` }
    ]);
  }

  // Check context usage
  const usage = await client.getContextUsage(sessionId);
  console.log(`Context tokens: ${usage.totalTokens}/${usage.maxTokens}`);
  console.log(`Messages: ${usage.messageCount}`);

  if (usage.totalTokens > usage.maxTokens * 0.8) {
    console.log('Context is getting full, compacting...');

    // Compact context using LLM summarization
    const result = await client.compactContext(sessionId);
    console.log(`Compacted: ${result.originalMessages} → ${result.compactedMessages} messages`);
    console.log(`Saved: ${result.tokensSaved} tokens`);
  }

  await client.destroySession(sessionId);
  client.close();
}
```

### Skills Management

```typescript
import { A3sClient } from '@a3s-lab/code';
import { readFileSync } from 'fs';

const client = new A3sClient();

async function skillsDemo() {
  await client.connect();

  const { sessionId } = await client.createSession({
    name: 'skills-demo',
    workspace: '/path/to/project',
  });

  // Load a custom skill from markdown file
  const skillContent = readFileSync('./my-skill.md', 'utf-8');

  await client.loadSkill(sessionId, 'my-custom-tool', skillContent);

  // List all available skills
  const { skills } = await client.listSkills(sessionId);
  console.log('Available skills:', skills.map(s => s.name));

  // Use the custom skill
  for await (const chunk of client.streamGenerate(sessionId, [
    { role: 'user', content: 'Use my-custom-tool to process data' }
  ])) {
    if (chunk.content) process.stdout.write(chunk.content);
  }

  // Unload the skill
  await client.unloadSkill(sessionId, 'my-custom-tool');

  await client.destroySession(sessionId);
  client.close();
}
```

### Todo/Task Tracking

```typescript
import { A3sClient } from '@a3s-lab/code';

const client = new A3sClient();

async function todoDemo() {
  await client.connect();

  const { sessionId } = await client.createSession({
    name: 'todo-demo',
    workspace: '/path/to/project',
  });

  // Set initial todos
  await client.setTodos(sessionId, [
    {
      id: '1',
      title: 'Analyze codebase structure',
      description: 'Understand the project layout',
      completed: false,
    },
    {
      id: '2',
      title: 'Fix bug in authentication',
      description: 'User login fails with invalid token',
      completed: false,
    }
  ]);

  // Agent works on tasks...
  await client.generate(sessionId, [
    { role: 'user', content: 'Complete the first todo item' }
  ]);

  // Get updated todos
  const { todos } = await client.getTodos(sessionId);
  console.log('Todos:');
  todos.forEach(todo => {
    const status = todo.completed ? '✓' : '○';
    console.log(`  ${status} ${todo.title}`);
  });

  await client.destroySession(sessionId);
  client.close();
}
```

### Operation Control

```typescript
import { A3sClient } from '@a3s-lab/code';

const client = new A3sClient();

async function controlDemo() {
  await client.connect();

  const { sessionId } = await client.createSession({
    name: 'control-demo',
    workspace: '/path/to/project',
  });

  // Start a long-running operation
  const generatePromise = (async () => {
    for await (const chunk of client.streamGenerate(sessionId, [
      { role: 'user', content: 'Analyze all files in this large project' }
    ])) {
      if (chunk.content) process.stdout.write(chunk.content);
    }
  })();

  // Cancel after 5 seconds
  setTimeout(async () => {
    console.log('\nCancelling operation...');
    await client.cancel(sessionId);
  }, 5000);

  try {
    await generatePromise;
  } catch (error) {
    console.log('Operation was cancelled');
  }

  // Pause and resume
  await client.pause(sessionId);
  console.log('Session paused');

  await client.resume(sessionId);
  console.log('Session resumed');

  await client.destroySession(sessionId);
  client.close();
}
```

## Configuration

### Using Real LLM APIs

The SDK requires a running A3S Code service configured with real LLM API credentials. See [examples/TESTING_WITH_REAL_MODELS.md](./examples/TESTING_WITH_REAL_MODELS.md) for detailed setup instructions.

**Quick setup:**

1. **Configure A3S Code** - Edit `a3s/.a3s/config.json`:

```json
{
  "defaultProvider": "openai",
  "defaultModel": "kimi-k2.5",
  "providers": [
    {
      "name": "anthropic",
      "apiKey": "sk-ant-xxx",
      "baseUrl": "https://api.anthropic.com",
      "models": [
        {
          "id": "claude-sonnet-4-20250514",
          "name": "Claude Sonnet 4",
          "family": "claude-sonnet",
          "toolCall": true
        }
      ]
    },
    {
      "name": "openai",
      "models": [
        {
          "id": "kimi-k2.5",
          "name": "KIMI K2.5",
          "apiKey": "sk-xxx",
          "baseUrl": "http://your-endpoint/v1",
          "toolCall": true
        }
      ]
    }
  ]
}
```

2. **Start A3S Code service:**

```bash
cd /path/to/a3s
./target/debug/a3s-code -d .a3s -w /tmp/a3s-workspace
```

3. **Use SDK with config:**

```typescript
import { A3sClient } from '@a3s-lab/code';

// Load configuration from A3S Code config directory
const client = new A3sClient({
  address: 'localhost:4088',
  configDir: '/path/to/a3s/.a3s'
});

// Create session - will use default model from config
const session = await client.createSession({
  name: 'my-session',
  workspace: '/tmp/workspace'
});

// Or specify model explicitly
const session = await client.createSession({
  name: 'my-session',
  workspace: '/tmp/workspace',
  llm: {
    provider: 'openai',
    model: 'kimi-k2.5'
  }
});
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `A3S_ADDRESS` | gRPC server address | `localhost:4088` |
| `A3S_CONFIG_DIR` | Configuration directory | - |

### Programmatic Configuration

```typescript
import { A3sClient, loadConfigFromDir } from '@a3s-lab/code';

// Load config from directory
const config = loadConfigFromDir('/path/to/.a3s');

// Create client with loaded config
const client = new A3sClient({
  address: config.address || 'localhost:4088',
  configDir: '/path/to/.a3s'
});

// Access config values
console.log('Default provider:', config.defaultProvider);
console.log('Default model:', config.defaultModel);
console.log('API key:', config.apiKey ? '(set)' : '(not set)');
```

### Legacy Config File Format

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

## API Reference

### Client Methods

#### Lifecycle (4 methods)

| Method | Description |
|--------|-------------|
| `healthCheck()` | Check agent health status |
| `getCapabilities()` | Get agent capabilities |
| `initialize(workspace, env?)` | Initialize the agent |
| `shutdown()` | Shutdown the agent |

#### Sessions (6 methods)

| Method | Description |
|--------|-------------|
| `createSession(config)` | Create a new session |
| `destroySession(id)` | Destroy a session |
| `listSessions()` | List all sessions |
| `getSession(id)` | Get session by ID |
| `configureSession(id, config)` | Update session configuration |
| `getMessages(id, limit?)` | Get conversation history |

#### Generation (4 methods)

| Method | Description |
|--------|-------------|
| `generate(sessionId, messages)` | Generate response (non-streaming) |
| `streamGenerate(sessionId, messages)` | Generate response (streaming) |
| `generateStructured(sessionId, messages, schema)` | Generate structured output |
| `streamGenerateStructured(sessionId, messages, schema)` | Stream structured output |

#### Context Management (3 methods)

| Method | Description |
|--------|-------------|
| `getContextUsage(sessionId)` | Get context token usage |
| `compactContext(sessionId)` | Compact session context |
| `clearContext(sessionId)` | Clear session context |

#### Skills (3 methods)

| Method | Description |
|--------|-------------|
| `loadSkill(sessionId, name)` | Load a skill |
| `unloadSkill(sessionId, name)` | Unload a skill |
| `listSkills(sessionId?)` | List available/loaded skills |

#### Control (3 methods)

| Method | Description |
|--------|-------------|
| `cancel(sessionId)` | Cancel running operation |
| `pause(sessionId)` | Pause session |
| `resume(sessionId)` | Resume session |

#### Events (1 method)

| Method | Description |
|--------|-------------|
| `subscribeEvents(sessionId)` | Subscribe to real-time events |

#### HITL (3 methods)

| Method | Description |
|--------|-------------|
| `confirmToolExecution(sessionId, response)` | Respond to confirmation request |
| `setConfirmationPolicy(sessionId, policy)` | Set confirmation policy |
| `getConfirmationPolicy(sessionId)` | Get confirmation policy |

#### Permissions (4 methods)

| Method | Description |
|--------|-------------|
| `setPermissionPolicy(sessionId, policy)` | Set permission policy |
| `getPermissionPolicy(sessionId)` | Get permission policy |
| `checkPermission(sessionId, request)` | Check tool permission |
| `addPermissionRule(sessionId, rule)` | Add permission rule |

#### External Tasks (4 methods)

| Method | Description |
|--------|-------------|
| `setLaneHandler(sessionId, lane, handler)` | Set lane handler |
| `getLaneHandler(sessionId, lane)` | Get lane handler |
| `completeExternalTask(sessionId, taskId, result)` | Complete external task |
| `listPendingExternalTasks(sessionId)` | List pending tasks |

#### Todos (2 methods)

| Method | Description |
|--------|-------------|
| `getTodos(sessionId)` | Get todo list |
| `setTodos(sessionId, todos)` | Set todo list |

#### Providers (7 methods)

| Method | Description |
|--------|-------------|
| `listProviders()` | List available providers |
| `getProvider(name)` | Get provider details |
| `addProvider(provider)` | Add a provider |
| `updateProvider(name, provider)` | Update provider |
| `removeProvider(name)` | Remove provider |
| `setDefaultModel(provider, model)` | Set default model |
| `getDefaultModel()` | Get default model |

#### Planning & Goals (4 methods)

| Method | Description |
|--------|-------------|
| `createPlan(sessionId, prompt, context?)` | Create execution plan |
| `getPlan(sessionId, planId)` | Get existing plan |
| `extractGoal(sessionId, prompt)` | Extract goal from prompt |
| `checkGoalAchievement(sessionId, goal, state)` | Check goal completion |

#### Memory System (5 methods)

| Method | Description |
|--------|-------------|
| `storeMemory(sessionId, memory)` | Store memory item |
| `retrieveMemory(sessionId, memoryId)` | Retrieve memory by ID |
| `searchMemories(sessionId, query, tags?, limit?)` | Search memories |
| `getMemoryStats(sessionId)` | Get memory statistics |
| `clearMemories(sessionId, type?)` | Clear memories |

**Total: 53 methods (100% API coverage)**

### Types

See `ts/types.ts` for complete type definitions including:

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

# Build
just build

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
| [sdk/typescript](.) | `@a3s-lab/code` | TypeScript SDK (this package) |
| [sdk/python](../python) | `a3s-code` | Python SDK |

## License

MIT License - see [LICENSE](LICENSE) for details.

---

<p align="center">
  Built by <a href="https://github.com/a3s-lab">A3S Lab</a>
</p>
