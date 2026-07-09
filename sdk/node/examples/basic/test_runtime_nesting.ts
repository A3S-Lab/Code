#!/usr/bin/env npx tsx
/**
 * Test: Nested Tokio Runtime (Issue #22)
 *
 * This test verifies that Agent.create() works correctly when called
 * from within an existing Tokio runtime (like NestJS watch mode).
 *
 * Previously, this would crash with:
 * "Cannot start a runtime from within a runtime"
 *
 * The fix uses tokio::runtime::Handle::try_current() to detect and
 * use the existing runtime instead of creating a new one.
 *
 * Run with: npx tsx examples/test_runtime_nesting.ts
 */

import { Agent } from '../../index.js';
import * as path from 'path';
import * as os from 'os';
import * as fs from 'fs';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

function findConfig(): string {
  if (process.env.A3S_CONFIG) return process.env.A3S_CONFIG;
  // Try configs directory first (for testing)
  const configsDir = path.join(__dirname, '..', 'configs', 'test_config.acl');
  if (fs.existsSync(configsDir)) return configsDir;
  // Then try standard locations
  const homeConfig = path.join(os.homedir(), '.a3s', 'config.acl');
  if (fs.existsSync(homeConfig)) return homeConfig;
  let p = path.resolve(__dirname);
  for (let i = 0; i < 10; i++) {
    const c = path.join(p, '.a3s', 'config.acl');
    if (fs.existsSync(c)) return c;
    const parent = path.dirname(p);
    if (parent === p) break;
    p = parent;
  }
  throw new Error('Config not found. Create configs/test_config.acl, ~/.a3s/config.acl, or set A3S_CONFIG');
}

function hasRealProviderConfig(configPath: string): boolean {
  if (process.env.A3S_CONFIG) return true;
  if (configPath.endsWith('test_config.acl')) {
    return Boolean(process.env.OPENAI_API_KEY && process.env.OPENAI_BASE_URL);
  }
  return true;
}

// Simulate NestJS-like environment where Tokio runtime already exists
async function simulateNestedRuntime() {
  console.log('='.repeat(70));
  console.log('  Test: Nested Tokio Runtime (Issue #22)');
  console.log('='.repeat(70));
  console.log();

  const configPath = findConfig();
  console.log(`  Config: ${configPath}`);
  console.log();

  // Simulate NestJS initialization - create agent while "inside" a runtime
  console.log('  [1] Simulating NestJS environment (existing Tokio runtime)...');
  console.log('  [2] Calling Agent.create() from within that runtime...');
  console.log();

  try {
    console.log('  Creating agent...');
    const agent = await Agent.create(configPath);
    console.log('  ✓ Agent created successfully!');
    console.log();

    console.log('  Creating session...');
    const session = agent.session('.', {
      planningMode: 'enabled',
    });
    console.log('  ✓ Session created successfully!');
    console.log();

    console.log('  Sending test prompt...');
    const result = await session.send('Say hello in exactly 3 words');
    console.log(`  ✓ Response: ${result.text.substring(0, 100)}...`);
    console.log();

    console.log('='.repeat(70));
    console.log('  [PASS] All operations completed successfully!');
    console.log('  Issue #22 is fixed: nested runtime works correctly.');
    console.log('='.repeat(70));
    return true;
  } catch (error: any) {
    console.error();
    console.error('  [FAIL] Error occurred:', error.message || error);
    console.error();
    if (error.message?.includes('Cannot start a runtime from within a runtime')) {
      console.error('  This is the original Issue #22 bug - the fix is not working.');
    }
    return false;
  }
}

// Test with multiple concurrent sessions (stress test)
async function testConcurrentSessions() {
  console.log();
  console.log('='.repeat(70));
  console.log('  Test: Concurrent Sessions (Stress Test)');
  console.log('='.repeat(70));
  console.log();

  const configPath = findConfig();

  try {
    const agent = await Agent.create(configPath);
    console.log('  ✓ Agent created for concurrent test');
    console.log();

    console.log('  Creating 3 concurrent sessions...');
    const sessions = [
      agent.session('.'),
      agent.session('.'),
      agent.session('.'),
    ];
    console.log('  ✓ All sessions created');
    console.log();

    console.log('  Sending concurrent prompts...');
    const results = await Promise.all([
      sessions[0].send('What is 2+2? Answer in numbers only.'),
      sessions[1].send('What is 3+3? Answer in numbers only.'),
      sessions[2].send('What is 4+4? Answer in numbers only.'),
    ]);

    console.log('  ✓ All responses received:');
    results.forEach((r, i) => {
      console.log(`    Session ${i + 1}: ${r.text.substring(0, 50)}`);
    });
    console.log();

    console.log('='.repeat(70));
    console.log('  [PASS] Concurrent sessions work correctly!');
    console.log('='.repeat(70));
    return true;
  } catch (error: any) {
    console.error();
    console.error('  [FAIL] Error:', error.message || error);
    return false;
  }
}

// Main
async function main() {
  const configPath = findConfig();
  if (!hasRealProviderConfig(configPath)) {
    console.log('Nested runtime example skipped.');
    console.log('Set OPENAI_API_KEY and OPENAI_BASE_URL, or set A3S_CONFIG to a real config.');
    process.exit(0);
  }

  let allPassed = true;

  allPassed = await simulateNestedRuntime() && allPassed;
  allPassed = await testConcurrentSessions() && allPassed;

  console.log();
  if (allPassed) {
    console.log('✓ All tests passed!');
    process.exit(0);
  } else {
    console.log('✗ Some tests failed');
    process.exit(1);
  }
}

main().catch(e => {
  console.error('Fatal error:', e);
  process.exit(1);
});
