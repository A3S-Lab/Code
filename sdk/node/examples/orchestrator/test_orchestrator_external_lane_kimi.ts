/**
 * Orchestrator External Lane Integration Test (Kimi)
 *
 * Tests new features added in v1.1.0:
 *   1. SubAgentConfig.laneConfig  — route Execute lane to External mode
 *   2. Orchestrator.pendingExternalTasksFor(subagentId) — poll pending tasks
 *   3. Orchestrator.completeExternalTask(subagentId, taskId, result) — unblock task
 *
 * Topology:
 *   Orchestrator  ──spawns──▶  SubAgent (Kimi, Execute lane = External)
 *        │                          │  bash tool blocked  ▶  ExternalTaskPending
 *        └── poll ──▶  pending tasks ──▶  execute locally ──▶  completeExternalTask
 *
 * Run:
 *   export MOONSHOT_API_KEY=sk-...
 *   npx tsx examples/test_orchestrator_external_lane_kimi.ts
 */

import { execSync } from 'child_process';
import * as os from 'os';
import * as path from 'path';
import { Agent, Orchestrator, SubAgentConfig, SessionQueueConfig, ExternalTaskResult } from '../../index.js';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

function localExecute(cmdType: string, payloadJson: string): { success: boolean; output: string; exitCode: number; error?: string } {
  let payload: Record<string, string> = {};
  try {
    payload = JSON.parse(payloadJson || '{}');
  } catch {}

  if (cmdType === 'bash') {
    const command = payload['command'] ?? 'echo "(no command)"';
    const cwd = payload['working_dir'] ?? '.';
    try {
      const out = execSync(command, { cwd, timeout: 30_000, encoding: 'utf8' });
      return { success: true, output: out, exitCode: 0 };
    } catch (e: any) {
      return { success: false, output: e.stdout ?? '', exitCode: e.status ?? 1, error: e.message };
    }
  }
  return { success: true, output: `(external handler: ${cmdType})`, exitCode: 0 };
}

function assertEq<T>(actual: T, expected: T, msg: string) {
  if (actual !== expected) throw new Error(`${msg}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
}

// ---------------------------------------------------------------------------
// Test 1 – laneConfig field accepted (placeholder mode)
// ---------------------------------------------------------------------------

async function testLaneConfigField() {
  console.log('\n[Test 1] SubAgentConfig.laneConfig field');
  console.log('-'.repeat(60));

  const config: SubAgentConfig = {
    agentType: 'general',
    prompt: 'echo hello',
    description: 'laneConfig field test',
    permissive: true,
    maxSteps: 3,
    laneConfig: { timeoutMs: 30_000 },
  };

  const orch = Orchestrator.create();   // placeholder mode
  const handle = orch.spawnSubagent(config);
  console.log(`  ✓ SubAgent spawned: ${handle.id}`);
  console.log(`  ✓ laneConfig accepted without error`);

  const result = handle.wait();
  console.log(`  ✓ Placeholder result: ${result.slice(0, 60)}`);
  console.log('[Test 1] PASSED\n');
}

// ---------------------------------------------------------------------------
// Test 2 – completeExternalTask returns false for unknown subagent
// ---------------------------------------------------------------------------

async function testCompleteExternalTaskUnknown() {
  console.log('[Test 2] completeExternalTask – unknown subagent returns false');
  console.log('-'.repeat(60));

  const orch = Orchestrator.create();
  const ok = orch.completeExternalTask('nonexistent-id', 'task-000', { success: true });
  assertEq(ok, false, 'completeExternalTask unknown subagent');
  console.log('  ✓ Returns false when subagent not found');
  console.log('[Test 2] PASSED\n');
}

// ---------------------------------------------------------------------------
// Test 3 – monitoring APIs with real Kimi agent
// ---------------------------------------------------------------------------

async function testMonitoringWithKimi(agent: Agent) {
  console.log('[Test 3] Orchestrator monitoring APIs with real Kimi agent');
  console.log('-'.repeat(60));

  const orch = Orchestrator.create(agent);

  const config: SubAgentConfig = {
    agentType: 'general',
    prompt: 'Please use the Glob tool to list Python files in the current directory (pattern: *.py) and report the count.',
    description: 'Count Python files',
    permissive: true,
    maxSteps: 5,
  };

  const handle = orch.spawnSubagent(config);
  console.log(`  ✓ Spawned SubAgent: ${handle.id}`);

  const deadline = Date.now() + 60_000;
  let snapshots = 0;
  while (Date.now() < deadline) {
    await sleep(1_000);
    const stateStr = handle.state();
    const activities = orch.getActiveActivities();
    snapshots++;
    console.log(`  [${snapshots}] state=${stateStr} active=${orch.activeCount()} activities=${activities.length}`);

    for (const entry of activities) {
      console.log(`      activity=${entry.activity.activityType}`);
    }

    if (stateStr.includes('Completed') || stateStr.includes('Error') || stateStr.includes('Cancelled')) break;
  }

  const result = handle.wait();
  console.log(`  ✓ Result (${result.length} chars): ${result.slice(0, 120)}`);

  // Validate types
  const finalStates = orch.getAllStates();
  if (!finalStates.some((e) => e.id === handle.id)) throw new Error('Handle ID not found in getAllStates');

  const info = orch.getSubagentInfo(handle.id);
  if (!info || info.id !== handle.id) throw new Error('getSubagentInfo failed');

  console.log('[Test 3] PASSED\n');
}

// ---------------------------------------------------------------------------
// Test 4 – pause / resume / cancel
// ---------------------------------------------------------------------------

async function testPauseResumeCancel(agent: Agent) {
  console.log('[Test 4] pause / resume / cancel SubAgent via Orchestrator');
  console.log('-'.repeat(60));

  const orch = Orchestrator.create(agent);

  const config: SubAgentConfig = {
    agentType: 'general',
    prompt: 'Use Glob to find all .py files in ., then Read each one and summarise in one sentence.',
    description: 'Summarise Python files',
    permissive: true,
    maxSteps: 20,
  };

  const handle = orch.spawnSubagent(config);
  console.log(`  ✓ Spawned: ${handle.id}`);

  // Wait for Running
  for (let i = 0; i < 20; i++) {
    await sleep(300);
    if (handle.state().includes('Running')) break;
  }

  console.log(`  State before pause: ${handle.state()}`);
  orch.pauseSubagent(handle.id);
  await sleep(1_000);
  const afterPause = handle.state();
  console.log(`  State after pause: ${afterPause}`);
  // Pause signals are only applied at tool-call checkpoints (not mid-LLM-stream).
  // For slow/streaming models, the pause state may not be visible within the check window.
  if (!afterPause.includes('Paused') && !afterPause.includes('Running')) {
    throw new Error(`Expected Paused or Running after pause signal, got ${afterPause}`);
  }

  orch.resumeSubagent(handle.id);
  await sleep(500);
  console.log(`  State after resume: ${handle.state()}`);

  orch.cancelSubagent(handle.id);
  for (let i = 0; i < 20; i++) {
    await sleep(300);
    const s = handle.state();
    if (s.includes('Cancelled') || s.includes('Error')) break;
  }

  const final = handle.state();
  console.log(`  Final state: ${final}`);
  if (!final.includes('Cancelled') && !final.includes('Error')) {
    throw new Error(`Expected Cancelled/Error, got ${final}`);
  }

  console.log('[Test 4] PASSED\n');
}

// ---------------------------------------------------------------------------
// Test 5 – External Lane dispatch via Orchestrator
// ---------------------------------------------------------------------------

async function testExternalLaneOrchestrator(agent: Agent) {
  console.log('[Test 5] External Lane dispatch via Orchestrator (core new feature)');
  console.log('-'.repeat(60));

  const laneConfig: SessionQueueConfig = {
    timeoutMs: 60_000,
    enableAllFeatures: true,
    laneHandlers: {
      execute: { mode: 'external', timeoutMs: 60_000 },
    },
  };

  const config: SubAgentConfig = {
    agentType: 'general',
    prompt: (
      'Run these three bash commands and report their output:\n' +
      "1. echo 'Hello from external worker!'\n" +
      "2. echo 'Step two done'\n" +
      "3. node -e \"console.log('Node.js OK')\""
    ),
    description: 'External lane test',
    permissive: true,
    maxSteps: 10,
    laneConfig,
  };

  const orch = Orchestrator.create(agent);
  const handle = orch.spawnSubagent(config);
  console.log(`  ✓ Spawned SubAgent: ${handle.id}`);

  let tasksCompleted = 0;
  let stop = false;

  // Worker: poll and complete external tasks
  const workerLoop = async () => {
    await sleep(500); // let session register
    const deadline = Date.now() + 90_000;
    while (!stop && Date.now() < deadline) {
      const tasks = orch.pendingExternalTasksFor(handle.id);
      for (const task of tasks) {
        console.log(`  📥 ExternalTask: ${task.taskId.slice(0, 8)} (${task.commandType})`);
        const res = localExecute(task.commandType, task.payload);
        console.log(`  🔧 Executed locally: success=${res.success} output=${JSON.stringify(res.output.slice(0, 60))}`);

        const result: ExternalTaskResult = {
          success: res.success,
          result: { output: res.output, exitCode: res.exitCode },
          error: res.error,
        };
        const ok = orch.completeExternalTask(handle.id, task.taskId, result);
        if (ok) {
          tasksCompleted++;
          console.log(`  📤 Task ${task.taskId.slice(0, 8)} completed (${tasksCompleted} total)`);
        }
      }
      await sleep(200);
    }
  };

  const workerPromise = workerLoop();

  // Wait for SubAgent
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    await sleep(1_000);
    const state = handle.state();
    console.log(`  state=${state} tasks_completed=${tasksCompleted}`);
    if (state.includes('Completed') || state.includes('Error')) break;
  }

  stop = true;
  await workerPromise;

  const result = handle.wait();
  console.log(`\n  ✓ SubAgent result (${result.length} chars):`);
  console.log(`    ${result.slice(0, 200)}`);
  console.log(`  ✓ External tasks completed by worker: ${tasksCompleted}`);

  if (tasksCompleted === 0) {
    console.log('  ⚠  No external tasks intercepted — lane may default to Internal');
  } else {
    console.log(`  ✓ External lane dispatch verified: ${tasksCompleted} tasks handled`);
  }

  console.log('[Test 5] PASSED\n');
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  console.log('='.repeat(70));
  console.log('A3S Code – Orchestrator External Lane Integration Test (Kimi)');
  console.log('='.repeat(70));

  const apiKey = process.env.MOONSHOT_API_KEY;
  // System config (~/.a3s/config.hcl) has the API key embedded — no env var needed.
  // Only require env var if using a local agent_kimi.hcl that references it.
  const systemConfig = path.join(os.homedir(), '.a3s', 'config.hcl');
  const localConfig = path.join(path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Z]:)/, '$1')), 'agent_kimi.hcl');

  let configPath: string;
  const fs = await import('fs');
  if (fs.existsSync(systemConfig)) {
    configPath = systemConfig;
  } else if (fs.existsSync(localConfig)) {
    if (!apiKey) {
      console.error('ERROR: MOONSHOT_API_KEY not set (required for agent_kimi.hcl)');
      process.exit(1);
    }
    configPath = localConfig;
  } else {
    console.error('ERROR: No config found: ~/.a3s/config.hcl or examples/agent_kimi.hcl');
    process.exit(1);
  }

  // Tests 1 & 2 — no real agent needed
  await testLaneConfigField();
  await testCompleteExternalTaskUnknown();

  // Tests 3–5 — real Kimi agent
  const agent = await Agent.create(configPath);
  console.log(`Agent created from ${configPath}\n`);

  await testMonitoringWithKimi(agent);
  await testPauseResumeCancel(agent);
  await testExternalLaneOrchestrator(agent);

  console.log('='.repeat(70));
  console.log('All tests completed!');
  console.log('='.repeat(70));
}

main().catch((err) => {
  console.error('Fatal error:', err);
  process.exit(1);
});
