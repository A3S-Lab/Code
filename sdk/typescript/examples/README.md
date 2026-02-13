# A3S Code SDK Examples

Examples demonstrating the session-centric API of the A3S Code TypeScript SDK.

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

npm run dev          # Session basics: generateText, streamText, multi-turn
npm run chat         # Multi-turn conversation with session
npm run stream       # All streaming APIs
npm run structured   # Structured output (generateObject, streamObject)
npm run tools        # Tool calling + multi-step agent
npm run kimi-test    # Custom provider (KIMI K2.5)
```

## Architecture

The SDK is session-centric. Every interaction goes through a `Session` object:

```typescript
import { A3sClient, createProvider } from '@a3s-lab/code';

const client = new A3sClient();
const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

// Create session — model and workspace bound here, immutable after
await using session = await client.createSession({
  model: openai('gpt-4o'),
  workspace: '/project',
  system: 'You are a helpful assistant',
});

// All calls go through the session
const { text } = await session.generateText({ prompt: 'Hello' });
const { textStream } = session.streamText({ prompt: 'Explain this' });
const { object } = await session.generateObject({ schema, prompt: 'Extract' });

// Context management
const usage = await session.getContextUsage();
await session.compactContext();

// `await using` auto-closes the session when the block exits
```

Standalone functions (`generateText`, `streamText`, etc.) are convenience wrappers that create a temporary session under the hood — useful for one-shot operations.

## Examples

### Session API (Core)

| Example | Command | Description |
|---------|---------|-------------|
| [simple-test.ts](src/simple-test.ts) | `npm run dev` | Session basics + `using` syntax |
| [chat-simulation.ts](src/chat-simulation.ts) | `npm run chat` | Multi-turn conversation |
| [streaming-demo.ts](src/streaming-demo.ts) | `npm run stream` | streamText + streamObject |
| [structured-generation.ts](src/structured-generation.ts) | `npm run structured` | generateObject + streamObject |
| [tool-calling.ts](src/tool-calling.ts) | `npm run tools` | Tools, maxSteps, onToolCall |

### Advanced Features (Low-Level Client)

| Example | Command | Description |
|---------|---------|-------------|
| [code-review-agent.ts](src/code-review-agent.ts) | `npm run code-review` | Complete agent: permissions, HITL, todos ⭐ |
| [context-management.ts](src/context-management.ts) | `npm run context` | Monitor, compact, clear context |
| [event-streaming.ts](src/event-streaming.ts) | `npm run events` | Real-time event subscription |
| [hitl-confirmation.ts](src/hitl-confirmation.ts) | `npm run hitl` | Human-in-the-loop confirmation |
| [permission-policy.ts](src/permission-policy.ts) | `npm run permission` | Allow/deny/ask permission rules |
| [todo-tracking.ts](src/todo-tracking.ts) | `npm run todo` | Task tracking with priorities |
| [external-tasks.ts](src/external-tasks.ts) | `npm run external-tasks` | Lane handlers, sandbox execution |
| [provider-config.ts](src/provider-config.ts) | `npm run provider` | Provider management |

## Session API Quick Reference

```typescript
// Create
const session = await client.createSession({
  model: openai('gpt-4o'),    // required, immutable
  workspace: '/project',       // optional, immutable
  system: 'You are helpful',   // optional
});

// Generate
const { text, steps } = await session.generateText({ prompt, tools, maxSteps });
const { textStream }  = session.streamText({ prompt, tools, maxSteps });
const { object }       = await session.generateObject({ schema, prompt });
const { partialStream } = session.streamObject({ schema, prompt });

// Context
const usage = await session.getContextUsage();
await session.compactContext();
await session.clearContext();
const messages = await session.getMessages();

// Cleanup
await session.close();
// Or use `await using` for automatic cleanup
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
