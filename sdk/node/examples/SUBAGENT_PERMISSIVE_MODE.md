# SubAgent Permissive Mode - Node.js Examples

## Overview

Version 1.0.3 introduces permissive mode support for sub-agents, allowing them to execute tools autonomously without human-in-the-loop (HITL) confirmation.

## Basic Usage

### TypeScript

```typescript
import { Agent } from '@a3s-lab/code';

const agent = await Agent.create('~/.a3s/config.hcl');
const session = agent.session('.', { permissive: true });

// Spawn a sub-agent with permissive mode
const result = await session.tool('task', {
  agent: 'general',
  description: 'Analyze code',
  prompt: 'Use glob and read tools to analyze Python files',
  permissive: true,  // ← Sub-agent runs without HITL
  max_steps: 10
});

console.log(result.output);
```

### JavaScript

```javascript
const { Agent } = require('@a3s-lab/code');

async function main() {
  const agent = await Agent.create('~/.a3s/config.hcl');
  const session = agent.session('.', { permissive: true });

  // Spawn a sub-agent with permissive mode
  const result = await session.tool('task', {
    agent: 'general',
    description: 'Analyze code',
    prompt: 'Use glob and read tools to analyze Python files',
    permissive: true,
    max_steps: 10
  });

  console.log(result.output);
}

main();
```

## Parallel Sub-Agents

```typescript
import { Agent } from '@a3s-lab/code';

const agent = await Agent.create('~/.a3s/config.hcl');
const session = agent.session('.', { permissive: true });

// Spawn multiple sub-agents in parallel
const result = await session.tool('parallel_task', {
  tasks: [
    {
      agent: 'explore',
      description: 'Count Python files',
      prompt: 'Find all .py files',
      permissive: true,
      max_steps: 3
    },
    {
      agent: 'explore',
      description: 'Count Rust files',
      prompt: 'Find all .rs files',
      permissive: true,
      max_steps: 3
    }
  ]
});

console.log(result.output);
```

## SubAgent Event Streaming

Monitor internal SubAgent events (tool calls, LLM responses):

```typescript
import { Agent } from '@a3s-lab/code';

const agent = await Agent.create('~/.a3s/config.hcl');
const session = agent.session('.', { permissive: true });

// Stream events and monitor SubAgent activity
const stream = await session.stream(
  'Use task tool to spawn a general agent. ' +
  'Ask it to analyze code with permissive=true.'
);

for await (const event of stream) {
  if (event.type === 'subagent_start') {
    console.log('SubAgent started:', event.toolName);
  } else if (event.type === 'subagent_end') {
    console.log('SubAgent ended');
  } else if (event.type === 'tool_start') {
    console.log('Tool call:', event.toolName);
  }
}
```

## Event Types

New event types in v1.0.3:

- `subagent_start` - SubAgent task started
- `subagent_end` - SubAgent task completed
- `subagent_progress` - SubAgent progress update
- `tool_input_delta` - Streaming tool input (partial JSON arguments)

## Use Cases

### Automated Testing

```typescript
// Run tests autonomously without HITL confirmation
const result = await session.tool('task', {
  agent: 'general',
  description: 'Run tests',
  prompt: 'Use bash tool to run npm test',
  permissive: true,
  max_steps: 5
});
```

### CI/CD Pipelines

```typescript
// Deploy code autonomously in CI environment
const result = await session.tool('task', {
  agent: 'general',
  description: 'Deploy application',
  prompt: 'Use bash tool to run deployment script',
  permissive: true,
  max_steps: 10
});
```

### Batch Processing

```typescript
// Process multiple files in parallel
const result = await session.tool('parallel_task', {
  tasks: files.map(file => ({
    agent: 'general',
    description: `Process ${file}`,
    prompt: `Analyze and transform ${file}`,
    permissive: true,
    max_steps: 5
  }))
});
```

## API Reference

### TaskParams

```typescript
interface TaskParams {
  agent: string;           // Agent type: 'general', 'explore', etc.
  description: string;     // Short task description
  prompt: string;          // Task prompt for the sub-agent
  permissive?: boolean;    // Enable autonomous execution (default: false)
  max_steps?: number;      // Maximum tool execution rounds (default: 20)
}
```

### ParallelTaskParams

```typescript
interface ParallelTaskParams {
  tasks: TaskParams[];     // Array of tasks to execute in parallel
}
```

## Related

- [GitHub Issue #2](https://github.com/A3S-Lab/Code/issues/2) - Original feature request
- [Python SDK Examples](../python/examples/test_permissive_subagents.py)
- [Release Notes v1.0.3](https://github.com/A3S-Lab/Code/releases/tag/v1.0.3)
