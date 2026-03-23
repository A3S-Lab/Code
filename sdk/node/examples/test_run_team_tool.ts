#!/usr/bin/env npx tsx
/**
 * A3S Code Node.js SDK - run_team Built-in Tool Integration Test (TypeScript)
 *
 * Demonstrates the `run_team` built-in tool, which the LLM can call autonomously
 * (or which you can invoke directly via session.tool()) to coordinate a
 * Lead → Worker → Reviewer team workflow.
 *
 * The tool is part of the task delegation triad:
 *   - task          — single subagent, blocks until done
 *   - parallel_task — fan-out to multiple subagents concurrently
 *   - run_team      — Lead decomposes goal, Workers execute, Reviewer approves/rejects
 *
 * Run with: npx tsx examples/test_run_team_tool.ts
 */

import { Agent, Session, ToolResult } from '../index.js';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

function findConfig(): string {
  const candidates = [
    path.join(__dirname, '..', '..', '..', '..', '..', '.a3s', 'config.hcl'),
    path.join(process.env.HOME ?? '', '.a3s', 'config.hcl'),
  ];
  for (const p of candidates) {
    if (fs.existsSync(p)) return p;
  }
  throw new Error('Config not found. Please create ~/.a3s/config.hcl');
}

/**
 * Test 1: Call run_team via session.tool() directly (no LLM round-trip).
 *
 * This is the most reliable way to invoke run_team from SDK code when you
 * know exactly what goal you want the team to tackle.
 */
async function testRunTeamDirect(agent: Agent): Promise<void> {
  console.log('\n--- Test 1: run_team via session.tool() ---');

  const session: Session = agent.session('.');

  const result: ToolResult = await session.tool('run_team', {
    goal: 'List the top-level directories in this project and briefly describe each one.',
    lead_agent: 'general',
    worker_agent: 'general',
    reviewer_agent: 'general',
    max_steps: 5,
  });

  console.log(`Exit code : ${result.exitCode}`);
  console.log(`Output    :\n${result.output}`);

  if (result.exitCode !== 0) {
    throw new Error(`Expected success, got: ${result.output}`);
  }
  if (!result.output.includes('Team run complete')) {
    throw new Error('Expected "Team run complete" in output');
  }

  console.log('✓ Test 1 passed\n');
}

/**
 * Test 2: Only 'goal' is required — all agent types default to 'general'.
 */
async function testRunTeamDefaults(agent: Agent): Promise<void> {
  console.log('--- Test 2: run_team with defaults ---');

  const session: Session = agent.session('.');

  const result: ToolResult = await session.tool('run_team', {
    goal: 'Count the Rust source files in the project.',
  });

  console.log(`Exit code : ${result.exitCode}`);
  console.log(`Output    :\n${result.output}`);

  if (result.exitCode !== 0) throw new Error(`Unexpected failure: ${result.output}`);
  if (!result.output.includes('Team run complete')) {
    throw new Error('Expected "Team run complete" in output');
  }

  console.log('✓ Test 2 passed\n');
}

/**
 * Test 3: LLM autonomously selects run_team.
 *
 * When the delegate-task skill is loaded, the LLM sees run_team in its
 * toolbox and can choose it for goals that require decomposition + review.
 */
async function testRunTeamViaLlm(agent: Agent): Promise<void> {
  console.log('--- Test 3: LLM-driven run_team (via session.send) ---');

  const session: Session = agent.session('.');

  const response = await session.send(
    'Use the run_team tool to coordinate a quick review of this project\'s ' +
    'directory structure. The team should identify the main components.'
  );

  console.log(`LLM response:\n${response.text}`);
  console.log('✓ Test 3 passed\n');
}

async function main(): Promise<void> {
  console.log('=== run_team Built-in Tool — Node.js SDK Integration Test ===\n');

  const configPath = findConfig();
  console.log(`Config: ${configPath}\n`);

  const agent = await Agent.create(configPath);
  console.log('Agent created.\n');

  await testRunTeamDirect(agent);
  await testRunTeamDefaults(agent);
  await testRunTeamViaLlm(agent);

  console.log('='.repeat(60));
  console.log('All run_team tool tests passed.');
  console.log('='.repeat(60));
}

main().catch((err: unknown) => {
  console.error('Test failed:', err);
  process.exit(1);
});
