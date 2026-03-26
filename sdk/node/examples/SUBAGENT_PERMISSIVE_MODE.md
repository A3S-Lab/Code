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

Monitor sub-agent events directly from the orchestrator handle:

```typescript
import { Agent, Orchestrator } from '@a3s-lab/code';

const agent = await Agent.create('agent.hcl');
const orch = Orchestrator.create(agent);
const handle = orch.spawnSubagent({
  agentType: 'general',
  prompt: 'Use bash to print hello, then explain it.',
  permissive: true,
  maxSteps: 5,
});

const events = handle.events();
while (true) {
  const event = await events.recv(1000);
  if (!event) continue;

  if (event.event_type === 'sub_agent_internal_event' && event.type === 'text_delta') {
    process.stdout.write(event.text ?? '');
  } else if (event.event_type === 'tool_execution_started') {
    console.log('tool args:', event.args);
  } else if (event.event_type === 'tool_execution_completed') {
    console.log('tool durationMs:', event.duration_ms);
  } else if (event.event_type === 'sub_agent_completed') {
    break;
  }
}
```

## Event Types

Current sub-agent event names:

- `sub_agent_started` - SubAgent task started
- `sub_agent_state_changed` - state transition
- `sub_agent_internal_event` - forwarded inner agent event
- `tool_execution_started` - tool call started with parsed `args`
- `tool_execution_completed` - tool call completed with `duration_ms`
- `sub_agent_progress` - SubAgent progress update
- `sub_agent_completed` - SubAgent task completed

Notes:

- Event names use `sub_agent_*`, not `subagent_*`.
- `sub_agent_internal_event` payloads are flattened. A text delta looks like:
  `{ event_type: 'sub_agent_internal_event', type: 'text_delta', text: '...' }`
- `tool_execution_started.args` contains the accumulated tool input JSON.
- `tool_execution_completed.duration_ms` is floored to `1` for very fast tool calls.

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
