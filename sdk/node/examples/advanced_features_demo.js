#!/usr/bin/env node
/**
 * A3S Code — Advanced Features Demo (Real LLM)
 *
 * Demonstrates features NOT covered by agentic_loop_demo.js:
 * 1. Direct tool execution (no LLM)
 * 2. Hooks (lifecycle event interception)
 * 3. Queue & lanes (priority-based scheduling)
 * 4. Security provider (taint tracking)
 * 5. Auto-compaction & resilience
 * 6. Persistent memory
 *
 * Usage:
 *     cd crates/code/sdk/node
 *     node examples/advanced_features_demo.js
 */

const { Agent, SessionOptions, SessionQueueConfig, builtinSkills } = require('../index.js');
const fs = require('fs');
const path = require('path');
const os = require('os');

// ============================================================================
// Helpers
// ============================================================================

function findConfigPath() {
  if (process.env.A3S_CONFIG) return process.env.A3S_CONFIG;
  const homeConfig = path.join(os.homedir(), '.a3s', 'config.hcl');
  if (fs.existsSync(homeConfig)) return homeConfig;
  const projectConfig = path.join(
    __dirname, '..', '..', '..', '..', '..', '..', '.a3s', 'config.hcl'
  );
  if (fs.existsSync(projectConfig)) return projectConfig;
  throw new Error('Config not found. Create ~/.a3s/config.hcl or set A3S_CONFIG');
}

function separator(title) {
  console.log(`\n${'═'.repeat(72)}`);
  console.log(`  ${title}`);
  console.log(`${'═'.repeat(72)}\n`);
}

function truncate(text, max = 200) {
  text = text.trim();
  return text.length <= max ? text : `${text.substring(0, max)}…`;
}

function makeTempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-adv-'));
}

function cleanupDir(dir) {
  try { fs.rmSync(dir, { recursive: true, force: true }); } catch (_) {}
}

// ============================================================================
// Demo 1: Direct Tool Execution
// ============================================================================

async function demo1DirectTools(agent) {
  separator('Demo 1: Direct Tool Execution (No LLM)');

  const workspace = makeTempDir();
  try {
    const session = agent.session(workspace, { permissive: true });
    console.log(`  Workspace: ${workspace}\n`);

    // Write
    console.log('  1. Write a file');
    let r = session.tool('write', { path: 'hello.txt', content: 'Hello from Node.js!\nLine 2.' });
    console.log(`     exit=${r.exitCode} output=${truncate(r.output, 60)}`);

    // Read
    console.log('  2. Read the file');
    const content = session.readFile('hello.txt');
    console.log(`     content: ${truncate(content, 60)}`);

    // Glob
    console.log('  3. Glob for *.txt');
    const files = session.glob('*.txt');
    console.log(`     matches: ${JSON.stringify(files)}`);

    // Bash
    console.log('  4. Run bash command');
    const bashOut = session.bash('wc -l hello.txt');
    console.log(`     output: ${bashOut.trim()}`);

    // Grep
    console.log('  5. Grep for "Node"');
    const grepOut = session.grep('Node');
    console.log(`     output: ${truncate(grepOut, 60)}`);

    // Edit
    console.log('  6. Edit the file');
    r = session.tool('edit', {
      path: 'hello.txt',
      old_string: 'Line 2.',
      new_string: 'Line 2 — edited from Node.js!'
    });
    console.log(`     exit=${r.exitCode}`);

    // Verify
    const final_ = session.readFile('hello.txt');
    console.log(`\n  ✓ Final content: ${truncate(final_, 80)}`);
  } finally {
    cleanupDir(workspace);
  }
}

// ============================================================================
// Demo 2: Hooks
// ============================================================================

async function demo2Hooks(agent) {
  separator('Demo 2: Hooks (Lifecycle Event Interception)');

  const workspace = makeTempDir();
  try {
    const session = agent.session(workspace, { permissive: true });

    // Register hooks
    session.registerHook('audit_tools', 'pre_tool_use', null, { priority: 10 });
    session.registerHook('audit_bash', 'post_tool_use', { tool: 'bash' }, { priority: 20 });
    session.registerHook('log_gen', 'generate_start');
    session.registerHook('log_err', 'on_error');

    console.log(`  Registered ${session.hookCount()} hooks`);
    console.log('  • audit_tools  (PreToolUse, priority=10)');
    console.log('  • audit_bash   (PostToolUse, bash only)');
    console.log('  • log_gen      (GenerateStart)');
    console.log('  • log_err      (OnError)\n');

    for (const event of session.stream(
      "Create a file test.sh with `echo hello` and run it with bash."
    )) {
      switch (event.type) {
        case 'tool_start':
          console.log(`  [hook] 🔧 PreToolUse → ${event.toolName}`);
          break;
        case 'tool_end':
          if (event.toolName === 'bash') {
            console.log(`  [hook] 🔧 PostToolUse(bash) → exit=${event.exitCode}`);
          }
          break;
        case 'end':
          console.log(`\n  ■ Done (${event.totalTokens} tokens)`);
          break;
        case 'error':
          console.log(`  ✗ Error: ${event.error}`);
          break;
      }
      if (event.type === 'end' || event.type === 'error') break;
    }

    const removed = session.unregisterHook('audit_tools');
    console.log(`\n  Unregistered audit_tools: ${removed}`);
    console.log(`  Remaining hooks: ${session.hookCount()}`);
  } finally {
    cleanupDir(workspace);
  }
}

// ============================================================================
// Demo 3: Queue & Lanes
// ============================================================================

async function demo3QueueLanes(agent) {
  separator('Demo 3: Queue & Lanes (Priority-Based Scheduling)');

  const workspace = makeTempDir();
  try {
    for (let i = 1; i <= 3; i++) {
      fs.writeFileSync(path.join(workspace, `file${i}.txt`), `Content of file ${i}\n`);
    }

    const qc = new SessionQueueConfig();
    qc.withLaneFeatures();
    qc.setQueryConcurrency(8);
    qc.setExecuteConcurrency(2);

    const session = agent.session(workspace, { permissive: true, queueConfig: qc });

    console.log(`  Queue active: ${session.hasQueue()}`);
    console.log(`  Workspace: ${workspace}\n`);

    const result = await session.send(
      'Read all .txt files and create a combined.md with their contents.'
    );

    console.log(`  Tool calls: ${result.toolCallsCount}`);
    console.log(`  Tokens:     ${result.totalTokens}`);

    const stats = session.queueStats();
    console.log(`\n  Queue stats: ${JSON.stringify(stats)}`);

    const dlq = session.deadLetters();
    console.log(`  Dead letters: ${dlq.length}`);
  } finally {
    cleanupDir(workspace);
  }
}

// ============================================================================
// Demo 4: Security Provider
// ============================================================================

async function demo4Security(agent) {
  separator('Demo 4: Security Provider (Taint Tracking)');

  const workspace = makeTempDir();
  try {
    fs.writeFileSync(
      path.join(workspace, 'secrets.env'),
      'DATABASE_URL=postgres://admin:p4ssw0rd@db.example.com/prod\n' +
      'API_KEY=sk-abc123def456\n' +
      'AWS_SECRET=AKIAIOSFODNN7EXAMPLE\n'
    );

    const opts = new SessionOptions();
    opts.defaultSecurity = true;
    const session = agent.session(workspace, { options: opts, permissive: true });

    console.log('  Security: DefaultSecurityProvider enabled');
    console.log('  Features: taint tracking + output sanitization\n');

    const result = await session.send(
      'Read secrets.env and tell me what types of secrets are in it. ' +
      'Do NOT include the actual secret values in your response.'
    );

    console.log(`  Tool calls: ${result.toolCallsCount}`);
    console.log(`  Tokens:     ${result.totalTokens}`);
    console.log(`  Response:   ${truncate(result.text, 300)}`);
  } finally {
    cleanupDir(workspace);
  }
}

// ============================================================================
// Demo 5: Auto-Compaction & Resilience
// ============================================================================

async function demo5Resilience(agent) {
  separator('Demo 5: Auto-Compaction & Resilience');

  const workspace = makeTempDir();
  try {
    const session = agent.session(workspace, {
      permissive: true,
      maxParseRetries: 3,
      toolTimeoutMs: 30_000,
      circuitBreakerThreshold: 5,
    });

    console.log('  Parse retries:   3');
    console.log('  Tool timeout:    30s');
    console.log('  Circuit breaker: 5\n');

    const result = await session.send(
      'Create a CSV file with 15 rows of sample data (id, name, score), ' +
      'then create a report.md analyzing the data.'
    );

    console.log(`  Tool calls: ${result.toolCallsCount}`);
    console.log(`  Tokens:     ${result.totalTokens}`);

    for (const name of ['data.csv', 'report.md']) {
      const p = path.join(workspace, name);
      if (fs.existsSync(p)) {
        console.log(`  ✓ ${name} (${fs.statSync(p).size} bytes)`);
      } else {
        console.log(`  ⚠ ${name} not found`);
      }
    }
  } finally {
    cleanupDir(workspace);
  }
}

// ============================================================================
// Demo 6: Persistent Memory
// ============================================================================

async function demo6Memory(agent) {
  separator('Demo 6: Persistent Memory');

  const workspace = makeTempDir();
  const memoryDir = makeTempDir();
  try {
    const opts = new SessionOptions();
    opts.memoryDir = memoryDir;
    const session = agent.session(workspace, { options: opts, permissive: true });

    console.log(`  Memory dir: ${memoryDir}\n`);

    // Turn 1: teach
    console.log('  [Turn 1] Teaching the agent...');
    const r1 = await session.send(
      'Remember this: The project deadline is March 15, 2026. ' +
      'The tech lead is Alice and the PM is Bob.'
    );
    console.log(`    Tokens: ${r1.totalTokens}`);

    // Turn 2: recall
    console.log('  [Turn 2] Asking about remembered info...');
    const r2 = await session.send('Who is the tech lead and when is the deadline?');
    console.log(`    Tokens: ${r2.totalTokens}`);
    console.log(`    Answer: ${truncate(r2.text, 150)}`);
  } finally {
    cleanupDir(workspace);
    cleanupDir(memoryDir);
  }
}

// ============================================================================
// Main
// ============================================================================

async function main() {
  console.log('╔══════════════════════════════════════════════════════════════════════╗');
  console.log('║      A3S Code Node.js SDK — Advanced Features Demo (Real LLM)      ║');
  console.log('╚══════════════════════════════════════════════════════════════════════╝');

  const configPath = findConfigPath();
  console.log(`\n  Config: ${configPath}`);

  const agent = await Agent.create(configPath);
  console.log('  Agent:  ✓ created\n');

  await demo1DirectTools(agent);
  await demo2Hooks(agent);
  await demo3QueueLanes(agent);
  await demo4Security(agent);
  await demo5Resilience(agent);
  await demo6Memory(agent);

  separator('All Demos Complete ✓');
}

main().catch((err) => {
  console.error('❌ Demo failed:', err);
  process.exit(1);
});
