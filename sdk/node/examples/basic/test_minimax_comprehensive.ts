#!/usr/bin/env npx tsx
/**
 * A3S Code Node.js SDK — MiniMax Model Comprehensive Integration Test (TypeScript)
 *
 * Uses MiniMax-M2.7-highspeed model via openai-compatible API proxy.
 * Tests the default SDK surface: Session, tools, commands, streaming, and errors.
 *
 * Run with: npx tsx basic/test_minimax_comprehensive.ts
 */

import { Agent, Session } from '../../index.js';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';

// ============================================================================
// Test Configuration
// ============================================================================

const MODEL = 'openai/MiniMax-M2.7-highspeed';
const PROVIDER_BASE_URL = process.env.OPENAI_BASE_URL || 'https://api.openai.com/v1/';
const HAS_REAL_PROVIDER_CONFIG = Boolean(
  process.env.A3S_CONFIG ||
  (process.env.OPENAI_API_KEY && process.env.OPENAI_API_KEY !== 'your-api-key')
);

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

  private createInlineConfig(): string {
    const apiKey = process.env.OPENAI_API_KEY || 'your-api-key';
    return `
default_model = "${MODEL}"

providers "openai" {
  api_key  = "${apiKey}"
  base_url = "${PROVIDER_BASE_URL}"

  models "MiniMax-M2.7-highspeed" {
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

    const config = process.env.A3S_CONFIG || this.createInlineConfig();

    // Create agent with configured MiniMax model
    console.log('Creating Agent with MiniMax-M2.7-highspeed...');
    try {
      this.agent = await Agent.create(config);
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

    await this.runTest('Create session with explicit allow policy', async () => {
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
      if (!session) throw new Error('Session is null');
      return session;
    });

    await this.runTest('Send simple prompt to MiniMax model', async () => {
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
      const result = await session.send('Say "Hello from MiniMax-M2.7-highspeed" exactly.');
      if (!result.text.includes('Hello')) {
        throw new Error(`Expected "Hello" in response, got: ${result.text.slice(0, 100)}`);
      }
      console.log(`    Response: ${result.text.slice(0, 80)}...`);
    });

    await this.runTest('List available commands', async () => {
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
      const commands = session.listCommands();
      if (!Array.isArray(commands) || commands.length === 0) {
        throw new Error('Commands should not be empty');
      }
      const hasHelp = commands.some((c) => c.name === 'help');
      if (!hasHelp) throw new Error('Built-in /help should be registered');
      console.log(`    Available commands: ${commands.length}`);
    });

    await this.runTest('/model command reports MiniMax', async () => {
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
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
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
      const result = await session.send('/help');
      if (!result.text.includes('/help')) {
        throw new Error('Help should mention /help');
      }
      console.log(`    Help output: ${result.text.slice(0, 100)}...`);
    });

    await this.runTest('Execute /tools command', async () => {
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
      const result = await session.send('/tools');
      if (!result.text.includes('Tools')) {
        throw new Error('Should list tools');
      }
      console.log(`    Tools output: ${result.text.slice(0, 100)}...`);
    });

    await this.runTest('Execute /cost command', async () => {
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
      // First make a real request to accumulate cost
      await session.send('What is 2+2?');
      const result = await session.send('/cost');
      if (!result.text.includes('Model')) {
        throw new Error('Cost should include model info');
      }
      console.log(`    Cost output: ${result.text.trim()}`);
    });

    await this.runTest('Execute /history command', async () => {
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
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
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });

      session.registerCommand('status', 'Show session status', (args, ctx) => {
        return `workspace=${ctx.workspace};tools=${ctx.toolNames.length};args=${args || '(none)'}`;
      });

      const commands = session.listCommands();
      const hasStatus = commands.some((c) => c.name === 'status');
      if (!hasStatus) throw new Error('Custom /status command should be registered');
    });

    await this.runTest('Execute custom slash command', async () => {
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });

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
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });

      session.registerCommand('checkcmd', 'Check command', () => 'checked');
      const commands = session.listCommands();
      const hasCheck = commands.some((c) => c.name === 'checkcmd');
      if (!hasCheck) throw new Error('checkcmd should be registered');
      console.log(`    Registered commands count: ${commands.length}`);
    });
  }

  // ============================================================================
  // Test 5: Event Streaming
  // ============================================================================

  async testEventStreaming(): Promise<void> {
    console.log('\n== Test 5: Event Streaming ==');
    console.log('-'.repeat(70));

    await this.runTest('Stream events from session', async () => {
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
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
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
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
  // Test 6: Error Handling
  // ============================================================================

  async testErrorHandling(): Promise<void> {
    console.log('\n== Test 9: Error Handling ==');
    console.log('-'.repeat(70));

    await this.runTest('Handle invalid command gracefully', async () => {
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
      // Invalid command should still return some response, not throw
      const result = await session.send('/nonexistentcommand12345');
      // Should not throw, result is returned
      console.log(`    Response received: ${result.text.slice(0, 50)}...`);
    });

    await this.runTest('Handle empty send gracefully', async () => {
      const session = this.agent.session(this.workspace, { permissionPolicy: { defaultDecision: 'allow' } });
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
  if (!HAS_REAL_PROVIDER_CONFIG) {
    console.log('MiniMax comprehensive test skipped.');
    console.log('Set OPENAI_API_KEY or A3S_CONFIG to run this real-provider example.');
    process.exit(0);
  }

  const test = new MiniMaxComprehensiveTest();

  try {
    await test.setup();

    // Run all tests - each prints its own pass/fail
    await test.testBasicSession();
    await test.testToolExecution();
    await test.testCustomCommands();
    await test.testEventStreaming();
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
