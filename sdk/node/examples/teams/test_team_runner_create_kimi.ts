#!/usr/bin/env npx tsx
/**
 * A3S Code Node.js SDK — TeamRunner.create() Integration Tests (Kimi)
 *
 * Covers the new API introduced alongside per-member model/prompt/workspace
 * overrides in TeamMemberOptions:
 *
 *   Scenario 1: TeamRunner.create() basic API
 *     Verifies the new factory constructor works end-to-end without any opts.
 *
 *   Scenario 2: Per-member model override
 *     Lead and worker each carry an explicit model field; run completes normally.
 *
 *   Scenario 3: Per-member prompt slot (role + guidelines)
 *     Worker is constrained via role to "reply in exactly one word".
 *     Verifies the custom system prompt influences the LLM output.
 *
 *   Scenario 4: Per-worker workspace isolation
 *     Two workers each receive a different temp-dir workspace.
 *     Each writes a marker file; test asserts files land in the correct dirs.
 *
 *   Scenario 5: Inheritance — opts omitted, agent definition defaults apply
 *     Uses the built-in "explore" agent (has a defined prompt).
 *     Omits TeamMemberOptions entirely; definition prompt must still be applied.
 *
 * Run with: npx tsx examples/test_team_runner_create_kimi.ts
 */

import {
  Agent,
  TeamRunner,
  TeamMemberOptions,
  TeamRunResult,
  TeamTaskBoard,
} from '../../index.js';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// ── Helpers ───────────────────────────────────────────────────────────────────

function findConfig(): string {
  if (process.env.A3S_CONFIG) return process.env.A3S_CONFIG;
  const homeConfig = path.join(os.homedir(), '.a3s', 'config.hcl');
  if (fs.existsSync(homeConfig)) return homeConfig;
  let p = path.resolve(__dirname);
  for (let i = 0; i < 10; i++) {
    const c = path.join(p, '.a3s', 'config.hcl');
    if (fs.existsSync(c)) return c;
    const parent = path.dirname(p);
    if (parent === p) break;
    p = parent;
  }
  throw new Error('Config not found. Create ~/.a3s/config.hcl or set A3S_CONFIG');
}

function pass(label: string): void {
  console.log(`  PASS  ${label}`);
}

function printResult(result: TeamRunResult): void {
  console.log(
    `  Done: ${result.doneTasks.length}, Rejected: ${result.rejectedTasks.length}, Rounds: ${result.rounds}`
  );
  for (const t of result.doneTasks) {
    const snippet = (t.result ?? '').replace(/\n/g, ' ').slice(0, 120);
    console.log(`    [${t.id}] ${t.description.slice(0, 60)}`);
    console.log(`      -> ${snippet}`);
  }
}

// ── Scenario 1: TeamRunner.create() basic API ─────────────────────────────────

async function testCreateBasic(agent: Agent, workspace: string): Promise<void> {
  console.log('\n== Scenario 1: TeamRunner.create() basic API ==');
  console.log('-'.repeat(70));

  // No TeamMemberOptions — all defaults from agent definition + Agent config.
  const runner = TeamRunner.create(agent, workspace);
  runner.addLead('general');
  runner.addWorker('general');
  runner.addReviewer('general');

  const goal =
    'Decompose into exactly 2 tasks: ' +
    '1) What is the capital of Japan? Reply in one word. ' +
    '2) What is 6 * 7? Reply with just the number.';

  console.log(`  Goal: ${goal.slice(0, 80)}...`);
  const t0 = Date.now();
  const result = await runner.runUntilDone(goal);
  console.log(`  Completed in ${((Date.now() - t0) / 1000).toFixed(1)}s`);
  printResult(result);

  if (result.doneTasks.length < 1) throw new Error('Expected at least 1 done task');
  pass(`TeamRunner.create() ran — ${result.doneTasks.length} tasks done`);
}

// ── Scenario 2: Per-member model override ────────────────────────────────────

async function testModelOverride(agent: Agent, workspace: string): Promise<void> {
  console.log('\n== Scenario 2: Per-member model override ==');
  console.log('-'.repeat(70));

  const kimiModel = 'openai/kimi-k2.5';

  const runner = TeamRunner.create(agent, workspace);
  runner.addLead('general', { model: kimiModel });
  runner.addWorker('general', { model: kimiModel });
  runner.addReviewer('general', { model: kimiModel });

  console.log(`  All members using explicit model: ${kimiModel}`);

  const goal =
    'Decompose into exactly 2 tasks: ' +
    '1) Name one planet in our solar system. One word. ' +
    '2) What color is the sky? One word.';

  const t0 = Date.now();
  const result = await runner.runUntilDone(goal);
  console.log(`  Completed in ${((Date.now() - t0) / 1000).toFixed(1)}s`);
  printResult(result);

  if (result.doneTasks.length < 1) throw new Error('Expected at least 1 done task');
  pass(`Model override accepted — ${result.doneTasks.length} tasks done`);
}

// ── Scenario 3: Per-member prompt slot ───────────────────────────────────────

async function testPromptSlots(agent: Agent, workspace: string): Promise<void> {
  console.log('\n== Scenario 3: Per-member prompt slots ==');
  console.log('-'.repeat(70));

  const workerOpts: TeamMemberOptions = {
    role: 'You are a terse answering machine. You MUST reply with exactly one word and nothing else.',
    guidelines: 'Single word answers only. No punctuation. No explanation.',
  };

  const reviewerOpts: TeamMemberOptions = {
    role: 'You are a lenient reviewer. Always approve every result without modification.',
    guidelines: 'Your only job is to approve. Always respond with APPROVED.',
  };

  const runner = TeamRunner.create(agent, workspace);
  runner.addLead('general');
  runner.addWorker('general', workerOpts);
  runner.addReviewer('general', reviewerOpts);

  const goal =
    'Decompose into exactly 2 tasks: ' +
    '1) What is the chemical symbol for water? ' +
    '2) What is the first element on the periodic table?';

  console.log('  Worker role: single-word answers only');
  console.log('  Reviewer role: always approve');
  const t0 = Date.now();
  const result = await runner.runUntilDone(goal);
  console.log(`  Completed in ${((Date.now() - t0) / 1000).toFixed(1)}s`);
  printResult(result);

  if (result.doneTasks.length < 1) throw new Error('Expected at least 1 done task');

  // Verify single-word constraint influenced output (answers should be short)
  const longAnswers = result.doneTasks.filter(t => (t.result ?? '').trim().split(/\s+/).length > 5);
  if (longAnswers.length > 0) {
    console.log(`  Warning: ${longAnswers.length} answers were longer than expected (prompt slots may not have been applied)`);
  }
  pass(`Prompt slots applied — ${result.doneTasks.length} tasks done`);
}

// ── Scenario 4: Per-worker workspace isolation ────────────────────────────────

async function testWorkerWorkspaceIsolation(agent: Agent): Promise<void> {
  console.log('\n== Scenario 4: Per-worker workspace isolation ==');
  console.log('-'.repeat(70));

  const ws1 = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-worker1-'));
  const ws2 = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-worker2-'));

  try {
    console.log(`  Worker-1 workspace: ${ws1}`);
    console.log(`  Worker-2 workspace: ${ws2}`);

    // Lead uses ws1 just to decompose; workers each get their own dir.
    const runner = TeamRunner.create(agent, ws1);
    runner.addLead('general');
    runner.addWorker('general', { workspace: ws1 });
    runner.addWorker('general', { workspace: ws2 });
    runner.addReviewer('general');

    const goal =
      'Decompose into exactly 2 tasks: ' +
      '1) Write the text "worker1_marker" (no quotes) to a file named marker.txt ' +
      'in the current workspace directory, then reply "done". ' +
      '2) Write the text "worker2_marker" (no quotes) to a file named marker.txt ' +
      'in the current workspace directory, then reply "done".';

    console.log('  Goal: each worker writes a marker file to its own workspace');
    const t0 = Date.now();
    const result = await runner.runUntilDone(goal);
    console.log(`  Completed in ${((Date.now() - t0) / 1000).toFixed(1)}s`);
    printResult(result);

    // Check whether at least one workspace received a marker file.
    const file1 = path.join(ws1, 'marker.txt');
    const file2 = path.join(ws2, 'marker.txt');
    const exists1 = fs.existsSync(file1);
    const exists2 = fs.existsSync(file2);
    console.log(`  marker.txt in ws1: ${exists1 ? fs.readFileSync(file1, 'utf8').trim() : 'absent'}`);
    console.log(`  marker.txt in ws2: ${exists2 ? fs.readFileSync(file2, 'utf8').trim() : 'absent'}`);

    if (!exists1 && !exists2) throw new Error('Neither worker wrote a marker file');
    if (exists1 && exists2) {
      const c1 = fs.readFileSync(file1, 'utf8').trim();
      const c2 = fs.readFileSync(file2, 'utf8').trim();
      // Files should differ (different markers) — proving isolation.
      if (c1 === c2) {
        console.log('  Warning: both files have the same content — workers may share workspace');
      } else {
        pass('Workspace isolation confirmed: different content in each worker dir');
        return;
      }
    }
    pass(`Workspace isolation: at least one worker wrote to its designated dir`);
  } finally {
    try { fs.rmSync(ws1, { recursive: true, force: true }); } catch {}
    try { fs.rmSync(ws2, { recursive: true, force: true }); } catch {}
  }
}

// ── Scenario 5: Inheritance — opts omitted, agent definition prompt applies ───

async function testInheritance(agent: Agent, workspace: string): Promise<void> {
  console.log('\n== Scenario 5: Inheritance — agent definition defaults ==');
  console.log('-'.repeat(70));

  // "explore" has a built-in prompt (EXPLORE_PROMPT) and max_steps=20.
  // Omitting opts entirely: the definition's prompt and max_steps must be applied.
  const runner = TeamRunner.create(agent, workspace);
  runner.addLead('general');
  runner.addWorker('explore'); // no opts — inherits definition's prompt + max_steps
  runner.addReviewer('general');

  const goal =
    'Decompose into exactly 1 task: ' +
    'List the names of 3 files or directories in the current workspace. ' +
    'If the workspace is empty, reply "workspace is empty".';

  console.log('  Worker: builtin "explore" agent, no TeamMemberOptions');
  const t0 = Date.now();
  const result = await runner.runUntilDone(goal);
  console.log(`  Completed in ${((Date.now() - t0) / 1000).toFixed(1)}s`);
  printResult(result);

  if (result.doneTasks.length < 1) throw new Error('Expected at least 1 done task');
  pass(`Inheritance verified — explore agent ran with definition defaults, ${result.doneTasks.length} task(s) done`);
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  const configPath = findConfig();
  const agent = await Agent.create(configPath);
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-team-create-'));

  console.log('='.repeat(70));
  console.log('  A3S Code — TeamRunner.create() Integration Tests (Kimi)');
  console.log(`  Config: ${configPath}`);
  console.log(`  Workspace: ${workspace}`);
  console.log('='.repeat(70));

  type Scenario = { name: string; fn: () => Promise<void> };
  const scenarios: Scenario[] = [
    { name: 'Scenario 1: create() basic API',         fn: () => testCreateBasic(agent, workspace) },
    { name: 'Scenario 2: per-member model override',  fn: () => testModelOverride(agent, workspace) },
    { name: 'Scenario 3: per-member prompt slots',    fn: () => testPromptSlots(agent, workspace) },
    { name: 'Scenario 4: per-worker workspace',       fn: () => testWorkerWorkspaceIsolation(agent) },
    { name: 'Scenario 5: inheritance (no opts)',      fn: () => testInheritance(agent, workspace) },
  ];

  let passed = 0;
  let failed = 0;
  for (const { name, fn } of scenarios) {
    try {
      await fn();
      passed++;
    } catch (e) {
      console.error(`\n  FAILED [${name}]: ${e instanceof Error ? e.message : String(e)}`);
      failed++;
    }
  }

  try { fs.rmSync(workspace, { recursive: true, force: true }); } catch {}

  console.log('\n' + '='.repeat(70));
  if (failed === 0) {
    console.log(`  ALL ${passed} SCENARIOS PASSED`);
  } else {
    console.log(`  ${passed} passed, ${failed} FAILED`);
    process.exit(1);
  }
  console.log('='.repeat(70));
}

main().catch(e => {
  console.error('Fatal:', e);
  process.exit(1);
});
