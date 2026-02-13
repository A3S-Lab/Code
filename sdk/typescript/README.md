# A3S Code TypeScript SDK

<p align="center">
  <strong>TypeScript/JavaScript Client for A3S Code Agent</strong>
</p>

<p align="center">
  <em>Session-based AI coding agent SDK with Vercel AI SDK-compatible convenience API</em>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/API_Coverage-100%25-brightgreen" alt="API Coverage">
  <img src="https://img.shields.io/badge/Methods-53-blue" alt="Methods">
  <img src="https://img.shields.io/badge/TypeScript-5.3+-blue" alt="TypeScript">
  <img src="https://img.shields.io/badge/License-MIT-green" alt="License">
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> •
  <a href="#session-based-api">Session API</a> •
  <a href="#high-level-api">High-Level API</a> •
  <a href="#tool-calling">Tool Calling</a> •
  <a href="#api-reference">API Reference</a> •
  <a href="./examples">Examples</a>
</p>

---

## Overview

**@a3s-lab/code** is the official TypeScript SDK for [A3S Code](https://github.com/a3s-lab/a3s). It provides:

1. **Session-Based API (`A3sClient`)** — The core pattern. Create sessions, manage their lifecycle, and use them for generation, context management, permissions, HITL, and more. Full control over every aspect.

2. **High-Level API (Vercel AI SDK-style)** — Convenience wrappers (`generateText`, `streamText`, `createChat`, `tool`, `createProvider`) that manage sessions automatically. Great for quick prototyping.

## Installation

```bash
npm install @a3s-lab/code
```

## Quick Start

### Session-Based (Core Pattern)

```typescript
import { A3sClient } from '@a3s-lab/code';

const client = new A3sClient({ address: 'localhost:4088' });

// Create a session
const { sessionId } = await client.createSession({
  name: 'my-session',
  workspace: '/project',
  systemPrompt: 'You are a helpful coding assistant.',
  llm: { provider: 'openai', model: 'gpt-4o', apiKey: 'sk-xxx' },
});

// Generate
const response = await client.generate(sessionId, [
  { role: 'user', content: 'Explain this codebase' },
]);
console.log(response.message?.content);

// Stream
for await (const chunk of client.streamGenerate(sessionId, [
  { role: 'user', content: 'Now refactor it' },
])) {
  if (chunk.type === 'content') process.stdout.write(chunk.content);
}

// Context management
const usage = await client.getContextUsage(sessionId);
await client.compactContext(sessionId);

// Cleanup
await client.destroySession(sessionId);
client.close();
```

### High-Level (Vercel AI SDK-style)

Same operations, but sessions are managed automatically:

```typescript
import { generateText, streamText, createProvider } from '@a3s-lab/code';

const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

// One-shot generation (auto session)
const { text } = await generateText({
  model: openai('gpt-4o'),
  prompt: 'Explain this codebase',
  workspace: '/project',
});

// Streaming (auto session)
const { textStream } = streamText({
  model: openai('gpt-4o'),
  prompt: 'Explain this codebase',
  workspace: '/project',
});
for await (const chunk of textStream) {
  process.stdout.write(chunk);
}
```

## Session-Based API

Sessions are the core concept in A3S Code. Each session maintains conversation history, context, permissions, and tool state.

### Session Lifecycle

```typescript
import { A3sClient, StorageType } from '@a3s-lab/code';

const client = new A3sClient({ address: 'localhost:4088' });

// Create with full configuration
const { sessionId } = await client.createSession({
  name: 'code-review',
  workspace: '/project',
  systemPrompt: 'You are a code reviewer.',
  storageType: StorageType.STORAGE_TYPE_FILE,  // Persistent
  autoCompact: true,
  llm: {
    provider: 'anthropic',
    model: 'claude-sonnet-4-20250514',
    apiKey: 'sk-ant-xxx',
  },
});

// Multi-turn conversation (session remembers context)
await client.generate(sessionId, [
  { role: 'user', content: 'Review the auth module' },
]);
await client.generate(sessionId, [
  { role: 'user', content: 'What about error handling?' },  // Knows about auth module
]);

// Context management
const usage = await client.getContextUsage(sessionId);
console.log(`Tokens: ${usage.usage?.totalTokens}`);
await client.compactContext(sessionId);  // Compress when large

// Session control
await client.pause(sessionId);
await client.resume(sessionId);
await client.cancel(sessionId);

// Cleanup
await client.destroySession(sessionId);
```

### Streaming

```typescript
for await (const chunk of client.streamGenerate(sessionId, messages)) {
  switch (chunk.type) {
    case 'content':
      process.stdout.write(chunk.content);
      break;
    case 'tool_call':
      console.log(`Tool: ${chunk.toolCall?.name}`);
      break;
    case 'done':
      console.log(`\nFinish: ${chunk.finishReason}`);
      break;
  }
}
```

### Structured Output

```typescript
// Unary
const response = await client.generateStructured(sessionId, messages, schemaJson);
const data = JSON.parse(response.data);

// Streaming
for await (const chunk of client.streamGenerateStructured(sessionId, messages, schemaJson)) {
  process.stdout.write(chunk.data);
}
```

### Events

```typescript
for await (const event of client.subscribeEvents(sessionId)) {
  console.log(`[${event.type}] ${event.message}`);
}
```

## High-Level API

The high-level API wraps sessions automatically. Use `createProvider()` to configure models, then call `generateText()`, `streamText()`, etc.

### Provider Configuration

```typescript
import { createProvider } from '@a3s-lab/code';

// OpenAI
const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });
const gpt4 = openai('gpt-4o');

// Anthropic
const anthropic = createProvider({ name: 'anthropic', apiKey: 'sk-ant-xxx' });
const claude = anthropic('claude-sonnet-4-20250514');

// Custom endpoint (KIMI, local models, etc.)
const kimi = createProvider({
  name: 'kimi',
  apiKey: 'sk-xxx',
  baseUrl: 'http://your-endpoint/v1',
});
const k2 = kimi('k2.5');
```

## Tool Calling

Define client-side tools with `tool()` and enable multi-step agent behavior with `maxSteps`:

```typescript
import { generateText, createProvider, tool } from '@a3s-lab/code';

const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

const weather = tool({
  description: 'Get weather for a city',
  parameters: {
    type: 'object',
    properties: {
      city: { type: 'string', description: 'City name' },
    },
    required: ['city'],
  },
  execute: async ({ city }) => ({
    city,
    temperature: 72,
    condition: 'sunny',
  }),
});

// Multi-step: model calls tools, gets results, continues reasoning
const { text, steps } = await generateText({
  model: openai('gpt-4o'),
  prompt: 'What is the weather in Tokyo and Paris?',
  tools: { weather },
  maxSteps: 5,
  onStepFinish: (step) => {
    console.log(`Step ${step.stepIndex}: ${step.toolCalls.length} tool calls`);
  },
  onToolCall: ({ toolName, args }) => {
    console.log(`Calling ${toolName}`, args);
  },
});

console.log(text);
console.log(`Completed in ${steps.length} steps`);
```

### Streaming with Tools

```typescript
const { textStream, toolStream } = streamText({
  model: openai('gpt-4o'),
  prompt: 'Check the weather everywhere',
  tools: { weather },
  maxSteps: 5,
});

for await (const chunk of textStream) {
  process.stdout.write(chunk);
}
```

### Tools Without Execute (onToolCall)

```typescript
const { text } = await generateText({
  model: openai('gpt-4o'),
  prompt: 'Look up the user profile',
  tools: {
    getUser: tool({
      description: 'Get user profile by ID',
      parameters: {
        type: 'object',
        properties: { userId: { type: 'string' } },
      },
      // No execute — handled by onToolCall
    }),
  },
  maxSteps: 3,
  onToolCall: async ({ toolName, args }) => {
    if (toolName === 'getUser') {
      return { name: 'Alice', role: 'admin' };
    }
  },
});
```

## Multi-Turn Chat

`createChat()` manages a persistent session for multi-turn conversations:

```typescript
import { createChat, createProvider, tool } from '@a3s-lab/code';

const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

const chat = createChat({
  model: openai('gpt-4o'),
  workspace: '/project',
  system: 'You are a helpful code assistant',
  tools: {
    search: tool({
      description: 'Search the codebase',
      parameters: {
        type: 'object',
        properties: { query: { type: 'string' } },
      },
      execute: async ({ query }) => ({ results: [`Found: ${query}`] }),
    }),
  },
  maxSteps: 5,
});

// Send and get complete response
const { text, steps } = await chat.send('What does main.rs do?');
console.log(text);

// Stream the response
const { textStream } = chat.stream('Now refactor it');
for await (const chunk of textStream) {
  process.stdout.write(chunk);
}

// Context management
const usage = await chat.getUsage();
console.log(`Tokens used: ${usage?.totalTokens}`);

await chat.compact(); // Compress context when it gets large
await chat.close();   // Clean up
```

## Structured Output

```typescript
import { generateObject, streamObject, createProvider } from '@a3s-lab/code';

const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

// Generate a typed object
const { object } = await generateObject({
  model: openai('gpt-4o'),
  schema: JSON.stringify({
    type: 'object',
    properties: {
      summary: { type: 'string' },
      files: { type: 'array', items: { type: 'string' } },
      complexity: { type: 'string', enum: ['low', 'medium', 'high'] },
    },
    required: ['summary', 'files', 'complexity'],
  }),
  prompt: 'Analyze this project structure',
  workspace: '/project',
});

// Stream partial results
const { partialStream, object: finalObject } = streamObject({
  model: openai('gpt-4o'),
  schema: '{"type":"object","properties":{"items":{"type":"array"}}}',
  prompt: 'List all project dependencies',
});

for await (const partial of partialStream) {
  process.stdout.write(partial);
}
const result = await finalObject;
```

## Configuration

### Using Real LLM APIs

The SDK requires a running A3S Code service. See [examples/TESTING_WITH_REAL_MODELS.md](./examples/TESTING_WITH_REAL_MODELS.md) for detailed setup.

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

3. **Use SDK:**

```typescript
import { A3sClient, loadConfigFromDir } from '@a3s-lab/code';

const config = loadConfigFromDir('/path/to/a3s/.a3s');
const client = new A3sClient({
  address: config.address || 'localhost:4088',
  configDir: '/path/to/a3s/.a3s',
});

// Create session — uses default model from config
const { sessionId } = await client.createSession({
  name: 'my-session',
  workspace: '/tmp/workspace',
});

// Or specify model explicitly
const { sessionId: s2 } = await client.createSession({
  name: 'my-session',
  workspace: '/tmp/workspace',
  llm: { provider: 'openai', model: 'kimi-k2.5' },
});
```

### Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `A3S_ADDRESS` | gRPC server address | `localhost:4088` |
| `A3S_CONFIG_DIR` | Configuration directory | - |

## API Reference

### High-Level API (Vercel AI SDK-style)

#### Core Functions

| Function | Description |
|----------|-------------|
| `generateText(options)` | Generate text (non-streaming), auto session management |
| `streamText(options)` | Stream text generation, returns `textStream`/`fullStream`/`toolStream` |
| `generateObject(options)` | Generate structured JSON output |
| `streamObject(options)` | Stream structured JSON output |
| `createChat(options)` | Create multi-turn chat with persistent session |
| `createProvider(options)` | Create provider factory for model selection |
| `tool(definition)` | Define a client-side tool with type safety |

#### Options

| Option | Type | Description |
|--------|------|-------------|
| `model` | `ModelRef` | Model reference from `createProvider()` |
| `prompt` | `string` | Simple text prompt |
| `messages` | `MessageInput[]` | Full message array for multi-turn |
| `system` | `string` | System prompt |
| `workspace` | `string` | Working directory |
| `tools` | `ToolSet` | Client-side tool definitions |
| `maxSteps` | `number` | Max generation + tool execution steps (default: 1) |
| `onStepFinish` | `(step) => void` | Called after each step completes |
| `onToolCall` | `(event) => void \| result` | Called when model invokes a tool |
| `server` | `A3sClientOptions` | gRPC server connection options |

#### Result Types

| Property | Available On | Description |
|----------|-------------|-------------|
| `text` | `generateText`, `streamText` | Generated text (string or Promise) |
| `textStream` | `streamText`, `chat.stream` | AsyncIterable of text chunks |
| `fullStream` | `streamText`, `chat.stream` | AsyncIterable of all event chunks |
| `toolStream` | `streamText`, `chat.stream` | AsyncIterable of tool call events |
| `toolCalls` | `generateText`, `chat.send` | Array of tool calls made |
| `steps` | `generateText`, `streamText`, `chat` | All step results |
| `usage` | all | Token usage statistics |
| `finishReason` | all | Why generation stopped |
| `object` | `generateObject`, `streamObject` | Parsed JSON object |

### Session-Based Client (A3sClient)

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
