# A3S Code SDK Examples

Comprehensive examples demonstrating all features of the A3S Code TypeScript SDK.

## Quick Start

### 1. Start A3S Code Service

```bash
cd /path/to/a3s
./target/debug/a3s-code -d .a3s -w /tmp/a3s-workspace
```

### 2. Run Examples

```bash
cd sdk/typescript/examples
npm install

# Start here
npm run dev          # Session-based + high-level API basics
npm run chat         # Multi-turn conversation (both styles)
npm run stream       # All streaming APIs
npm run structured   # Structured output (both styles)
npm run tools        # Tool calling + multi-step agent
npm run kimi-test    # Custom provider (KIMI K2.5)
```

## Architecture

A3S Code SDK has two API layers:

1. **Session-Based API (`A3sClient`)** — The core pattern. You create sessions, manage their lifecycle, and use them for generation, context management, permissions, HITL, etc. This gives you full control.

2. **High-Level API (Vercel AI SDK-style)** — Convenience wrappers (`generateText`, `streamText`, `createChat`, `tool`, `createProvider`) that automatically manage sessions under the hood. Great for quick prototyping and simple use cases.

Most examples demonstrate **both styles** so you can choose what fits your needs.

## Examples

### Core Examples (Both API Styles)

These examples show session-based usage first, then the equivalent high-level API.

| Example | Command | Description |
|---------|---------|-------------|
| [simple-test.ts](src/simple-test.ts) | `npm run dev` | Session-based generation + `generateText()`/`streamText()` |
| [chat-simulation.ts](src/chat-simulation.ts) | `npm run chat` | Session-based chat + `createChat()` with tools |
| [streaming-demo.ts](src/streaming-demo.ts) | `npm run stream` | `streamGenerate()` + `streamText()`/`streamObject()` |
| [structured-generation.ts](src/structured-generation.ts) | `npm run structured` | `generateStructured()` + `generateObject()`/`streamObject()` |
| [provider-config.ts](src/provider-config.ts) | `npm run provider` | `createProvider()` + low-level provider management |

### High-Level API Showcases

These examples focus on the Vercel AI SDK-style API.

| Example | Command | Description |
|---------|---------|-------------|
| [tool-calling.ts](src/tool-calling.ts) | `npm run tools` | `tool()`, `maxSteps`, `onToolCall`, `onStepFinish` |
| [kimi-test.ts](src/kimi-test.ts) | `npm run kimi-test` | `createProvider()` with custom endpoint |

### Advanced Features (Session-Based)

These examples use `A3sClient` directly for features that require session-level control.

| Example | Command | Description |
|---------|---------|-------------|
| [code-review-agent.ts](src/code-review-agent.ts) | `npm run code-review` | Complete agent: permissions, HITL, todos, context ⭐ |
| [context-management.ts](src/context-management.ts) | `npm run context` | Monitor, compact, clear context |
| [event-streaming.ts](src/event-streaming.ts) | `npm run events` | Real-time event subscription |
| [hitl-confirmation.ts](src/hitl-confirmation.ts) | `npm run hitl` | Human-in-the-loop confirmation |
| [permission-policy.ts](src/permission-policy.ts) | `npm run permission` | Allow/deny/ask permission rules |
| [todo-tracking.ts](src/todo-tracking.ts) | `npm run todo` | Task tracking with priorities |
| [external-tasks.ts](src/external-tasks.ts) | `npm run external-tasks` | Lane handlers, sandbox execution |
| [storage-configuration.ts](src/storage-configuration.ts) | `npm run storage` | Memory vs File storage |
| [skill-management.ts](src/skill-management.ts) | `npm run skills` | Load/unload skills |
| [memory-demo.ts](src/memory-demo.ts) | — | Memory system APIs |
| [planning-demo.ts](src/planning-demo.ts) | — | Planning & goal tracking |

## API Quick Reference

### Session-Based (Core)

```typescript
import { A3sClient } from '@a3s-lab/code';

const client = new A3sClient({ address: 'localhost:4088' });

// Create session
const { sessionId } = await client.createSession({
  name: 'my-session',
  workspace: '/project',
  systemPrompt: 'You are a helpful assistant.',
  llm: { provider: 'openai', model: 'gpt-4o', apiKey: 'sk-xxx' },
});

// Generate
const response = await client.generate(sessionId, [
  { role: 'user', content: 'Hello' },
]);

// Stream
for await (const chunk of client.streamGenerate(sessionId, messages)) {
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

```typescript
import {
  generateText, streamText, generateObject, streamObject,
  createChat, createProvider, tool,
} from '@a3s-lab/code';

const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

// One-shot (auto session)
const { text } = await generateText({
  model: openai('gpt-4o'),
  prompt: 'Hello',
});

// Streaming (auto session)
const { textStream } = streamText({
  model: openai('gpt-4o'),
  prompt: 'Hello',
});
for await (const chunk of textStream) process.stdout.write(chunk);

// Tool calling
const { text, steps } = await generateText({
  model: openai('gpt-4o'),
  prompt: 'What is the weather?',
  tools: {
    weather: tool({
      description: 'Get weather',
      parameters: { type: 'object', properties: { city: { type: 'string' } } },
      execute: async ({ city }) => ({ city, temp: 72 }),
    }),
  },
  maxSteps: 5,
});

// Multi-turn chat (auto session)
const chat = createChat({ model: openai('gpt-4o'), workspace: '/project' });
const { text: reply } = await chat.send('Hello');
await chat.close();

// Structured output (auto session)
const { object } = await generateObject({
  model: openai('gpt-4o'),
  schema: JSON.stringify({ type: 'object', properties: { name: { type: 'string' } } }),
  prompt: 'Extract the name',
});
```

## Prerequisites

1. **A3S Code Agent running** on `localhost:4088` (or set `A3S_ADDRESS`)
2. **Node.js 18+**
3. **Configuration** in `a3s/.a3s/config.json` with provider/model settings

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `A3S_ADDRESS` | Agent gRPC address | `localhost:4088` |
| `OPENAI_API_KEY` | OpenAI API key | — |
| `ANTHROPIC_API_KEY` | Anthropic API key | — |

## Troubleshooting

**Connection Refused** — Make sure A3S Code Agent is running:
```bash
./target/debug/a3s-code -d .a3s -w /tmp/workspace
```

**Module Not Found** — Install dependencies:
```bash
npm install
```

## Learn More

- [SDK Documentation](../README.md)
- [API Reference](../README.md#api-reference)

## License

MIT
