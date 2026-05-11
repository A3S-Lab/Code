#!/usr/bin/env npx tsx
/**
 * Integration test: generate_object tool via A3S Code Node.js SDK
 *
 * Tests structured JSON output using MiniMax-M2.7-highspeed model.
 * Config is loaded from .a3s/config.acl (never hardcoded).
 *
 * Run: cd crates/code/sdk/node/examples && npx tsx basic/test_generate_object.ts
 */

import { Agent, Session } from '../../index.js';
import * as fs from 'fs';
import * as path from 'path';
import * as os from 'os';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// ============================================================================
// Config loader — reads .a3s/config.acl at runtime, never hardcodes secrets
// ============================================================================

function loadConfig(): string {
  const candidates = [
    path.resolve(__dirname, '../../../../../../.a3s/config.acl'),
    path.resolve(__dirname, '../../../../../.a3s/config.acl'),
    path.resolve(__dirname, '../../../../.a3s/config.acl'),
  ];
  for (const p of candidates) {
    if (fs.existsSync(p)) {
      return fs.readFileSync(p, 'utf-8');
    }
  }
  throw new Error(`.a3s/config.acl not found. Tried: ${candidates.join(', ')}`);
}

// ============================================================================
// Test runner
// ============================================================================

interface TestResult {
  name: string;
  passed: boolean;
  error?: string;
  durationMs: number;
}

const results: TestResult[] = [];

async function runTest(name: string, fn: () => Promise<void>): Promise<void> {
  const start = Date.now();
  try {
    await fn();
    results.push({ name, passed: true, durationMs: Date.now() - start });
    console.log(`  ✓ PASS (${Date.now() - start}ms): ${name}`);
  } catch (e) {
    results.push({ name, passed: false, error: String(e), durationMs: Date.now() - start });
    console.log(`  ✗ FAIL (${Date.now() - start}ms): ${name}`);
    console.log(`    Error: ${e}`);
  }
}

// ============================================================================
// Tests
// ============================================================================

async function main() {
  console.log('='.repeat(70));
  console.log('generate_object Integration Test (Node.js SDK + MiniMax)');
  console.log('='.repeat(70));

  const configContent = loadConfig();
  const workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'a3s-genobj-test-'));
  console.log(`Workspace: ${workspace}\n`);

  const agent = await Agent.create(configContent);
  const session = agent.session(workspace, {
    permissionPolicy: { defaultDecision: 'allow' },
  });

  // ── Test 1: Direct tool call — simple extraction ──────────────────────────

  await runTest('generate_object: extract person info (tool mode)', async () => {
    const result = await session.tool('generate_object', {
      schema: {
        type: 'object',
        required: ['name', 'age', 'city'],
        properties: {
          name: { type: 'string' },
          age: { type: 'integer' },
          city: { type: 'string' },
        },
      },
      prompt: 'Extract: "Alice Chen is 28 years old and lives in Shanghai."',
      schema_name: 'person',
      mode: 'tool',
    });

    if (result.exitCode !== 0) throw new Error(`Tool failed: ${result.output}`);
    const parsed = JSON.parse(result.output);
    const obj = parsed.object;
    if (obj.name.toLowerCase() !== 'alice chen') throw new Error(`name: ${obj.name}`);
    if (obj.age !== 28) throw new Error(`age: ${obj.age}`);
    if (!obj.city.toLowerCase().includes('shanghai')) throw new Error(`city: ${obj.city}`);
    console.log(`    Result: ${JSON.stringify(obj)}`);
  });

  // ── Test 2: Enum classification ───────────────────────────────────────────

  await runTest('generate_object: sentiment classification with enum', async () => {
    const result = await session.tool('generate_object', {
      schema: {
        type: 'object',
        required: ['sentiment', 'confidence'],
        properties: {
          sentiment: { type: 'string', enum: ['positive', 'negative', 'neutral'] },
          confidence: { type: 'number', minimum: 0, maximum: 1 },
        },
      },
      prompt: 'Classify sentiment: "This is the worst product I have ever used. Total waste of money."',
      schema_name: 'sentiment',
    });

    if (result.exitCode !== 0) throw new Error(`Tool failed: ${result.output}`);
    const parsed = JSON.parse(result.output);
    const obj = parsed.object;
    if (obj.sentiment !== 'negative') throw new Error(`sentiment: ${obj.sentiment}`);
    if (obj.confidence < 0 || obj.confidence > 1) throw new Error(`confidence: ${obj.confidence}`);
    console.log(`    Result: sentiment=${obj.sentiment}, confidence=${obj.confidence}`);
  });

  // ── Test 3: Nested schema ─────────────────────────────────────────────────

  await runTest('generate_object: nested schema (API error)', async () => {
    const result = await session.tool('generate_object', {
      schema: {
        type: 'object',
        required: ['error'],
        properties: {
          error: {
            type: 'object',
            required: ['code', 'message', 'type'],
            properties: {
              code: { type: 'integer' },
              message: { type: 'string', minLength: 3 },
              type: { type: 'string', enum: ['not_found', 'unauthorized', 'validation'] },
            },
          },
        },
      },
      prompt: 'Generate a 404 not_found API error for missing user resource.',
      schema_name: 'api_error',
    });

    if (result.exitCode !== 0) throw new Error(`Tool failed: ${result.output}`);
    const parsed = JSON.parse(result.output);
    const obj = parsed.object;
    if (obj.error.code !== 404) throw new Error(`code: ${obj.error.code}`);
    if (obj.error.type !== 'not_found') throw new Error(`type: ${obj.error.type}`);
    if (obj.error.message.length < 3) throw new Error(`message too short`);
    console.log(`    Result: ${JSON.stringify(obj.error)}`);
  });

  // ── Test 4: Array output ──────────────────────────────────────────────────

  await runTest('generate_object: array of items', async () => {
    const result = await session.tool('generate_object', {
      schema: {
        type: 'object',
        required: ['items'],
        properties: {
          items: {
            type: 'array',
            minItems: 3,
            maxItems: 5,
            items: {
              type: 'object',
              required: ['name', 'category'],
              properties: {
                name: { type: 'string' },
                category: { type: 'string', enum: ['fruit', 'vegetable', 'grain'] },
              },
            },
          },
        },
      },
      prompt: 'List 3 food items with their categories.',
      schema_name: 'food_list',
    });

    if (result.exitCode !== 0) throw new Error(`Tool failed: ${result.output}`);
    const parsed = JSON.parse(result.output);
    const items = parsed.object.items;
    if (!Array.isArray(items)) throw new Error('items is not array');
    if (items.length < 3 || items.length > 5) throw new Error(`items count: ${items.length}`);
    for (const item of items) {
      if (!['fruit', 'vegetable', 'grain'].includes(item.category)) {
        throw new Error(`invalid category: ${item.category}`);
      }
    }
    console.log(`    Result: ${items.length} items — ${items.map((i: any) => i.name).join(', ')}`);
  });

  // ── Test 5: Streaming via session.stream (agent decides to use tool) ──────

  await runTest('generate_object: streaming via session.send (agent-driven)', async () => {
    const result = await session.send(
      'Use the generate_object tool to extract the following into a structured object with fields "title" (string), "year" (integer), "genre" (string): The movie "Inception" was released in 2010 and is a sci-fi thriller.'
    );
    // The agent should have called generate_object and returned the result
    if (!result.text.toLowerCase().includes('inception') && !result.text.includes('"title"')) {
      // Check if it used the tool
      if (result.toolCallsCount === 0) {
        throw new Error(`Agent did not use generate_object tool. Response: ${result.text.slice(0, 200)}`);
      }
    }
    console.log(`    Tool calls: ${result.toolCallsCount}, tokens: ${result.totalTokens}`);
  });

  // ── Test 6: Prompt mode ───────────────────────────────────────────────────

  await runTest('generate_object: prompt mode fallback', async () => {
    const result = await session.tool('generate_object', {
      schema: {
        type: 'object',
        required: ['answer'],
        properties: {
          answer: { type: 'integer' },
        },
      },
      prompt: 'What is 7 * 8? Return the answer.',
      schema_name: 'math',
      mode: 'prompt',
    });

    if (result.exitCode !== 0) throw new Error(`Tool failed: ${result.output}`);
    const parsed = JSON.parse(result.output);
    if (parsed.object.answer !== 56) throw new Error(`answer: ${parsed.object.answer}`);
    console.log(`    Result: 7*8 = ${parsed.object.answer}`);
  });

  // ── Summary ───────────────────────────────────────────────────────────────

  console.log('\n' + '='.repeat(70));
  const passed = results.filter(r => r.passed).length;
  const failed = results.filter(r => !r.passed).length;
  console.log(`Results: ${passed} passed, ${failed} failed, ${results.length} total`);
  console.log('='.repeat(70));

  // Cleanup
  fs.rmSync(workspace, { recursive: true, force: true });

  if (failed > 0) process.exit(1);
}

main().catch(e => {
  console.error('Fatal:', e);
  process.exit(1);
});
