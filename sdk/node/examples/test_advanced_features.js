#!/usr/bin/env node
/**
 * A3S Code — Advanced Features Demo
 *
 * Demonstrates: auto-compact, memory, security, hooks, batch tool,
 * permission checker, planning, and context providers.
 *
 * ## Usage
 *
 *     cd crates/code/sdk/node
 *     node examples/test_advanced_features.js
 *
 * Requires a valid LLM API key in ~/.a3s/config.hcl or $A3S_CONFIG.
 */

const { Agent } = require('../index.js');
const fs = require('fs');
const path = require('path');
const os = require('os');

function findConfigPath() {
  if (process.env.A3S_CONFIG) return process.env.A3S_CONFIG;
  const homeConfig = path.join(os.homedir(), '.a3s', 'config.hcl');
  if (fs.existsSync(homeConfig)) return homeConfig;
  throw new Error('No config found. Set A3S_CONFIG or create ~/.a3s/config.hcl');
}

// ============================================================================
// Test 1: Auto-Compact
// ============================================================================

async function testAutoCompact() {
  console.log('\n=== Test 1: Auto-Compact ===');
  const agent = await Agent.create(findConfigPath());

  // Enable auto-compact at 80% context usage
  const session = agent.session(os.tmpdir(), {
    autoCompact: true,
    autoCompactThreshold: 0.8,
  });

  const result = await session.send('Say hello in one sentence.');
  console.log('✓ auto-compact session created, response:', result.text.slice(0, 60));
}

// ============================================================================
// Test 2: File Memory
// ============================================================================

async function testMemory() {
  console.log('\n=== Test 2: File Memory ===');
  const agent = await Agent.create(findConfigPath());
  const memDir = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-mem-'));

  const session = agent.session(os.tmpdir(), {
    memoryDir: memDir,
  });

  const result = await session.send('Remember: my favorite language is Rust.');
  console.log('✓ memory session created, response:', result.text.slice(0, 60));

  // Cleanup
  fs.rmSync(memDir, { recursive: true, force: true });
}

// ============================================================================
// Test 3: Default Security Provider
// ============================================================================

async function testSecurity() {
  console.log('\n=== Test 3: Security Provider ===');
  const agent = await Agent.create(findConfigPath());

  // Enable default security: taint-tracks input, sanitizes output
  const session = agent.session(os.tmpdir(), {
    defaultSecurity: true,
  });

  const result = await session.send('What is 2 + 2?');
  console.log('✓ security session created, response:', result.text.slice(0, 60));
}

// ============================================================================
// Test 4: Hooks — intercept lifecycle events
// ============================================================================

async function testHooks() {
  console.log('\n=== Test 4: Hooks ===');
  const agent = await Agent.create(findConfigPath());
  const session = agent.session(os.tmpdir());

  const toolCalls = [];

  // Register a hook that fires before every tool use
  session.registerHook('log-tools', 'pre_tool_use', (event) => {
    toolCalls.push(event.toolName || event.tool_name || 'unknown');
    console.log(`  → pre_tool_use: ${JSON.stringify(event).slice(0, 80)}`);
    return null; // allow execution
  });

  console.log(`✓ hook registered, hook count: ${session.hookCount()}`);

  // Cleanup
  session.unregisterHook('log-tools');
  console.log(`✓ hook unregistered, hook count: ${session.hookCount()}`);
}

// ============================================================================
// Test 5: Planning Mode
// ============================================================================

async function testPlanning() {
  console.log('\n=== Test 5: Planning Mode ===');
  const agent = await Agent.create(findConfigPath());

  const session = agent.session(os.tmpdir(), {
    planning: true,
    goalTracking: true,
  });

  const result = await session.send('List 3 benefits of Rust in one sentence each.');
  console.log('✓ planning session, response length:', result.text.length);
}

// ============================================================================
// Test 6: Permissive Policy (no HITL confirmation)
// ============================================================================

async function testPermissive() {
  console.log('\n=== Test 6: Permissive Policy ===');
  const agent = await Agent.create(findConfigPath());

  const session = agent.session(os.tmpdir(), {
    permissive: true,
  });

  const result = await session.send('What is the current directory?');
  console.log('✓ permissive session, response:', result.text.slice(0, 60));
}

// ============================================================================
// Test 7: Resilience Options
// ============================================================================

async function testResilience() {
  console.log('\n=== Test 7: Resilience Options ===');
  const agent = await Agent.create(findConfigPath());

  const session = agent.session(os.tmpdir(), {
    maxParseRetries: 3,
    toolTimeoutMs: 30000,
    circuitBreakerThreshold: 5,
  });

  const result = await session.send('Say "resilient" once.');
  console.log('✓ resilience session, response:', result.text.slice(0, 60));
}

// ============================================================================
// Test 8: Combined — security + auto-compact + memory + hooks
// ============================================================================

async function testCombined() {
  console.log('\n=== Test 8: Combined Features ===');
  const agent = await Agent.create(findConfigPath());
  const memDir = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-combined-'));

  const session = agent.session(os.tmpdir(), {
    defaultSecurity: true,
    autoCompact: true,
    autoCompactThreshold: 0.9,
    memoryDir: memDir,
    permissive: true,
  });

  session.registerHook('audit', 'post_tool_use', (event) => {
    console.log(`  → post_tool_use fired`);
    return null;
  });

  const result = await session.send('What is 1 + 1?');
  console.log('✓ combined session, response:', result.text.slice(0, 60));

  fs.rmSync(memDir, { recursive: true, force: true });
}

// ============================================================================
// Main
// ============================================================================

async function main() {
  console.log('A3S Code — Advanced Features Test');
  console.log('===================================');

  const tests = [
    testAutoCompact,
    testMemory,
    testSecurity,
    testHooks,
    testPlanning,
    testPermissive,
    testResilience,
    testCombined,
  ];

  let passed = 0;
  let failed = 0;

  for (const test of tests) {
    try {
      await test();
      passed++;
    } catch (err) {
      console.error(`✗ ${test.name} failed:`, err.message);
      failed++;
    }
  }

  console.log(`\n===================================`);
  console.log(`Results: ${passed} passed, ${failed} failed`);
  process.exit(failed > 0 ? 1 : 0);
}

main().catch((err) => {
  console.error('Fatal:', err);
  process.exit(1);
});
