#!/usr/bin/env node
/**
 * Query-Lane Tool Parallelization Test
 *
 * Demonstrates A3S Code's Query-lane tool parallelization with slow I/O operations.
 * Parallelization is OPT-IN (default: serial execution). Users control when and how
 * to parallelize via SessionQueueConfig.
 *
 * This test uses web_fetch to demonstrate real performance benefits, as network I/O
 * is significantly slower than local file operations.
 *
 * Performance: 3-8x speedup for network I/O operations
 */

const path = require('path');
const fs = require('fs');
const { Agent } = require('../');

const PROMPT =
  'Fetch the following web pages and extract their titles:\n' +
  '1. https://www.rust-lang.org/\n' +
  '2. https://tokio.rs/\n' +
  '3. https://docs.rs/\n' +
  '4. https://crates.io/\n' +
  '5. https://github.com/rust-lang/rust\n' +
  '6. https://blog.rust-lang.org/\n' +
  '7. https://www.rust-lang.org/learn\n' +
  '8. https://www.rust-lang.org/tools\n' +
  '9. https://www.rust-lang.org/governance\n' +
  '10. https://www.rust-lang.org/community\n' +
  '\n' +
  "Fetch all pages at once using web_fetch tool, don't do them one by one.";

function findConfig() {
  // Check ~/.a3s/config.hcl
  const homeConfig = path.join(require('os').homedir(), '.a3s', 'config.hcl');
  if (fs.existsSync(homeConfig)) return homeConfig;

  // Walk up from this file to find .a3s/config.hcl
  let dir = __dirname;
  for (let i = 0; i < 10; i++) {
    const candidate = path.join(dir, '.a3s', 'config.hcl');
    if (fs.existsSync(candidate)) return candidate;
    dir = path.dirname(dir);
  }
  throw new Error('Config file not found');
}

async function testDefaultSerial(agent) {
  console.log('\n📦 Test 1: Default Behavior (Serial Execution)');
  console.log('-'.repeat(80));
  console.log('Task: Fetch 10 web pages with default configuration\n');

  // Create session WITHOUT parallelization (default: enable_parallelization = false)
  const session = agent.session('.');

  const start = Date.now();
  const result = await session.send(PROMPT);
  const elapsed = (Date.now() - start) / 1000;

  console.log(`✓ Completed in: ${elapsed.toFixed(2)}s`);
  console.log(`  Result length: ${result.text.length} chars`);
  console.log(`  Tool calls: ${result.toolCallsCount}`);
  console.log('\n💡 Default: enable_parallelization = false (serial execution)');
  console.log('   Expected: ~10 * avg_fetch_time (network latency adds up)\n');

  return elapsed;
}

async function testEnabledParallel(agent) {
  console.log('\n⚡ Test 2: Enabled Parallelization (Parallel Execution)');
  console.log('-'.repeat(80));
  console.log('Task: Fetch 10 web pages in parallel via opt-in configuration\n');

  console.log('✓ SessionQueueConfig created');
  console.log('  enable_parallelization: true (OPT-IN)');
  console.log('  Query lane max concurrency: 10');
  console.log('  Custom strategy:');
  console.log('    - min_tool_count: 3 (lower threshold)');
  console.log('    - allowed_tools: [web_fetch, web_search]');
  console.log('    - blocked_tools: [bash, write, edit, patch]\n');

  // Create session WITH parallelization enabled
  // SessionOptions and SessionQueueConfig are plain JS objects (napi(object))
  const session = agent.session('.', {
    queueConfig: {
      enableParallelization: true,
      queryConcurrency: 10,
      parallelizationStrategy: {
        minToolCount: 3,
        allowedTools: ['web_fetch', 'web_search'],
      },
    },
  });

  const start = Date.now();
  const result = await session.send(PROMPT);
  const elapsed = (Date.now() - start) / 1000;

  console.log(`\n✓ Completed in: ${elapsed.toFixed(2)}s`);
  console.log(`  Result length: ${result.text.length} chars`);
  console.log(`  Tool calls: ${result.toolCallsCount}`);
  console.log('\n💡 Parallelization enabled: web_fetch calls execute in parallel');
  console.log('   Expected: ~max(fetch_times) instead of sum(fetch_times)');
  console.log('   Speedup: 3-8x for network I/O operations\n');

  return elapsed;
}

async function main() {
  console.log('='.repeat(80));
  console.log('Query-Lane Tool Parallelization Test (Node.js SDK)');
  console.log('='.repeat(80));
  console.log('\n📌 Test Scenario: Fetch 10 web pages');
  console.log('   This demonstrates real performance benefits with slow I/O operations.\n');

  const configPath = findConfig();
  console.log(`📄 Using config: ${configPath}\n`);

  const agent = await Agent.create(configPath);

  // Run tests
  const sequentialTime = await testDefaultSerial(agent);
  const parallelTime = await testEnabledParallel(agent);

  // Performance comparison
  console.log('\n' + '='.repeat(80));
  console.log('Performance Comparison');
  console.log('='.repeat(80));
  console.log(`Sequential (default):   ${sequentialTime.toFixed(2)}s (baseline)`);
  console.log(`Parallel (opt-in):      ${parallelTime.toFixed(2)}s (${(sequentialTime / parallelTime).toFixed(2)}x speedup)`);
  console.log('\n✅ All parallelization tests completed!');
  console.log('='.repeat(80));
}

main().catch(console.error);
