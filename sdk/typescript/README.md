# A3S Code TypeScript SDK

<p align="center">
  <strong>TypeScript/JavaScript Client for A3S Code Agent</strong>
</p>

<p align="center">
  <em>Session-centric AI coding agent SDK with Vercel AI SDK-compatible convenience API</em>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/API_Coverage-100%25-brightgreen" alt="API Coverage">
  <img src="https://img.shields.io/badge/Methods-53-blue" alt="Methods">
  <img src="https://img.shields.io/badge/TypeScript-5.3+-blue" alt="TypeScript">
  <img src="https://img.shields.io/badge/License-MIT-green" alt="License">
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> •
  <a href="#session-api">Session API</a> •
  <a href="#tool-calling">Tool Calling</a> •
  <a href="#convenience-api">Convenience API</a> •
  <a href="#api-reference">API Reference</a> •
  <a href="./examples">Examples</a>
</p>

---

## Overview

**@a3s-lab/code** is the official TypeScript SDK for [A3S Code](https://github.com/a3s-lab/a3s). The SDK is session-centric — every interaction goes through a `Session` object:

1. **Session API** — The core pattern. Create a session with `client.createSession()`, then call `session.generateText()`, `session.streamText()`, etc. Model and workspace are bound at creation time and immutable. Supports `await using` for automatic cleanup.

2. **Convenience API** — Standalone functions (`generateText`, `streamText`, etc.) that create temporary sessions under the hood. Great for one-shot operations.

## Installation

```bash
npm install @a3s-lab/code
```

## Quick Start

```typescript
import { A3sClient, createProvider } from '@a3s-lab/code';

const client = new A3sClient({ address: 'localhost:4088' });
const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

// Create session — model and workspace bound here, immutable after
await using session = await client.createSession({
  model: openai('gpt-4o'),
  workspace: '/project',
  system: 'You are a helpful coding assistant.',
});

// Generate text
const { text } = await session.generateText({
  prompt: 'Explain this codebase',
});
console.log(text);

// Stream text
const { textStream } = session.streamText({
  prompt: 'Now refactor it',
});
for await (const chunk of textStream) {
  process.stdout.write(chunk);
}

// Multi-turn: session remembers context
const { text: followUp } = await session.generateText({
  prompt: 'What about error handling?',
});

// Context management
const usage = await session.getContextUsage();
await session.compactContext();
// session.close() called automatically via `await using`
```

## Session API

Sessions are the core concept in A3S Code. Each session binds a model and workspace at creation time (immutable). The session maintains conversation history, context, permissions, and tool state.

### Session Lifecycle

```typescript
import { A3sClient, createProvider } from '@a3s-lab/code';

const client = new A3sClient({ address: 'localhost:4088' });
const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

// Create session — model and workspace are immutable after creation
const session = await client.createSession({
  model: openai('gpt-4o'),
  workspace: '/project',
  system: 'You are a code reviewer.',
});

// Multi-turn conversation (session remembers context)
await session.generateText({ prompt: 'Review the auth module' });
await session.generateText({ prompt: 'What about error handling?' });

// Context management
const usage = await session.getContextUsage();
console.log(`Tokens: ${usage?.totalTokens}`);
await session.compactContext();  // Compress when large

// Cleanup
await session.close();
```

### Auto-Cleanup with `using`

```typescript
// `await using` calls session.close() automatically when the block exits
{
  await using session = await client.createSession({
    model: openai('gpt-4o'),
    workspace: '/project',
  });

  const { text } = await session.generateText({ prompt: 'Hello' });
  // session.close() called automatically here
}
```

### Streaming

```typescript
const { textStream, fullStream, toolStream, text, steps } = session.streamText({
  prompt: 'Explain this codebase',
  tools: { weather: weatherTool },
  maxSteps: 5,
});

// Text-only stream
for await (const chunk of textStream) {
  process.stdout.write(chunk);
}

// Or full event stream
for await (const chunk of fullStream) {
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
const { object, data, usage } = await session.generateObject({
  schema: JSON.stringify({
    type: 'object',
    properties: { summary: { type: 'string' }, files: { type: 'array' } },
  }),
  prompt: 'Analyze this project',
});

// Streaming
const { partialStream, object: finalObject } = session.streamObject({
  schema: '{"type":"object","properties":{"items":{"type":"array"}}}',
  prompt: 'List project dependencies',
});
for await (const partial of partialStream) {
  process.stdout.write(partial);
}
const result = await finalObject;
```

### Events (Low-Level Client)

```typescript
for await (const event of client.subscribeEvents(session.id)) {
  console.log(`[${event.type}] ${event.message}`);
}
```

## Tool Calling

Define client-side tools with `tool()` and enable multi-step agent behavior with `maxSteps`:

```typescript
import { A3sClient, createProvider, tool } from '@a3s-lab/code';

const client = new A3sClient();
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

await using session = await client.createSession({
  model: openai('gpt-4o'),
  system: 'You are a helpful assistant with weather tools.',
});

// Multi-step: model calls tools, gets results, continues reasoning
const { text, steps } = await session.generateText({
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
const { textStream, toolStream } = session.streamText({
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
const { text } = await session.generateText({
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

## Convenience API

Standalone functions that create temporary sessions under the hood. Useful for one-shot operations:

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
});
for await (const chunk of textStream) {
  process.stdout.write(chunk);
}
```

### createChat (Convenience Wrapper)

`createChat()` is a convenience wrapper that manages a session internally:

```typescript
import { createChat, createProvider } from '@a3s-lab/code';

const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

const chat = createChat({
  model: openai('gpt-4o'),
  workspace: '/project',
  system: 'You are a helpful code assistant',
});

const { text } = await chat.send('What does main.rs do?');
const { textStream } = chat.stream('Now refactor it');
for await (const chunk of textStream) {
  process.stdout.write(chunk);
}
await chat.close();
```

For new code, prefer using `Session` directly — it provides the same functionality with more control.

## Message Conversion (UIMessage ↔ ModelMessage)

The SDK provides Vercel AI SDK-style message types for frontend ↔ backend conversion:

- `UIMessage` — Frontend format with `id`, `createdAt`, `parts` (for rendering in chat UIs)
- `ModelMessage` — Backend format with `role`, `content` (for LLM / generateText / streamText)

### Frontend → Backend

```typescript
import { convertToModelMessages, generateText, createProvider } from '@a3s-lab/code';
import type { UIMessage } from '@a3s-lab/code';

const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

// UIMessages from your frontend (e.g., useChat hook, database, etc.)
const uiMessages: UIMessage[] = [
  {
    id: 'msg-1',
    role: 'user',
    content: 'What does main.rs do?',
    parts: [{ type: 'text', text: 'What does main.rs do?' }],
    createdAt: new Date(),
  },
];

// Convert to model format before calling generateText/streamText
const modelMessages = convertToModelMessages(uiMessages);
const { text } = await generateText({
  model: openai('gpt-4o'),
  messages: modelMessages,
});
```

### Backend → Frontend

```typescript
import { convertToUIMessages } from '@a3s-lab/code';
import type { ModelMessage } from '@a3s-lab/code';

// ModelMessages from LLM response or database
const modelMessages: ModelMessage[] = [
  { role: 'user', content: 'Hello' },
  { role: 'assistant', content: 'Hi! How can I help?' },
];

// Convert to UIMessage format for rendering
const uiMessages = convertToUIMessages(modelMessages);
// uiMessages[0].parts → [{ type: 'text', text: 'Hello' }]
// uiMessages[1].parts → [{ type: 'text', text: 'Hi! How can I help?' }]
```

### A3S Message ↔ UIMessage (Shorthand)

```typescript
import { a3sMessagesToUI, uiMessagesToA3s } from '@a3s-lab/code';

// A3S session messages → UIMessage (for rendering)
const messages = await client.getMessages(sessionId);
const uiMessages = a3sMessagesToUI(messages.messages);

// UIMessage → A3S messages (for session generation)
const a3sMessages = uiMessagesToA3s(uiMessages);
await client.generate(sessionId, a3sMessages);
```

### Tool Invocations in UIMessage

UIMessages support rich tool invocation parts for rendering tool calls in chat UIs:

```typescript
const assistantMessage: UIMessage = {
  id: 'msg-2',
  role: 'assistant',
  content: 'The weather in Tokyo is 22°C.',
  parts: [
    {
      type: 'tool-invocation',
      toolInvocation: {
        toolCallId: 'call-1',
        toolName: 'weather',
        args: { city: 'Tokyo' },
        state: 'result',
        result: { temperature: 22, condition: 'sunny' },
      },
    },
    { type: 'text', text: 'The weather in Tokyo is 22°C and sunny.' },
  ],
};

// Converts to: assistant message with toolCalls + tool result message
const modelMessages = convertToModelMessages([assistantMessage]);
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

### Session (Core)

| Method | Description |
|--------|-------------|
| `session.generateText(options)` | Generate text, supports tools + maxSteps |
| `session.streamText(options)` | Stream text, returns `textStream`/`fullStream`/`toolStream` |
| `session.generateObject(options)` | Generate structured JSON output |
| `session.streamObject(options)` | Stream structured JSON output |
| `session.getContextUsage()` | Get context token usage |
| `session.compactContext()` | Compact session context |
| `session.clearContext()` | Clear conversation history |
| `session.getMessages(limit?)` | Get conversation messages |
| `session.close()` | Close session and release resources |
| `session.id` | Session ID (readonly) |
| `session.closed` | Whether session is closed (readonly) |

### Convenience Functions

| Function | Description |
|----------|-------------|
| `generateText(options)` | Generate text (auto session) |
| `streamText(options)` | Stream text (auto session) |
| `generateObject(options)` | Generate structured output (auto session) |
| `streamObject(options)` | Stream structured output (auto session) |
| `createChat(options)` | Create multi-turn chat (auto session) |
| `createProvider(options)` | Create provider factory for model selection |
| `tool(definition)` | Define a client-side tool |

### Message Conversion

| Function | Description |
|----------|-------------|
| `convertToModelMessages(uiMessages)` | UIMessage[] → ModelMessage[] |
| `convertToUIMessages(modelMessages)` | ModelMessage[] → UIMessage[] |
| `a3sMessagesToUI(messages)` | A3S Message[] → UIMessage[] |
| `uiMessagesToA3s(uiMessages)` | UIMessage[] → A3S Message[] |

### Low-Level Client (A3sClient)

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
