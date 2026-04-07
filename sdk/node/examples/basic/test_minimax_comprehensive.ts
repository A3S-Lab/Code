#!/usr/bin/env npx tsx
/**
 * A3S Code Node.js SDK — MiniMax Model Comprehensive Integration Test (TypeScript)
 *
 * Uses MiniMax-M2.7-highspeed model via openai-compatible API proxy.
 * Tests the full SDK surface: Session, Teams, Orchestrator, Tools, Events.
 *
 * Run with: npx tsx examples/test_minimax_comprehensive.ts
 */

import {
  Agent,
  Session,
  Orchestrator,
  Team,
  TeamRunner,
  TeamTaskBoard,
  TeamTask,
  TeamRunResult,
  TeamRole,
  TeamTaskStatus,
  TeamConfig,
  AgentSlot,
  SubAgentConfig,
} from '../../index.js';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

// ============================================================================
// Test Configuration
// ============================================================================

const MODEL = 'openai/MiniMax-M2.7-highspeed';
const PROVIDER_BASE_URL = process.env.OPENAI_BASE_URL || 'https://api.openai.com/v1/';

interface TestResult {
  name: string;
  passed: boolean;
  error?: string;
  durationMs: number;
}

class MiniMaxComprehensiveTest {
  private agent!: Agent;
  private workspace!: string;
  private results: TestResult[] = [];

  // ── Setup ─────────────────────────────────────────────────────────────────

  private findConfig(): string {
    const candidates = [
      process.env.A3S_CONFIG,
      path.join(os.homedir(), '.a3s', 'config.hcl'),
      path.join(__dirname, '..', 'configs', 'test_config.hcl'),
    ].filter(Boolean);
    for (const c of candidates) {
      if (c && fs.existsSync(c)) return c;
    }
    throw new Error('Config not found. Set A3S_CONFIG env var or create ~/.a3s/config.hcl');
  }

  private createInlineConfig(): string {
    const apiKey = process.env.OPENAI_API_KEY || 'your-api-key';
    return `
default_model = "${MODEL}"

providers {
  name     = "openai"
  api_key  = "${apiKey}"
  base_url = "${PROVIDER_BASE_URL}"

  models {
    id   = "MiniMax-M2.7-highspeed"
    name = "MiniMax-M2.7-highspeed"
  }
}

storage_backend = "memory"
max_tool_rounds = 30
`.trim();
  }

  async setup(): Promise<void> {
    console.log('='.repeat(70));
    console.log('MiniMax-M2.7-highspeed Comprehensive Test');
    console.log('='.repeat(70));
    console.log();

    // Create workspace
    this.workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-minimax-test-'));
    console.log(`✓ Workspace: ${this.workspace}`);
    console.log(`✓ Model: ${MODEL}`);
    console.log();

    // Create agent with inline config using MiniMax model
    console.log('Creating Agent with MiniMax-M2.7-highspeed...');
    try {
      this.agent = await Agent.create(this.createInlineConfig());
      console.log('✓ Agent created successfully');
    } catch (e) {
      throw new Error(`Failed to create agent: ${e}`);
    }
    console.log();
  }

  private async runTest<T>(name: string, fn: () => Promise<T>, timeoutMs?: number): Promise<T> {
    const start = Date.now();
    const timeout = timeoutMs ? new Promise<never>((_, reject) =>
      setTimeout(() => reject(new Error(`Test timed out after ${timeoutMs}ms`)), timeoutMs)
    ) : null;

    try {
      const result = timeout ? await Promise.race([fn(), timeout]) : await fn();
      this.results.push({
        name,
        passed: true,
        durationMs: Date.now() - start,
      });
      console.log(`  ✓ PASS (${Date.now() - start}ms): ${name}`);
      return result;
    } catch (e) {
      this.results.push({
        name,
        passed: false,
        error: String(e),
        durationMs: Date.now() - start,
      });
      console.log(`  ✗ FAIL (${Date.now() - start}ms): ${name}`);
      console.log(`    Error: ${e}`);
      throw e;
    }
  }

  // ============================================================================
  // Test 1: Basic Session & send()
  // ============================================================================

  async testBasicSession(): Promise<void> {
    console.log('\n== Test 1: Basic Session ==');
    console.log('-'.repeat(70));

    await this.runTest('Create session with permissive mode', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      if (!session) throw new Error('Session is null');
      return session;
    });

    await this.runTest('Send simple prompt to MiniMax model', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      const result = await session.send('Say "Hello from MiniMax-M2.7-highspeed" exactly.');
      if (!result.text.includes('Hello')) {
        throw new Error(`Expected "Hello" in response, got: ${result.text.slice(0, 100)}`);
      }
      console.log(`    Response: ${result.text.slice(0, 80)}...`);
    });

    await this.runTest('List available commands', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      const commands = session.listCommands();
      if (!Array.isArray(commands) || commands.length === 0) {
        throw new Error('Commands should not be empty');
      }
      const hasHelp = commands.some((c) => c.name === 'help');
      if (!hasHelp) throw new Error('Built-in /help should be registered');
      console.log(`    Available commands: ${commands.length}`);
    });

    await this.runTest('/model command reports MiniMax', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      const result = await session.send('/model');
      if (!result.text.includes('MiniMax')) {
        throw new Error(`Expected MiniMax in model response, got: ${result.text}`);
      }
      console.log(`    Model info: ${result.text.trim()}`);
    });
  }

  // ============================================================================
  // Test 2: Tool Execution
  // ============================================================================

  async testToolExecution(): Promise<void> {
    console.log('\n== Test 2: Tool Execution ==');
    console.log('-'.repeat(70));

    await this.runTest('Execute /help command', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      const result = await session.send('/help');
      if (!result.text.includes('/help')) {
        throw new Error('Help should mention /help');
      }
      console.log(`    Help output: ${result.text.slice(0, 100)}...`);
    });

    await this.runTest('Execute /tools command', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      const result = await session.send('/tools');
      if (!result.text.includes('Tools')) {
        throw new Error('Should list tools');
      }
      console.log(`    Tools output: ${result.text.slice(0, 100)}...`);
    });

    await this.runTest('Execute /cost command', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      // First make a real request to accumulate cost
      await session.send('What is 2+2?');
      const result = await session.send('/cost');
      if (!result.text.includes('Model')) {
        throw new Error('Cost should include model info');
      }
      console.log(`    Cost output: ${result.text.trim()}`);
    });

    await this.runTest('Execute /history command', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      const result = await session.send('/history');
      if (!result.text.includes('Messages')) {
        throw new Error('History should include message count');
      }
      console.log(`    History output: ${result.text.slice(0, 80)}...`);
    });
  }

  // ============================================================================
  // Test 3: Custom Slash Commands
  // ============================================================================

  async testCustomCommands(): Promise<void> {
    console.log('\n== Test 3: Custom Slash Commands ==');
    console.log('-'.repeat(70));

    await this.runTest('Register custom slash command', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });

      session.registerCommand('status', 'Show session status', (args, ctx) => {
        return `workspace=${ctx.workspace};tools=${ctx.toolNames.length};args=${args || '(none)'}`;
      });

      const commands = session.listCommands();
      const hasStatus = commands.some((c) => c.name === 'status');
      if (!hasStatus) throw new Error('Custom /status command should be registered');
    });

    await this.runTest('Execute custom slash command', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });

      session.registerCommand('echo', 'Echo arguments', (args) => {
        return `echo:${args || '(empty)'}`;
      });

      const result = await session.send('/echo hello world');
      if (!result.text.includes('hello world')) {
        throw new Error(`Expected 'hello world' in echo, got: ${result.text}`);
      }
      console.log(`    Echo result: ${result.text}`);
    });

    await this.runTest('Registered custom command appears in listCommands', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });

      session.registerCommand('checkcmd', 'Check command', () => 'checked');
      const commands = session.listCommands();
      const hasCheck = commands.some((c) => c.name === 'checkcmd');
      if (!hasCheck) throw new Error('checkcmd should be registered');
      console.log(`    Registered commands count: ${commands.length}`);
    });
  }

  // ============================================================================
  // Test 4: Scheduled Tasks & Cron
  // ============================================================================

  async testScheduledTasks(): Promise<void> {
    console.log('\n== Test 4: Scheduled Tasks & Cron ==');
    console.log('-'.repeat(70));

    await this.runTest('Schedule a one-time task', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      const taskId = session.scheduleTask('print date', 5);
      if (!taskId || !/^[0-9a-f]{8}$/.test(taskId)) {
        throw new Error(`Expected 8-char hex task ID, got: ${taskId}`);
      }
      console.log(`    Scheduled task ID: ${taskId}`);

      const tasks = session.listScheduledTasks();
      const found = tasks.some((t) => t.id === taskId);
      if (!found) throw new Error('Scheduled task should be listable');
    });

    await this.runTest('Cancel scheduled task', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      const taskId = session.scheduleTask('to cancel', 10);
      const cancelled = session.cancelScheduledTask(taskId);
      if (!cancelled) throw new Error('cancelScheduledTask should return true');
    });

    await this.runTest('Schedule recurring loop task', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      const result = await session.send('/loop 2s echo test');
      if (!result.text.includes('Scheduled')) {
        throw new Error('Loop should schedule a recurring task');
      }
      console.log(`    Loop result: ${result.text.trim()}`);
    });
  }

  // ============================================================================
  // Test 5: Event Streaming
  // ============================================================================

  async testEventStreaming(): Promise<void> {
    console.log('\n== Test 5: Event Streaming ==');
    console.log('-'.repeat(70));

    await this.runTest('Stream events from session', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      const events: string[] = [];

      const stream = await session.stream('Say "streaming works" exactly.');
      while (true) {
        const result = await stream.next();
        if (!result.value || result.done) break;
        events.push(result.value.type);
        if (result.value.type === 'end') break;
      }

      if (events.length === 0) throw new Error('Should receive at least one event');
      console.log(`    Event types: ${events.join(', ')}`);
    });

    await this.runTest('Stream text_delta events accumulate to full response', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      let fullText = '';

      const stream = await session.stream('What is 1+1?');
      while (true) {
        const result = await stream.next();
        if (!result.value || result.done) break;
        if (result.value.type === 'text_delta' && result.value.text) {
          fullText += result.value.text;
        }
        if (result.value.type === 'end') break;
      }

      if (fullText.length === 0) throw new Error('Should accumulate text');
      console.log(`    Full response: ${fullText.slice(0, 100)}...`);
    });
  }

  // ============================================================================
  // Test 6: Task Board Primitives (No LLM)
  // ============================================================================

  async testTaskBoardPrimitives(): Promise<void> {
    console.log('\n== Test 6: Task Board Primitives ==');
    console.log('-'.repeat(70));

    await this.runTest('Create team with TeamRole enum', async () => {
      const config: TeamConfig = {
        maxTasks: 20,
        maxRounds: 5,
        pollIntervalMs: 10,
        channelBuffer: 128,
      };
      const team = new Team('board-test', config);

      // Use TeamRole enum values
      team.addMember('pm', TeamRole.Lead);
      team.addMember('dev-1', TeamRole.Worker);
      team.addMember('dev-2', TeamRole.Worker);
      team.addMember('qa', TeamRole.Reviewer);

      if (team.memberCount !== 4) {
        throw new Error(`Expected 4 members, got ${team.memberCount}`);
      }
      console.log(`    Members: ${team.memberCount}`);
    });

    await this.runTest('Post and claim tasks via task board', async () => {
      const config: TeamConfig = { maxTasks: 10, maxRounds: 3, pollIntervalMs: 10, channelBuffer: 64 };
      const team = new Team('task-test', config);
      team.addMember('lead', TeamRole.Lead);
      team.addMember('worker', TeamRole.Worker);

      const board: TeamTaskBoard = team.taskBoard();

      // Post tasks
      const tid1 = board.post('Implement login feature', 'lead');
      const tid2 = board.post('Write unit tests', 'lead');
      if (!tid1 || !tid2) throw new Error('Failed to post tasks');

      // Claim task
      const task: TeamTask | null = board.claim('worker');
      if (!task) throw new Error('Should claim a task');
      if (task.status !== 'in_progress') {
        throw new Error(`Expected in_progress, got ${task.status}`);
      }
      console.log(`    Claimed task: ${task.id} (${task.status})`);

      // Complete task
      board.complete(task.id, 'Login feature implemented with JWT');
      const done: TeamTask[] = board.byStatus(TeamTaskStatus.Done);
      if (done.length !== 1) throw new Error('Should have 1 done task');
      console.log(`    Done tasks: ${done.length}`);
    });

    await this.runTest('TeamTaskStatus enum values', async () => {
      // Verify all enum values
      const statuses = [
        TeamTaskStatus.Open,
        TeamTaskStatus.InProgress,
        TeamTaskStatus.InReview,
        TeamTaskStatus.Done,
        TeamTaskStatus.Rejected,
      ];

      const names = ['Open', 'InProgress', 'InReview', 'Done', 'Rejected'];
      for (let i = 0; i < statuses.length; i++) {
        if (statuses[i] !== i) {
          throw new Error(`TeamTaskStatus.${names[i]} should be ${i}, got ${statuses[i]}`);
        }
      }
      console.log(`    All ${statuses.length} TeamTaskStatus values verified`);
    });
  }

  // ============================================================================
  // Test 7: Orchestrator with MiniMax Model
  // ============================================================================

  async testOrchestrator(): Promise<void> {
    console.log('\n== Test 7: Orchestrator ==');
    console.log('-'.repeat(70));

    await this.runTest('Create orchestrator with agent', async () => {
      const orch = Orchestrator.create(this.agent);
      if (!orch) throw new Error('Orchestrator should be created');
      console.log('    Orchestrator created');
    });

    await this.runTest('Spawn subagent with MiniMax model', async () => {
      const orch = Orchestrator.create(this.agent);

      const config: SubAgentConfig = {
        agentType: 'general',
        description: 'Test subagent',
        prompt: 'Respond with exactly: "MiniMax subagent works"',
        permissive: true,
        maxSteps: 2,
      };

      const handle = orch.spawnSubagent(config);
      if (!handle || !handle.id) throw new Error('SubAgent handle should have ID');
      console.log(`    Spawned: ${handle.id}`);

      // Wait for completion
      orch.waitAll();
      const info = orch.getSubagentInfo(handle.id);
      console.log(`    Final state: ${info?.state}`);
    });

    await this.runTest('Orchestrator active count tracking', async () => {
      const orch = Orchestrator.create(this.agent);

      const config: SubAgentConfig = {
        agentType: 'general',
        description: 'Count test',
        prompt: 'Count from 1 to 3',
        permissive: true,
        maxSteps: 3,
      };

      orch.spawnSubagent(config);
      const active = orch.activeCount();
      if (active < 1) throw new Error('Should have at least 1 active subagent');
      console.log(`    Active count: ${active}`);

      orch.waitAll();
    });

    await this.runTest('Orchestrator pause/resume', async () => {
      const orch = Orchestrator.create(this.agent);

      const config: SubAgentConfig = {
        agentType: 'general',
        description: 'Pause test',
        prompt: 'Slow task',
        permissive: true,
        maxSteps: 5,
      };

      const handle = orch.spawnSubagent(config);

      // Small delay to ensure task is running
      await new Promise((r) => setTimeout(r, 500));

      orch.pauseSubagent(handle.id);
      const info = orch.getSubagentInfo(handle.id);
      if (info?.state !== 'paused') {
        throw new Error(`Expected paused state, got ${info?.state}`);
      }
      console.log(`    Paused state: ${info?.state}`);

      orch.resumeSubagent(handle.id);
      const info2 = orch.getSubagentInfo(handle.id);
      console.log(`    Resumed state: ${info2?.state}`);

      orch.waitAll();
    });
  }

  // ============================================================================
  // Test 8: runTeam with MiniMax Model
  // ============================================================================

  async testRunTeam(): Promise<void> {
    console.log('\n== Test 8: runTeam (Lead → Worker → Reviewer) ==');
    console.log('-'.repeat(70));

    await this.runTest('runTeam with MiniMax model', async () => {
      const orch = Orchestrator.create(this.agent);

      const slots: AgentSlot[] = [
        {
          agentType: 'general',
          role: 'lead',
          description: 'Lead: decompose goal into tasks',
          prompt: '',
          permissive: true,
          maxSteps: 3,
        },
        {
          agentType: 'general',
          role: 'worker',
          description: 'Worker: execute assigned tasks',
          prompt: '',
          permissive: true,
          maxSteps: 3,
        },
        {
          agentType: 'general',
          role: 'reviewer',
          description: 'Reviewer: approve or reject results',
          prompt: '',
          permissive: true,
          maxSteps: 2,
        },
      ];

      const result: TeamRunResult = await orch.runTeam(
        'List 2 programming languages and give one fact about each.',
        this.workspace,
        slots,
      );

      console.log(`    Done: ${result.doneTasks.length}, Rejected: ${result.rejectedTasks.length}, Rounds: ${result.rounds}`);

      if (result.doneTasks.length === 0 && result.rejectedTasks.length === 0) {
        throw new Error('At least one task should be done or rejected');
      }
    }, 120000); // 2 minute timeout for team test
  }

  // ============================================================================
  // Test 9: TeamRunner Direct Usage
  // ============================================================================

  async testTeamRunner(): Promise<void> {
    console.log('\n== Test 9: TeamRunner Direct Usage ==');
    console.log('-'.repeat(70));

    await this.runTest('Create team and bind sessions', async () => {
      const config: TeamConfig = {
        maxTasks: 10,
        maxRounds: 5,
        pollIntervalMs: 100,
        channelBuffer: 64,
      };
      const team = new Team('direct-test', config);
      team.addMember('lead', TeamRole.Lead);
      team.addMember('worker', TeamRole.Worker);
      team.addMember('reviewer', TeamRole.Reviewer);

      const runner = new TeamRunner(team);
      runner.bindSession('lead', this.agent.session(this.workspace, { permissive: true }));
      runner.bindSession('worker', this.agent.session(this.workspace, { permissive: true }));
      runner.bindSession('reviewer', this.agent.session(this.workspace, { permissive: true }));

      console.log('    Team and runner configured');
    });

    await this.runTest('TeamRunner.runUntilDone', async () => {
      const config: TeamConfig = {
        maxTasks: 5,
        maxRounds: 3,
        pollIntervalMs: 100,
        channelBuffer: 32,
      };
      const team = new Team('runner-test', config);
      team.addMember('lead', TeamRole.Lead);
      team.addMember('worker', TeamRole.Worker);
      team.addMember('reviewer', TeamRole.Reviewer);

      const runner = new TeamRunner(team);
      runner.bindSession('lead', this.agent.session(this.workspace, { permissive: true }));
      runner.bindSession('worker', this.agent.session(this.workspace, { permissive: true }));
      runner.bindSession('reviewer', this.agent.session(this.workspace, { permissive: true }));

      const result: TeamRunResult = await runner.runUntilDone(
        'Name 2 data structures: array and object. Give one sentence about each.',
      );

      console.log(`    Done: ${result.doneTasks.length}, Rounds: ${result.rounds}`);
    }, 120000);
  }

  // ============================================================================
  // Test 10: Error Handling
  // ============================================================================

  async testErrorHandling(): Promise<void> {
    console.log('\n== Test 10: Error Handling ==');
    console.log('-'.repeat(70));

    await this.runTest('Handle invalid command gracefully', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      // Invalid command should still return some response, not throw
      const result = await session.send('/nonexistentcommand12345');
      // Should not throw, result is returned
      console.log(`    Response received: ${result.text.slice(0, 50)}...`);
    });

    await this.runTest('Handle empty send gracefully', async () => {
      const session = this.agent.session(this.workspace, { permissive: true });
      // Empty send might throw or return empty - either is acceptable
      try {
        await session.send('');
      } catch {
        console.log('    Empty send threw (acceptable)');
      }
    });
  }

  // ============================================================================
  // Cleanup & Report
  // ============================================================================

  cleanup(): void {
    console.log('\n== Cleanup ==');
    console.log('-'.repeat(70));

    // Clean up workspace
    try {
      fs.rmSync(this.workspace, { recursive: true, force: true });
      console.log(`✓ Removed workspace: ${this.workspace}`);
    } catch (e) {
      console.log(`✗ Failed to remove workspace: ${e}`);
    }
  }

  printReport(): void {
    console.log('\n' + '='.repeat(70));
    console.log('TEST REPORT');
    console.log('='.repeat(70));

    const passed = this.results.filter((r) => r.passed).length;
    const failed = this.results.filter((r) => !r.passed).length;
    const total = this.results.length;
    const totalTime = this.results.reduce((sum, r) => sum + r.durationMs, 0);

    console.log(`\nTotal: ${total} | Passed: ${passed} | Failed: ${failed}`);
    console.log(`Total time: ${(totalTime / 1000).toFixed(1)}s`);
    console.log();

    if (failed > 0) {
      console.log('Failed tests:');
      for (const r of this.results.filter((r) => !r.passed)) {
        console.log(`  ✗ ${r.name}`);
        console.log(`    ${r.error}`);
      }
      console.log();
    }

    console.log('Per-test timing:');
    for (const r of this.results) {
      const status = r.passed ? '✓' : '✗';
      console.log(`  ${status} ${r.name}: ${r.durationMs}ms`);
    }

    console.log('\n' + '='.repeat(70));
    if (failed === 0) {
      console.log('✓ ALL TESTS PASSED');
    } else {
      console.log(`✗ ${failed} TEST(S) FAILED`);
    }
    console.log('='.repeat(70));
  }
}

// ============================================================================
// Main
// ============================================================================

async function main(): Promise<void> {
  const test = new MiniMaxComprehensiveTest();

  try {
    await test.setup();

    // Run all tests - each prints its own pass/fail
    await test.testBasicSession();
    await test.testToolExecution();
    await test.testCustomCommands();
    await test.testScheduledTasks();
    await test.testEventStreaming();
    await test.testTaskBoardPrimitives();
    await test.testOrchestrator();
    await test.testRunTeam();
    await test.testTeamRunner();
    await test.testErrorHandling();

    test.cleanup();
    test.printReport();

    // Exit with error code if any tests failed
    const failedCount = test['results'].filter((r: TestResult) => !r.passed).length;
    process.exit(failedCount > 0 ? 1 : 0);
  } catch (e) {
    console.error('\n\n✗ FATAL ERROR:', e);
    test.cleanup();
    process.exit(1);
  }
}

main().catch((e) => {
  console.error('Unhandled error:', e);
  process.exit(1);
});
