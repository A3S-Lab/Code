# AgenticLoop & Skills System API Design

## Design Principles

1. **Session-centric** — Every interaction goes through a Session
2. **Always agentic** — Every `send()` can trigger the AgenticLoop (plan → execute → reflect)
3. **Batteries included** — Built-in tools, agents, and skills auto-loaded; users extend, not build from scratch
4. **Progressive disclosure** — Skills load lazily, only names/descriptions in context
5. **Event-driven** — All operations emit events for real-time UI
6. **HITL-native** — Human approval integrated at every level
7. **Lane-based** — Each session has its own priority queue; tasks can be handled internally or externally (distributed)

---

## 0. Built-in Capabilities (Auto-Loaded)

A3S Code automatically loads a set of built-in tools, agents, and skills
for every session. Users don't need to configure these — they're available
out of the box.

### Built-in Tools (Server-Side)

These tools are always available in every session. The agent calls them
as needed during the AgenticLoop.

| Category | Tools | Description |
|----------|-------|-------------|
| File I/O | `Read`, `Write`, `Edit` | Read, write, and edit files |
| Search | `Grep`, `Glob`, `Find`, `Ls` | Search and navigate the codebase |
| Shell | `Bash` | Execute shell commands |
| Subagent | `Task` | Delegate to a subagent |
| Skills | `FindSkills`, `UseSkill` | Progressive skill discovery and loading |
| Context | `TodoRead`, `TodoWrite` | Task tracking within the session |

### Built-in Agents (Subagents)

Pre-configured agents with restricted permissions. The primary agent
delegates to these via the `Task` tool automatically.

| Agent | Description | Permissions | Max Steps |
|-------|-------------|-------------|-----------|
| `explore` | Read-only codebase exploration | Read, Grep, Glob, Ls | 20 |
| `plan` | Read-only planning and analysis | Read, Grep, Glob, Ls | 30 |
| `general` | Multi-step task execution | Read, Write, Edit, Bash | 50 |

### Built-in Skills (Auto-Discovered)

Skills from `~/.config/a3s/skills/` (user-level) and `.a3s/skills/`
(project-level) are auto-discovered. Only names + descriptions are
injected into the system prompt (~30-50 tokens each). Full content
is loaded on-demand via `FindSkills` / `UseSkill` tools.

---

## 1. Session Creation (Full Configuration)

```typescript
import { A3sClient, createProvider } from '@a3s-lab/code';

const client = new A3sClient();
const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

// Full session configuration
await using session = await client.createSession({
  // Required: model (immutable after creation)
  model: openai('gpt-4o'),

  // Optional: workspace (immutable after creation)
  workspace: '/project',

  // Optional: system prompt
  system: 'You are a senior software engineer.',

  // Optional: additional skills to load (on top of auto-discovered ones)
  skills: [
    '/path/to/custom-skills/',           // Directory
    'github-commands',                    // By name (from auto-discovered)
    {                                     // Inline definition
      name: 'deploy-rules',
      description: 'Production deployment safety rules',
      content: 'Always run tests before deploying...',
      allowedTools: ['Bash(npm:*)', 'Bash(git:*)'],
    },
  ],

  // Optional: HITL confirmation policy
  confirmation: {
    requireConfirmation: ['Bash', 'Write', 'Edit'],
    autoApprove: ['Read', 'Grep', 'Glob', 'Ls'],
    timeout: 30_000,
    timeoutAction: 'reject',
  },

  // Optional: permission policy
  permissions: {
    defaultAction: 'allow',
    rules: [
      { tool: 'Bash', pattern: 'rm -rf*', action: 'deny' },
      { tool: 'Bash', pattern: 'sudo*', action: 'deny' },
    ],
  },

  // Optional: lane handler overrides
  lanes: {
    execute: { mode: 'external', timeout: 120_000 },
  },

  // Optional: context management
  autoCompact: true,
  autoCompactThreshold: 0.8,
});
```

### SessionCreateOptions (Updated)

```typescript
interface SessionCreateOptions {
  /** Model reference (immutable after creation) */
  model: ModelRef;
  /** Working directory (immutable after creation) */
  workspace?: string;
  /** System prompt */
  system?: string;
  /** Session ID override */
  sessionId?: string;
  /** Initial context messages */
  initialContext?: MessageInput[];

  // --- New fields ---

  /** Skills to load (directories, names, or inline definitions) */
  skills?: Array<string | SkillDefinition>;
  /** HITL confirmation policy */
  confirmation?: ConfirmationPolicy;
  /** Tool permission policy */
  permissions?: PermissionPolicy;
  /** Lane handler overrides */
  lanes?: Partial<Record<LaneName, LaneHandlerConfig>>;
  /** Enable auto-compaction */
  autoCompact?: boolean;
  /** Auto-compact threshold (0.0-1.0) */
  autoCompactThreshold?: number;
}

type LaneName = 'control' | 'query' | 'execute' | 'generate';
```

---

## 2. AgenticLoop (Triggered by Every send())

The key insight: **every `session.send()` can trigger the AgenticLoop**.
When the model decides to call tools, the session automatically enters
the agentic loop (generate → tool call → execute → reflect → repeat).

There is no separate `session.run()` — `send()` IS the agentic entry point.

### Basic Usage

```typescript
await using session = await client.createSession({
  model: openai('gpt-4o'),
  workspace: '/project',
});

// Simple question — model answers directly, no tools needed
const { text } = await session.send('What is TypeScript?');

// Complex task — model enters AgenticLoop automatically
// (reads files, greps code, edits files, runs tests, etc.)
const { text, steps, toolCalls } = await session.send(
  'Refactor the auth module to use JWT',
);
console.log(`Completed in ${steps.length} steps, ${toolCalls.length} tool calls`);

// Follow-up — session remembers context, may enter AgenticLoop again
const { text: followUp } = await session.send(
  'Now add unit tests for the changes you made',
);
```

### Streaming

```typescript
// Stream the entire interaction (including AgenticLoop events)
const { eventStream, result } = session.sendStream(
  'Fix all TODO comments in src/',
);

for await (const event of eventStream) {
  switch (event.type) {
    case 'text':
      process.stdout.write(event.content);
      break;
    case 'tool_call':
      console.log(`\n🔧 ${event.toolName}(${JSON.stringify(event.args)})`);
      break;
    case 'tool_result':
      console.log(`   → ${event.success ? '✅' : '❌'} ${event.output.slice(0, 80)}`);
      break;
    case 'step_finish':
      console.log(`\n--- Step ${event.stepIndex} ---`);
      break;
    case 'plan':
      console.log('\n📋 Plan:');
      event.steps.forEach((s, i) => console.log(`  ${i + 1}. ${s.description}`));
      break;
    case 'reflection':
      console.log(`\n🔍 Confidence: ${event.confidence}`);
      break;
    case 'confirmation_required':
      console.log(`\n⚠️  Approve ${event.toolName}?`);
      break;
    case 'subagent_start':
      console.log(`\n🤖 → ${event.agentName}: ${event.task}`);
      break;
    case 'context_compact':
      console.log(`\n📦 ${event.beforeTokens} → ${event.afterTokens} tokens`);
      break;
    case 'done':
      console.log(`\n✅ ${event.finishReason}`);
      break;
  }
}

const finalResult = await result;
```

### send() Options

```typescript
interface SendOptions {
  /** Maximum agent loop iterations. @default 50 */
  maxSteps?: number;

  /** Execution strategy. @default 'auto' */
  strategy?: 'direct' | 'planned' | 'iterative' | 'parallel' | 'auto';

  /** Enable reflection after tool failures. @default true */
  reflection?: boolean;

  /** Enable planning before execution. @default 'auto' */
  planning?: boolean | 'auto';

  /** Additional client-side tools (on top of built-in server tools) */
  tools?: ToolSet;

  /** Abort signal for cancellation */
  signal?: AbortSignal;

  // Callbacks
  onStepFinish?: (step: StepResult) => void | Promise<void>;
  onToolCall?: (event: ToolCallEvent) => void | unknown | Promise<void | unknown>;
  onEvent?: (event: AgentLoopEvent) => void | Promise<void>;
  onConfirmation?: (request: ConfirmationRequest) => boolean | Promise<boolean>;
}

interface SendResult {
  /** Final text output */
  text: string;
  /** All steps executed */
  steps: StepResult[];
  /** All tool calls made */
  toolCalls: ToolCall[];
  /** Token usage */
  usage: Usage;
  /** Why the loop stopped */
  finishReason: 'stop' | 'max_steps' | 'cancelled' | 'error';
  /** Execution plan (if planning was used) */
  plan?: ExecutionPlan;
}

interface SendStreamResult {
  /** Real-time event stream */
  eventStream: AsyncIterable<AgentLoopEvent>;
  /** Promise resolving to final result */
  result: Promise<SendResult>;
}
```

### How send() Decides to Enter AgenticLoop

```
session.send("What is TypeScript?")
  ↓
  LLM generates response
  ↓
  No tool calls → return text directly (1 step)

session.send("Refactor auth to use JWT")
  ↓
  LLM generates response with tool calls (Read, Grep, etc.)
  ↓
  Enter AgenticLoop:
    Step 1: Read src/auth.ts → tool result
    Step 2: LLM analyzes → calls Edit to modify file
    Step 3: Edit src/auth.ts → tool result
    Step 4: LLM calls Bash("npm test") → tool result
    Step 5: Tests pass → LLM generates final summary
  ↓
  Return result (5 steps, 4 tool calls)
```

## 3. Session Lane Queue (Core Infrastructure)

Every Session has its own A3S Lane queue instance. The Lane queue routes ALL
atomic tasks (tool calls, LLM generations, subagent dispatches) to priority
lanes. Every task scheduled through the Lane queue is externally extensible
via three execution modes:

- **Internal**: Execute in-process (default)
- **External**: Register task, wait for external worker to complete it
- **Hybrid**: Execute in-process but also emit events for monitoring

This means ANY task — file reads, shell commands, LLM calls, subagent work —
can be offloaded to external workers on other machines for distributed parallel
processing. The API allows external systems to poll for pending tasks, execute
them in their own environment, and return results.

### Lane Architecture

```
Session
  └── LaneQueue
        ├── Control Lane  (priority: highest) — cancel, pause, resume
        ├── Query Lane    (priority: high)    — read, grep, glob, ls
        ├── Execute Lane  (priority: high)    — write, edit, bash
        └── Generate Lane (priority: normal)  — LLM generation
```

### Configuring Lane Handlers

```typescript
await using session = await client.createSession({
  model: openai('gpt-4o'),
  workspace: '/project',
});

// Default: all lanes use internal execution
// Override specific lanes to external mode:
await session.setLaneHandler('execute', {
  mode: 'external',       // Tasks wait for external worker
  timeout: 60_000,        // Max wait time for external completion
  timeoutAction: 'fallback-internal',  // Fall back to internal on timeout
});

// Or hybrid: execute internally but also emit events
await session.setLaneHandler('query', {
  mode: 'hybrid',
});

// Get current handler config
const handler = await session.getLaneHandler('execute');
console.log(handler.mode); // 'external'
```

### External Task Processing (Worker Side)

External workers poll for pending tasks and complete them. This enables
distributing heavy tasks (bash builds, test runs, deployments) to other
machines or containers.

```typescript
// === Worker process (can be on a different machine) ===
import { A3sClient } from '@a3s-lab/code';

const worker = new A3sClient({ address: 'agent-host:4088' });

// Poll for pending external tasks on a session
async function processExternalTasks(sessionId: string) {
  while (true) {
    const { tasks } = await worker.listPendingExternalTasks(sessionId);

    for (const task of tasks) {
      console.log(`Processing: ${task.toolName}(${JSON.stringify(task.args)})`);

      try {
        // Execute the task (on this machine, in a sandbox, etc.)
        const output = await executeLocally(task);

        // Report result back
        await worker.completeExternalTask(sessionId, task.id, {
          success: true,
          output,
        });
      } catch (err) {
        await worker.completeExternalTask(sessionId, task.id, {
          success: false,
          error: err.message,
        });
      }
    }

    await sleep(1000); // Poll interval
  }
}

// Or use event stream for real-time task notifications
for await (const event of worker.subscribeEvents(sessionId)) {
  if (event.type === 'external_task_pending') {
    const task = event.task;
    // Process immediately without polling
    const result = await executeLocally(task);
    await worker.completeExternalTask(sessionId, task.id, result);
  }
}
```

### Session-Level External Task API

```typescript
// From the session object (convenience wrappers)
const pending = await session.listPendingTasks();
console.log(`${pending.length} tasks waiting for external processing`);

// Complete a task
await session.completeTask(taskId, {
  success: true,
  output: 'Build succeeded',
});

// Get queue stats
const stats = await session.getQueueStats();
console.log(`Control: ${stats.control.pending} pending`);
console.log(`Query: ${stats.query.pending} pending, ${stats.query.active} active`);
console.log(`Execute: ${stats.execute.pending} pending, ${stats.execute.external} external`);
console.log(`Generate: ${stats.generate.pending} pending`);
console.log(`Dead letters: ${stats.deadLetters}`);
```

### Distributed Parallel Processing Example

```typescript
// === Orchestrator ===
const client = new A3sClient();
const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

await using session = await client.createSession({
  model: openai('gpt-4o'),
  workspace: '/project',
});

// Route bash commands to external workers (e.g., CI machines)
await session.setLaneHandler('execute', {
  mode: 'external',
  timeout: 300_000,  // 5 min for builds
});

// Agent runs normally — when it calls Bash/Write/Edit,
// those tasks are queued as external tasks
const { eventStream, result } = session.sendStream(
  'Run the test suite on all platforms and fix any failures',
);

for await (const event of eventStream) {
  if (event.type === 'text') process.stdout.write(event.content);
  if (event.type === 'external_task_pending') {
    console.log(`\n⏳ Waiting for external: ${event.task.toolName}`);
  }
  if (event.type === 'external_task_completed') {
    console.log(`\n✅ External completed: ${event.task.toolName}`);
  }
}

// === Worker 1 (Linux CI) ===
// Picks up Bash tasks, runs them in Linux environment
processExternalTasks(session.id, { filter: 'linux' });

// === Worker 2 (macOS CI) ===
// Picks up Bash tasks, runs them in macOS environment
processExternalTasks(session.id, { filter: 'macos' });

// === Worker 3 (Windows CI) ===
processExternalTasks(session.id, { filter: 'windows' });
```

### Lane Types and Auto-Routing

```typescript
// Tasks are auto-routed to lanes by tool name:
// Control: cancel, pause, resume
// Query:   Read, Grep, Glob, Ls, Find
// Execute: Write, Edit, Bash, Task (subagent)
// Generate: LLM generation calls

// You can also submit tasks to specific lanes manually:
await session.submitToLane('execute', {
  tool: 'Bash',
  args: { command: 'npm run build' },
});

// Or let the system auto-route:
await session.submitTask({
  tool: 'Bash',
  args: { command: 'npm run build' },
});
// → auto-routed to Execute lane
```

### ExternalTask Type

```typescript
interface ExternalTask {
  /** Unique task ID */
  id: string;
  /** Session this task belongs to */
  sessionId: string;
  /** Which lane this task is on */
  lane: 'control' | 'query' | 'execute' | 'generate';
  /** Tool to execute */
  toolName: string;
  /** Tool arguments */
  args: Record<string, unknown>;
  /** When the task was created */
  createdAt: Date;
  /** Timeout for external completion */
  timeout: number;
  /** Task metadata */
  metadata?: Record<string, string>;
}

interface ExternalTaskResult {
  success: boolean;
  output?: string;
  error?: string;
  metadata?: Record<string, string>;
}

interface LaneHandlerConfig {
  /** Execution mode */
  mode: 'internal' | 'external' | 'hybrid';
  /** Timeout for external task completion (ms) */
  timeout?: number;
  /** What to do when external task times out */
  timeoutAction?: 'reject' | 'fallback-internal' | 'auto-retry';
  /** Max retries for failed tasks */
  maxRetries?: number;
}

interface QueueStats {
  control: LaneStats;
  query: LaneStats;
  execute: LaneStats;
  generate: LaneStats;
  deadLetters: number;
}

interface LaneStats {
  pending: number;
  active: number;
  external: number;
  completed: number;
  failed: number;
}
```

---

## 4. Skills System API

Skills are lazily-loaded capability modules. Only names and descriptions are
in the system prompt (~30-50 tokens each). Full content is loaded on-demand
when the agent decides to use a skill.

### Loading Skills

```typescript
// Load skills from directory (project-level or user-level)
await session.loadSkills('/project/.a3s/skills');
await session.loadSkills('~/.config/a3s/skills');

// Load a single skill by name
await session.loadSkill('github-commands');

// Load skill from inline definition
await session.addSkill({
  name: 'code-review',
  description: 'Expert code review with security focus',
  content: `
    Review code for:
    1. Security vulnerabilities (OWASP Top 10)
    2. Performance issues
    3. Code style and best practices
  `,
  allowedTools: ['Read(*)', 'Grep(*)', 'Glob(*)'],
});

// List loaded skills
const skills = await session.listSkills();
// → [{ name: 'github-commands', description: '...', loaded: true }]

// Unload a skill
await session.unloadSkill('github-commands');
```

### Skill Definition Format (Markdown)

```markdown
---
name: github-commands
description: GitHub CLI operations for PR management
allowed-tools:
  - Bash(gh:*)
  - Read(*)
---

# GitHub Commands

Use the `gh` CLI for all GitHub operations.

## Pull Requests
- `gh pr create --title "..." --body "..."`
- `gh pr list --state open`
- `gh pr review --approve`

## Issues
- `gh issue create --title "..." --body "..."`
- `gh issue list --label bug`
```

### Progressive Disclosure Flow

```
1. Session starts → skill names/descriptions injected into system prompt
   "Available skills: github-commands (GitHub CLI operations), ..."

2. Agent decides to use a skill → calls find_skills("github")
   Returns: [{ name: 'github-commands', description: '...' }]

3. Agent loads skill content → calls use_skill("github-commands")
   Full skill content injected into context

4. Agent executes with skill's allowed tools
   Permission check: Bash(gh:*) → allowed, Bash(rm:*) → denied
```

### Skill Types

```typescript
interface Skill {
  /** Unique skill name */
  name: string;
  /** Short description (~30-50 tokens, shown in system prompt) */
  description: string;
  /** Full skill content (loaded on-demand) */
  content: string;
  /** Tool permission patterns */
  allowedTools?: string[];
  /** Whether this skill disables direct model tool invocation */
  disableModelInvocation?: boolean;
}

interface SkillInfo {
  name: string;
  description: string;
  /** Whether full content is loaded in context */
  loaded: boolean;
  /** Source: 'project' | 'user' | 'builtin' | 'inline' */
  source: string;
}
```

---

## 5. Built-in Agents (Subagents)

Built-in agents are pre-configured subagents with restricted permissions.
They run in child sessions and return results to the parent.

### Using Built-in Agents

```typescript
// Delegate to a built-in agent
const result = await session.delegate('explore', 'Find all API endpoints in src/');
console.log(result.text);

// Delegate with streaming
const { eventStream } = session.delegateStream('plan', 'Design a caching layer');
for await (const event of eventStream) {
  if (event.type === 'text') process.stdout.write(event.content);
}

// List available agents
const agents = await session.listAgents();
// → [
//   { name: 'explore', description: 'Read-only codebase exploration', mode: 'subagent' },
//   { name: 'plan', description: 'Read-only planning mode', mode: 'subagent' },
//   { name: 'general', description: 'Multi-step task execution', mode: 'subagent' },
// ]
```

### Built-in Agent Definitions

| Agent | Description | Permissions | Max Steps |
|-------|-------------|-------------|-----------|
| `explore` | Read-only codebase exploration | read, grep, glob, ls | 20 |
| `plan` | Read-only planning mode | read, grep, glob, ls | 30 |
| `general` | Multi-step task execution | read, write, edit, bash | 50 |

### Custom Agent Registration

```typescript
// Register a custom agent
session.registerAgent({
  name: 'security-audit',
  description: 'Security-focused code audit',
  system: 'You are a security expert. Find vulnerabilities.',
  permissions: {
    allow: ['Read', 'Grep', 'Glob'],
    deny: ['Write', 'Edit', 'Bash'],
  },
  maxSteps: 30,
  model: openai('gpt-4o'),  // Optional: override model
});

// Use it
const result = await session.delegate('security-audit', 'Audit the auth module');
```

---

## 6. HITL (Human-in-the-Loop) on Session

### Confirmation Policy

```typescript
// Set confirmation policy at session level
await session.setConfirmation({
  // Which tools require confirmation
  requireConfirmation: ['Bash', 'Write', 'Edit'],
  // Auto-approve read-only tools
  autoApprove: ['Read', 'Grep', 'Glob', 'Ls'],
  // Timeout before auto-action
  timeout: 30_000,
  // What to do on timeout
  timeoutAction: 'auto-approve', // or 'reject'
});

// Handle confirmations via callback (in session.send)
const result = await session.send('Deploy to production', {
  onConfirmation: async (request) => {
    console.log(`Approve ${request.toolName}(${JSON.stringify(request.args)})?`);
    // In a real app, show UI dialog and wait for user input
    return request.toolName !== 'Bash' || !request.args.command?.includes('rm');
  },
});

// Or handle confirmations via event stream (in session.sendStream)
const { eventStream } = session.sendStream('Refactor the database layer');
for await (const event of eventStream) {
  if (event.type === 'confirmation_required') {
    // Approve or reject
    await session.confirm(event.confirmationId, true);
  }
}
```

### Permission Policy

```typescript
// Set permission policy
await session.setPermissions({
  defaultAction: 'ask',  // 'allow' | 'deny' | 'ask'
  rules: [
    { tool: 'Read', action: 'allow' },
    { tool: 'Grep', action: 'allow' },
    { tool: 'Bash', pattern: 'git:*', action: 'allow' },
    { tool: 'Bash', pattern: 'rm:*', action: 'deny' },
    { tool: 'Write', action: 'ask' },
  ],
});
```

---

## 7. Context & Token Management on Session

```typescript
// Get detailed usage stats
const stats = await session.getStats();
console.log(`Tokens: ${stats.totalTokens}`);
console.log(`Cost: $${stats.totalCost}`);
console.log(`Messages: ${stats.messageCount}`);
console.log(`Tool calls: ${stats.toolCallCount}`);

// Context usage with threshold info
const ctx = await session.getContextUsage();
console.log(`Used: ${ctx.usedTokens}/${ctx.maxTokens} (${ctx.percent}%)`);
console.log(`Auto-compact at: ${ctx.compactThreshold}%`);

// Manual compact
await session.compactContext();

// Configure auto-compact
await session.configure({
  autoCompact: true,
  autoCompactThreshold: 0.8,  // 80%
});

// Token cost breakdown
const costs = await session.getCostSummary();
console.log('By model:', costs.modelBreakdown);
console.log('By day:', costs.dayBreakdown);

// Tool execution metrics
const metrics = await session.getToolMetrics();
for (const [tool, stats] of Object.entries(metrics)) {
  console.log(`${tool}: ${stats.callCount} calls, ${stats.avgDuration}ms avg`);
}
```

---

## 8. Complete Example: Coding Agent

```typescript
import { A3sClient, createProvider, tool } from '@a3s-lab/code';

const client = new A3sClient();
const openai = createProvider({ name: 'openai', apiKey: 'sk-xxx' });

await using session = await client.createSession({
  model: openai('gpt-4o'),
  workspace: '/project',
  system: 'You are a senior software engineer.',
});

// Load project skills
await session.loadSkills('/project/.a3s/skills');

// Configure lane handlers — route heavy tasks to external workers
await session.setLaneHandler('execute', {
  mode: 'external',
  timeout: 120_000,
  timeoutAction: 'fallback-internal',
});

// Set safety policies
await session.setConfirmation({
  requireConfirmation: ['Bash', 'Write'],
  autoApprove: ['Read', 'Grep', 'Glob'],
  timeout: 30_000,
  timeoutAction: 'reject',
});

await session.setPermissions({
  defaultAction: 'allow',
  rules: [
    { tool: 'Bash', pattern: 'rm -rf*', action: 'deny' },
    { tool: 'Bash', pattern: 'sudo*', action: 'deny' },
  ],
});

// Run the agentic loop
const { eventStream, result } = session.sendStream(
  'Add comprehensive error handling to the API routes in src/routes/',
  {
    strategy: 'planned',
    maxSteps: 50,
    reflection: true,
    onConfirmation: async (req) => {
      if (['Read', 'Grep', 'Glob'].includes(req.toolName)) return true;
      console.log(`\n⚠️  Approve ${req.toolName}?`);
      console.log(`   Args: ${JSON.stringify(req.args)}`);
      return true;
    },
  },
);

// Stream events to UI
for await (const event of eventStream) {
  switch (event.type) {
    case 'plan':
      console.log('\n📋 Plan:');
      event.steps.forEach((s, i) => console.log(`  ${i + 1}. ${s.description}`));
      break;
    case 'text':
      process.stdout.write(event.content);
      break;
    case 'tool_call':
      console.log(`\n🔧 ${event.toolName}`);
      break;
    case 'reflection':
      console.log(`\n🔍 Confidence: ${event.confidence}`);
      break;
    case 'subagent_start':
      console.log(`\n🤖 Delegating to ${event.agentName}: ${event.task}`);
      break;
    case 'external_task_pending':
      console.log(`\n⏳ External: ${event.task.toolName} → waiting for worker`);
      break;
    case 'external_task_completed':
      console.log(`\n✅ External: ${event.task.toolName} → done`);
      break;
    case 'context_compact':
      console.log(`\n📦 Context: ${event.beforeTokens} → ${event.afterTokens} tokens`);
      break;
    case 'done':
      console.log(`\n✅ Done: ${event.finishReason}`);
      break;
  }
}

const finalResult = await result;
console.log(`\nTotal: ${finalResult.steps.length} steps, ${finalResult.toolCalls.length} tool calls`);
console.log(`Tokens: ${finalResult.usage.totalTokens}, Cost: $${finalResult.usage.totalCost}`);

// Queue stats
const stats = await session.getQueueStats();
console.log(`Queue: ${stats.execute.completed} executed, ${stats.execute.external} external`);
```

### External Worker (Separate Process/Machine)

```typescript
import { A3sClient } from '@a3s-lab/code';

const worker = new A3sClient({ address: 'agent-host:4088' });
const sessionId = process.env.SESSION_ID!;

// Real-time task processing via event stream
for await (const event of worker.subscribeEvents(sessionId)) {
  if (event.type !== 'external_task_pending') continue;

  const task = event.task;
  console.log(`[Worker] ${task.toolName}(${JSON.stringify(task.args)})`);

  try {
    const output = await runInSandbox(task.toolName, task.args);
    await worker.completeExternalTask(sessionId, task.id, {
      success: true,
      output,
    });
  } catch (err) {
    await worker.completeExternalTask(sessionId, task.id, {
      success: false,
      error: err.message,
    });
  }
}
```

---

## API Summary

### Session Methods

#### AgenticLoop

| Method | Description |
|--------|-------------|
| `session.send(prompt, options?)` | Send message, auto-enters AgenticLoop when tools are called |
| `session.sendStream(prompt, options?)` | Stream the full interaction including AgenticLoop events |
| `session.delegate(agent, task)` | Delegate task to built-in/custom agent |
| `session.delegateStream(agent, task)` | Stream delegated task events |

#### Lane Queue (Per-Session, All Tasks Externally Extensible)

| Method | Description |
|--------|-------------|
| `session.setLaneHandler(lane, config)` | Set lane execution mode (internal/external/hybrid) |
| `session.getLaneHandler(lane)` | Get lane handler config |
| `session.listPendingTasks()` | List tasks waiting for external processing |
| `session.completeTask(taskId, result)` | Complete an external task |
| `session.submitTask(task)` | Submit task (auto-routed to lane) |
| `session.submitToLane(lane, task)` | Submit task to specific lane |
| `session.getQueueStats()` | Get per-lane queue statistics |

#### Skills

| Method | Description |
|--------|-------------|
| `session.loadSkills(dir)` | Load skills from directory |
| `session.loadSkill(name)` | Load a single skill |
| `session.addSkill(skill)` | Add inline skill definition |
| `session.unloadSkill(name)` | Unload a skill |
| `session.listSkills()` | List available skills |

#### Built-in Agents

| Method | Description |
|--------|-------------|
| `session.listAgents()` | List available agents |
| `session.registerAgent(def)` | Register custom agent |

#### HITL & Permissions

| Method | Description |
|--------|-------------|
| `session.setConfirmation(policy)` | Set HITL confirmation policy |
| `session.setPermissions(policy)` | Set tool permission policy |
| `session.confirm(id, approved)` | Respond to confirmation request |

#### Context & Observability

| Method | Description |
|--------|-------------|
| `session.getStats()` | Get token/cost/tool statistics |
| `session.getToolMetrics()` | Get per-tool execution metrics |
| `session.getCostSummary()` | Get cost breakdown |
| `session.configure(options)` | Update session config (not model/workspace) |

### Relationship to Existing Methods

| Existing | New | Relationship |
|----------|-----|-------------|
| `session.generateText()` | `session.send()` | `send()` is the agentic version — auto-enters AgenticLoop when tools are called |
| `session.streamText()` | `session.sendStream()` | `sendStream()` streams the full agent loop including tool calls, planning, reflection |
| Low-level `client.loadSkill()` | `session.loadSkill()` | Session-level convenience |
| Low-level `client.confirmToolExecution()` | `session.confirm()` | Session-level convenience |
| Low-level `client.setPermissionPolicy()` | `session.setPermissions()` | Session-level convenience |
| Low-level `client.setLaneHandler()` | `session.setLaneHandler()` | Session-level convenience |
| Low-level `client.listPendingExternalTasks()` | `session.listPendingTasks()` | Session-level convenience |
| Low-level `client.completeExternalTask()` | `session.completeTask()` | Session-level convenience |
