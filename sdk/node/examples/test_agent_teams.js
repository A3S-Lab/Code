#!/usr/bin/env node
/**
 * A3S Code Node.js SDK - Agent Teams Integration Test
 *
 * Demonstrates multi-agent team coordination using TeamRunner:
 *   - Lead decomposes a goal into tasks via LLM JSON response
 *   - Workers concurrently claim and execute tasks
 *   - Reviewer approves or rejects completed work
 *   - Rejected tasks are re-queued for retry
 *
 * Run with: node examples/test_agent_teams.js
 */

const { Agent, Team, TeamRunner } = require('../index.js');
const fs = require('fs');
const path = require('path');
const os = require('os');

/**
 * Find config file in home directory or project root.
 */
function findConfigPath() {
  const homeConfig = path.join(os.homedir(), '.a3s', 'config.hcl');
  if (fs.existsSync(homeConfig)) return homeConfig;

  const projectConfig = path.join(
    __dirname, '..', '..', '..', '..', '..', '..', '.a3s', 'config.hcl'
  );
  if (fs.existsSync(projectConfig)) return projectConfig;

  throw new Error('Config file not found. Please create ~/.a3s/config.hcl');
}

/**
 * Print a concise board summary.
 */
function printBoard(board) {
  const stats = board.stats();
  console.log(
    `  Board: total=${board.len}, open=${stats.open}, in_progress=${stats.inProgress}, ` +
    `in_review=${stats.inReview}, done=${stats.done}, rejected=${stats.rejected}`
  );
}

/**
 * Test 1: Manual task board API (no LLM required).
 */
async function testManualCoordination() {
  console.log('\n📋 Test 1: Manual Task Board Coordination');
  console.log('-'.repeat(80));

  const team = new Team('manual-test');
  team.addMember('lead', 'lead');
  team.addMember('worker-1', 'worker');
  team.addMember('reviewer', 'reviewer');

  const board = team.taskBoard();

  // Lead posts tasks
  const id1 = board.post('Refactor auth to JWT', 'lead');
  const id2 = board.post('Write integration tests', 'lead');
  const id3 = board.post('Update API docs', 'lead');
  if (!id1 || !id2 || !id3) throw new Error('Tasks should be posted');

  console.log('  Posted 3 tasks. Board state:');
  printBoard(board);

  // Worker claims first task
  const task = board.claim('worker-1');
  if (!task) throw new Error('Worker should claim a task');
  if (task.status !== 'in_progress') throw new Error(`Expected in_progress, got ${task.status}`);
  console.log(`  Worker claimed: [${task.id}] ${task.description}`);

  // Worker completes it
  board.complete(task.id, 'Auth refactored to use JWT with RS256 keys');
  const completed = board.get(task.id);
  if (completed.status !== 'in_review') throw new Error(`Expected in_review, got ${completed.status}`);
  console.log(`  Task submitted for review → status: ${completed.status}`);

  // Reviewer rejects
  board.reject(task.id);
  const rejected = board.get(task.id);
  if (rejected.status !== 'rejected') throw new Error(`Expected rejected, got ${rejected.status}`);
  console.log(`  Reviewer rejected → status: ${rejected.status}`);

  // Worker re-claims and completes the rejected task
  const retried = board.claim('worker-1');
  if (!retried || retried.id !== task.id) throw new Error('Should reclaim rejected task');
  board.complete(task.id, 'Auth refactored with proper token rotation');
  board.approve(task.id);
  const approved = board.get(task.id);
  if (approved.status !== 'done') throw new Error(`Expected done, got ${approved.status}`);
  console.log(`  After retry: reviewer approved → status: ${approved.status}`);

  // Verify by_status
  const doneTasks = board.byStatus('done');
  const openTasks = board.byStatus('open');
  if (doneTasks.length !== 1) throw new Error('Expected 1 done task');
  if (openTasks.length !== 2) throw new Error('Expected 2 open tasks');
  console.log(`  Done: ${doneTasks.length}, Open: ${openTasks.length}`);

  console.log('\n✅ Test 1 passed: Manual task board coordination works\n');
}

/**
 * Test 2: Full TeamRunner workflow with real LLM.
 */
async function testTeamRunnerWorkflow() {
  console.log('\n🤖 Test 2: TeamRunner — Automated Lead → Worker → Reviewer Workflow');
  console.log('-'.repeat(80));

  const configPath = findConfigPath();
  const agent = await Agent.create(configPath);

  const team = new Team('code-review-team', {
    maxTasks: 20,
    channelBuffer: 128,
    maxRounds: 8,
    pollIntervalMs: 100,
  });
  team.addMember('lead', 'lead');
  team.addMember('worker-1', 'worker');
  team.addMember('reviewer', 'reviewer');

  console.log(`  Team created with ${team.memberCount} members`);

  const runner = new TeamRunner(team);
  runner.bindSession('lead',     agent.session('.'));
  runner.bindSession('worker-1', agent.session('.'));
  runner.bindSession('reviewer', agent.session('.'));

  const goal =
    'Audit this JavaScript codebase for code quality issues. ' +
    'Identify and document the top 3 most important improvements.';
  console.log(`  Goal: ${goal}`);
  console.log('  Running team workflow (Lead decomposes → Workers execute → Reviewer approves)...');

  const start = Date.now();
  const result = await runner.runUntilDone(goal);
  const elapsed = ((Date.now() - start) / 1000).toFixed(1);

  console.log(`\n  Completed in ${elapsed}s, ${result.rounds} reviewer rounds`);
  console.log(`  Done tasks: ${result.doneTasks.length}`);
  console.log(`  Rejected tasks: ${result.rejectedTasks.length}`);

  for (const task of result.doneTasks) {
    const snippet = (task.result || '').slice(0, 120).replace(/\n/g, ' ');
    const ellipsis = (task.result || '').length > 120 ? '...' : '';
    console.log(`\n  ✓ [${task.id.slice(0, 8)}] ${task.description}`);
    console.log(`    Result: ${snippet}${ellipsis}`);
  }

  if (result.doneTasks.length === 0) throw new Error('At least one task should be completed');
  console.log('\n✅ Test 2 passed: TeamRunner executed end-to-end workflow\n');
}

/**
 * Test 3: TeamRunner without a reviewer — tasks reach InReview state.
 */
async function testTeamRunnerNoReviewer() {
  console.log('\n🔧 Test 3: TeamRunner Without Reviewer (tasks reach InReview)');
  console.log('-'.repeat(80));

  const configPath = findConfigPath();
  const agent = await Agent.create(configPath);

  const team = new Team('no-reviewer-team', { maxRounds: 3, maxTasks: 10, channelBuffer: 128, pollIntervalMs: 50 });
  team.addMember('lead', 'lead');
  team.addMember('worker-1', 'worker');
  // No reviewer bound

  const runner = new TeamRunner(team);
  runner.bindSession('lead',     agent.session('.'));
  runner.bindSession('worker-1', agent.session('.'));

  const board = runner.taskBoard();
  const result = await runner.runUntilDone('Count the number of JavaScript files in this project');

  const inReview = board.byStatus('in_review');
  const done = board.byStatus('done');
  console.log(`  InReview: ${inReview.length}, Done: ${done.length}`);

  if (inReview.length + done.length === 0) throw new Error('Tasks should have been executed');
  console.log('\n✅ Test 3 passed: Team runs correctly without a reviewer\n');
}

async function main() {
  console.log('='.repeat(80));
  console.log('  A3S Code — Agent Teams Integration Tests');
  console.log('='.repeat(80));

  try {
    await testManualCoordination();
    await testTeamRunnerWorkflow();
    await testTeamRunnerNoReviewer();

    console.log('='.repeat(80));
    console.log('  ✅ All agent team tests passed!');
    console.log('='.repeat(80));
  } catch (err) {
    console.error(`\n❌ Test failed: ${err.message}`);
    process.exit(1);
  }
}

main();
