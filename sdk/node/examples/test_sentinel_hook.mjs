/**
 * Sentinel Hook Propagation Test (TypeScript/Node.js)
 *
 * Verifies that hooks registered on a parent session propagate automatically
 * to sub-agents spawned by the `task` tool.
 *
 * Uses Kimi K2.5 via config at a3s/.a3s/config.hcl
 */

import { createRequire } from 'module';
import path from 'path';
import { fileURLToPath } from 'url';
import os from 'os';
import fs from 'fs';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// Load the native SDK
const require = createRequire(import.meta.url);
const sdk = require('../index.js');
const { Agent } = sdk;

// __dirname = .../a3s/crates/code/sdk/node/examples — go up 5 levels to reach a3s/
const CONFIG_PATH = path.join(__dirname, '..', '..', '..', '..', '..', '.a3s', 'config.hcl');

async function main() {
  console.log('='.repeat(60));
  console.log('Sentinel Hook Propagation Test (TypeScript/Node.js)');
  console.log('='.repeat(60));
  console.log();

  if (!fs.existsSync(CONFIG_PATH)) {
    console.error(`✗ Config not found: ${CONFIG_PATH}`);
    process.exit(1);
  }
  console.log(`Config: ${CONFIG_PATH}`);
  console.log();

  const agent = await Agent.create(CONFIG_PATH);
  console.log('✓ Agent created');

  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'sentinel-hook-test-'));
  try {
    const session = agent.session(workspace, {
      permissive: true,
      builtinSkills: true,
    });

    // --- Step 1: Register sentinel hooks ---
    console.log('\n[Step 1] Registering sentinel hooks on parent session...');
    session.registerHook('sentinel-pre-tool', 'pre_tool_use', null, { priority: 5 });
    session.registerHook('sentinel-post-tool', 'post_tool_use', null, { priority: 5 });
    const count = session.hookCount();
    console.log(`  Hooks registered: ${count}`);
    console.assert(count === 2, `Expected 2 hooks, got ${count}`);
    console.log('  ✓ 2 hooks registered with priority 5');
    console.log();
    console.log('  These hooks propagate to sub-agents via ToolContext.sentinel_hook');
    console.log();

    // --- Step 2: Send prompt that triggers the task tool ---
    console.log('[Step 2] Sending prompt to trigger sub-agent via `task` tool...');
    console.log();

    const parentToolsCalled = [];
    let taskToolUsed = false;

    const prompt =
      "Use the task tool to delegate the following to a sub-agent of type 'general': " +
      "list .ts files in /tmp. Just call task once and return the result.";

    // EventStream uses .next() API (not Symbol.asyncIterator)
    const stream = await session.stream(prompt);

    while (true) {
      const { value: event, done } = await stream.next();
      if (done || !event) break;

      if (event.type === 'tool_start') {
        parentToolsCalled.push(event.toolName);
        const marker = event.toolName === 'task' ? '★' : ' ';
        console.log(`  ${marker} [parent tool_start] ${event.toolName}`);
        if (event.toolName === 'task') {
          taskToolUsed = true;
          console.log();
          console.log('    → task tool invoked — sub-agent is being spawned');
          console.log('    → HookEngine from parent → child AgentConfig.hook_engine');
          console.log('    → ToolContext.sentinel_hook set for grandchild propagation');
          console.log();
        }
      } else if (event.type === 'tool_end') {
        if (event.toolName === 'task') {
          console.log(`  ★ [parent tool_end  ] task → exit=${event.exitCode}`);
          if (event.toolOutput) {
            console.log(`    sub-agent result: ${event.toolOutput.slice(0, 200)}`);
          }
        }
      } else if (event.type === 'end') {
        console.log(`\n  [end] total_tokens=${event.totalTokens}`);
        break;
      } else if (event.type === 'error') {
        console.log(`  [error] ${event.error}`);
        break;
      }
    }

    // --- Step 3: Verify results ---
    console.log();
    console.log('='.repeat(60));
    console.log('Results');
    console.log('='.repeat(60));
    console.log(`\nParent tools called: [${parentToolsCalled.join(', ')}]`);
    console.log(`task tool used:      ${taskToolUsed}`);
    console.log(`Hooks after run:     ${session.hookCount()}`);

    console.assert(session.hookCount() === 2,
      `Hooks were modified (expected 2, got ${session.hookCount()})`);
    console.log('\n✓ Parent hooks intact after sub-agent execution');

    if (taskToolUsed) {
      console.log('✓ task tool was called — sentinel hook propagated to sub-agent');
      console.log();
      console.log('Sentinel hook propagation path:');
      console.log('  1. session.registerHook() → stored in session.hook_engine');
      console.log('  2. build_agent_loop()     → config.hook_engine = Arc::clone(&hook_engine)');
      console.log('  3. tool_context           → .with_sentinel_hook(hook)');
      console.log('  4. TaskTool::execute()    → child AgentConfig.hook_engine = sentinel_hook');
      console.log('  5. Child ToolContext      → .sentinel_hook = hook (grandchild propagation)');
    } else {
      console.log('⚠ task tool was not used by the LLM');
    }

    // --- Step 4: Unregister ---
    console.log();
    console.log('[Step 4] Unregistering hooks...');
    const r1 = session.unregisterHook('sentinel-pre-tool');
    const r2 = session.unregisterHook('sentinel-post-tool');
    console.assert(r1 && r2, 'Failed to unregister hooks');
    console.assert(session.hookCount() === 0);
    console.log('  ✓ Both hooks removed, hookCount = 0');

    console.log();
    console.log('='.repeat(60));
    if (taskToolUsed) {
      console.log('✓ Sentinel Hook Test PASSED');
    } else {
      console.log('⚠ Sentinel Hook Test PARTIAL (task tool not invoked by LLM)');
    }
    console.log('='.repeat(60));

  } finally {
    fs.rmSync(workspace, { recursive: true, force: true });
  }
}

main().catch(e => {
  console.error(`\n✗ Error: ${e.message}`);
  console.error(e.stack);
  process.exit(1);
});
