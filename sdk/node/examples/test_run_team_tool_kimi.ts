#!/usr/bin/env npx tsx
/**
 * run_team built-in tool smoke test — Kimi model (TypeScript).
 *
 * Tests Session.runTeam() — the typed convenience wrapper that invokes the
 * `run_team` built-in tool (Lead → Worker → Reviewer) directly from SDK code,
 * without going through the Orchestrator API.
 *
 * Compared to test_run_team_kimi.ts (which tests Orchestrator.runTeam()),
 * this test exercises the new Session.runTeam() convenience method added to
 * the Node SDK alongside the RunTeamTool built-in.
 *
 * Run with:
 *   cd sdk/node
 *   npx tsx examples/test_run_team_tool_kimi.ts
 */

import { Agent, Session, ToolResult } from '../index.js';
import * as path from 'path';
import * as fs from 'fs';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const CONFIG_PATH = path.join(
  __dirname, '..', '..', '..', '..', '..', '.a3s', 'config.hcl'
);

function printSection(title: string): void {
  console.log(`\n${'─'.repeat(60)}`);
  console.log(`  ${title}`);
  console.log('─'.repeat(60));
}

async function main(): Promise<void> {
  console.log('=== run_team built-in tool — Kimi integration test (TypeScript) ===\n');

  if (!fs.existsSync(CONFIG_PATH)) {
    console.error(`✗ Config not found: ${CONFIG_PATH}`);
    process.exit(1);
  }
  console.log(`✓ Config: ${CONFIG_PATH}`);

  const agent = await Agent.create(CONFIG_PATH);
  console.log('✓ Agent created (kimi-k2.5)\n');

  // ------------------------------------------------------------------
  // Test 1: session.runTeam() — typed convenience method, all defaults
  // ------------------------------------------------------------------
  printSection('Test 1: session.runTeam() with defaults');

  const session: Session = agent.session('.');
  console.log('Goal: List 3 JavaScript built-in types and describe each briefly.\n');

  const result: ToolResult = await session.runTeam(
    'List 3 JavaScript built-in types and describe each one in one sentence.',
    null, null, null,
    5, // maxSteps
  );

  console.log(`exit_code : ${result.exitCode}`);
  console.log(`output    :\n${result.output}`);

  if (result.exitCode !== 0) throw new Error(`Expected success; got exitCode=${result.exitCode}`);
  if (!result.output.includes('Team run complete')) throw new Error("Missing 'Team run complete' in output");
  console.log('✓ Test 1 passed');

  // ------------------------------------------------------------------
  // Test 2: explicit agent types
  // ------------------------------------------------------------------
  printSection('Test 2: session.runTeam() with explicit agent types');

  const session2: Session = agent.session('.');
  console.log('Goal: Count TypeScript source files in the codebase.\n');

  const result2: ToolResult = await session2.runTeam(
    'Count the number of .ts source files in the codebase and report the total.',
    'general',   // leadAgent
    'explore',   // workerAgent
    'general',   // reviewerAgent
    5,           // maxSteps
  );

  console.log(`exit_code : ${result2.exitCode}`);
  console.log(`output    :\n${result2.output}`);

  if (result2.exitCode !== 0) throw new Error(`Expected success; got exitCode=${result2.exitCode}`);
  if (!result2.output.includes('Team run complete')) throw new Error("Missing 'Team run complete'");
  console.log('✓ Test 2 passed');

  // ------------------------------------------------------------------
  // Test 3: tool() generic call — same underlying route
  // ------------------------------------------------------------------
  printSection("Test 3: session.tool('run_team', {...}) — generic call");

  const session3: Session = agent.session('.');
  const result3: ToolResult = await session3.tool('run_team', {
    goal: 'What programming language is this project primarily written in?',
    max_steps: 10,
  });

  console.log(`exit_code : ${result3.exitCode}`);
  console.log(`output    :\n${result3.output}`);

  if (result3.exitCode !== 0) throw new Error(`Unexpected failure: ${result3.output}`);
  if (!result3.output.includes('Team run complete')) throw new Error("Missing 'Team run complete'");
  console.log('✓ Test 3 passed');

  console.log(`\n${'='.repeat(60)}`);
  console.log('All tests passed ✓');
  console.log('='.repeat(60));
}

main().catch((err: unknown) => {
  console.error('\n✗ Test failed:', err);
  process.exit(1);
});
