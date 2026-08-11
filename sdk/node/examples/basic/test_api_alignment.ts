/**
 * Integration tests for Node.js SDK API alignment with Rust core.
 *
 * Tests the new SessionOptions fields added in this PR:
 *   - temperature
 *   - thinkingBudget
 *   - continuationEnabled
 *   - maxContinuationTurns
 *   - maxParallelTasks
 *   - autoDelegation / autoParallel
 *
 * Uses the kimi-k2.5 model. API key is read from KIMI_API_KEY environment
 * variable.
 *
 * Usage:
 *   KIMI_API_KEY=sk-... npx ts-node test_api_alignment.ts
 */

import { createRequire } from 'node:module';
import type {
  ArtifactStoreLimits,
  AutoDelegationOptions,
  DelegateTaskOptions,
  SessionOptions,
  ToolResultTransformPolicy,
  ToolArtifact,
  ToolResult,
  VerificationReport,
} from '@a3s-lab/code';
import * as path from 'path';
import { fileURLToPath } from 'url';

const require = createRequire(import.meta.url);
const a3sCode = require('@a3s-lab/code') as typeof import('@a3s-lab/code');
const { Agent, LocalWorkspaceBackend } = a3sCode;

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

// ACL config that reads API key from KIMI_API_KEY env var.
const KIMI_ACL_CONFIG = path.join(__dirname, 'agent_kimi_k2.5.acl');

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

type Status = 'PASS' | 'FAIL' | 'SKIP';
const results: Array<{ name: string; status: Status; detail: string }> = [];

function check(name: string, condition: boolean, detail = '') {
  const status: Status = condition ? 'PASS' : 'FAIL';
  results.push({ name, status, detail });
  const icon = condition ? '✓' : '✗';
  const msg = detail ? `  ${icon} ${name}: ${detail}` : `  ${icon} ${name}`;
  console.log(msg);
  return condition;
}

function skip(name: string, reason: string) {
  results.push({ name, status: 'SKIP', detail: reason });
  console.log(`  - ${name}: SKIP (${reason})`);
}

function section(title: string) {
  console.log(`\n${'='.repeat(60)}`);
  console.log(`  ${title}`);
  console.log('='.repeat(60));
}

// ---------------------------------------------------------------------------
// Phase 1: SessionOptions type checks (no LLM call)
// ---------------------------------------------------------------------------

section('Phase 1: SessionOptions field type checks');

// Verify new fields are accepted by TypeScript and accepted as undefined
const opts: SessionOptions = {
  temperature: undefined,
  thinkingBudget: undefined,
  continuationEnabled: undefined,
  maxContinuationTurns: undefined,
  maxParallelTasks: undefined,
  autoDelegation: undefined,
  autoParallel: undefined,
  planningMode: undefined,
  artifactStoreLimits: undefined,
  toolResultTransformPolicy: undefined,
  workspaceBackend: new LocalWorkspaceBackend(process.cwd()),
};
check('temperature field accepted', true);
check('thinkingBudget field accepted', true);
check('continuationEnabled field accepted', true);
check('maxContinuationTurns field accepted', true);
check('maxParallelTasks field accepted', true);
check('autoDelegation field accepted', true);
check('autoParallel field accepted', true);
check('planningMode field accepted', true);
check('artifactStoreLimits field accepted', true);
check('toolResultTransformPolicy field accepted', true);
check('workspaceBackend field accepted', true);

const artifactLimits: ArtifactStoreLimits = { maxArtifacts: 4, maxBytes: 1024 * 1024 };
const transformPolicy: ToolResultTransformPolicy = {
  schema: 'a3s.code.tool-result-transform-policy.v1',
  maxOutputBytes: 100 * 1024,
  headBytes: 64 * 1024,
  tailBytes: 32 * 1024,
  foldRepeatedLines: true,
  repeatedLineThreshold: 3,
  structuredSampleItems: 32,
};
const opts2: SessionOptions = {
  temperature: 0.5,
  thinkingBudget: 8000,
  continuationEnabled: false,
  maxContinuationTurns: 5,
  maxParallelTasks: 3,
  autoDelegation: { enabled: true, maxTasks: 2, minConfidence: 0.8 },
  autoParallel: false,
  planningMode: 'disabled',
  artifactStoreLimits: artifactLimits,
  toolResultTransformPolicy: transformPolicy,
};
check('temperature value 0.5', opts2.temperature === 0.5);
check('thinkingBudget value 8000', opts2.thinkingBudget === 8000);
check('continuationEnabled value false', opts2.continuationEnabled === false);
check('maxContinuationTurns value 5', opts2.maxContinuationTurns === 5);
check('maxParallelTasks value 3', opts2.maxParallelTasks === 3);
check('autoDelegation enabled value true', opts2.autoDelegation?.enabled === true);
check('autoParallel value false', opts2.autoParallel === false);
check('planningMode value disabled', opts2.planningMode === 'disabled');
check('artifactStoreLimits maxArtifacts value 4', opts2.artifactStoreLimits?.maxArtifacts === 4);
check('toolResultTransformPolicy tailBytes value', opts2.toolResultTransformPolicy?.tailBytes === 32 * 1024);

const autoDelegationOpts: AutoDelegationOptions = {
  enabled: true,
  autoParallel: false,
  minConfidence: 0.75,
  maxTasks: 4,
};
check('AutoDelegationOptions type accepted', autoDelegationOpts.autoParallel === false);

const delegatedTask: DelegateTaskOptions = {
  agent: 'explore',
  description: 'Find release tests',
  prompt: 'Locate the release verification surface.',
  maxSteps: 1,
};
check('DelegateTaskOptions maxSteps accepted', delegatedTask.maxSteps === 1);

type SessionApi = ReturnType<InstanceType<typeof Agent>['session']>;
type TaskMethod = SessionApi['task'];
type TasksMethod = SessionApi['tasks'];
type ToolDefinitionsMethod = SessionApi['toolDefinitions'];
type GetArtifactMethod = SessionApi['getArtifact'];
type RecordVerificationReportsMethod = SessionApi['recordVerificationReports'];
const taskName: keyof Pick<SessionApi, 'task'> = 'task';
const tasksName: keyof Pick<SessionApi, 'tasks'> = 'tasks';
const toolDefinitionsName: keyof Pick<SessionApi, 'toolDefinitions'> = 'toolDefinitions';
const getArtifactName: keyof Pick<SessionApi, 'getArtifact'> = 'getArtifact';
const recordVerificationReportsName: keyof Pick<SessionApi, 'recordVerificationReports'> = 'recordVerificationReports';
const writeFileName: keyof Pick<SessionApi, 'writeFile'> = 'writeFile';
const lsName: keyof Pick<SessionApi, 'ls'> = 'ls';
const editFileName: keyof Pick<SessionApi, 'editFile'> = 'editFile';
const patchFileName: keyof Pick<SessionApi, 'patchFile'> = 'patchFile';
check('task method type accepted', taskName === 'task');
check('tasks method type accepted', tasksName === 'tasks');
check('toolDefinitions method type accepted', toolDefinitionsName === 'toolDefinitions');
check('getArtifact method type accepted', getArtifactName === 'getArtifact');
check('recordVerificationReports method type accepted', recordVerificationReportsName === 'recordVerificationReports');
check('writeFile method type accepted', writeFileName === 'writeFile');
check('ls method type accepted', lsName === 'ls');
check('editFile method type accepted', editFileName === 'editFile');
check('patchFile method type accepted', patchFileName === 'patchFile');

const sessionForAgentOpts: SessionOptions = {
  role: 'Custom reviewer',
  skillDirs: ['./skills'],
};
check('sessionForAgent options accepted', sessionForAgentOpts.role === 'Custom reviewer');

const verificationReport: VerificationReport = {
  schema: 'a3s.verification_report.v1',
  subject: 'sdk:typecheck',
  status: 'passed',
  checks: [{
    id: 'check:typecheck',
    kind: 'test',
    description: 'Run TypeScript type checks',
    status: 'passed',
    required: true,
  }],
};
const maybeArtifact: ToolArtifact | null = null;
const toolResultShape: Pick<ToolResult, 'name' | 'exitCode'> = { name: 'read', exitCode: 0 };
const _typeOnlyMethods: [
  TaskMethod | undefined,
  TasksMethod | undefined,
  ToolDefinitionsMethod | undefined,
  GetArtifactMethod | undefined,
  RecordVerificationReportsMethod | undefined,
] = [undefined, undefined, undefined, undefined, undefined];
check('VerificationReport type accepted', verificationReport.checks[0].status === 'passed');
check('ToolArtifact nullable return type accepted', maybeArtifact === null);
check('ToolResult shape type accepted', toolResultShape.exitCode === 0);
check('method type aliases accepted', _typeOnlyMethods.length === 5);

// ---------------------------------------------------------------------------
// Phase 2: Integration tests against kimi-k2.5
// ---------------------------------------------------------------------------

const apiKey = process.env.KIMI_API_KEY;

async function runLiveTests(_apiKey: string) {
  section('Phase 2: Live integration tests (kimi-k2.5)');

  const cwd = process.cwd();

  // --- 2a: basic send ---
  console.log('\n  2a. Basic send');
  try {
    const agent = await Agent.create(KIMI_ACL_CONFIG);
    const session = agent.session(cwd);
    const result = await session.send('Reply with exactly: HELLO');
    check('basic send returns result', result != null);
    check('basic send has text', Boolean(result?.text));
    check('basic send contains HELLO', (result?.text ?? '').toUpperCase().includes('HELLO'),
      `got: ${result?.text?.slice(0, 80)}`);
  } catch (e: any) {
    check('basic send', false, String(e));
  }

  // --- 2b: temperature ---
  console.log('\n  2b. temperature in SessionOptions');
  try {
    const agent = await Agent.create(KIMI_ACL_CONFIG);
    const session = agent.session(cwd, {
      model: 'openai/kimi-k2.5',
      temperature: 0.0,
    });
    const result = await session.send('Reply with exactly: TEMPERATURE_OK');
    check('temperature=0.0 accepted', result != null);
    check('temperature=0.0 has text', Boolean(result?.text),
      `got: ${result?.text?.slice(0, 80)}`);
  } catch (e: any) {
    check('temperature via SessionOptions', false, String(e));
  }

  // --- 2c: continuationEnabled=false ---
  console.log('\n  2c. continuationEnabled=false');
  try {
    const agent = await Agent.create(KIMI_ACL_CONFIG);
    const session = agent.session(cwd, { continuationEnabled: false });
    const result = await session.send('Reply with exactly: CONT_OK');
    check('continuationEnabled=false accepted', result != null);
    check('continuationEnabled=false has text', Boolean(result?.text),
      `got: ${result?.text?.slice(0, 80)}`);
  } catch (e: any) {
    check('continuationEnabled=false', false, String(e));
  }

  // --- 2d: maxContinuationTurns=1 ---
  console.log('\n  2d. maxContinuationTurns=1');
  try {
    const agent = await Agent.create(KIMI_ACL_CONFIG);
    const session = agent.session(cwd, { maxContinuationTurns: 1 });
    const result = await session.send('Reply with exactly: TURNS_OK');
    check('maxContinuationTurns=1 accepted', result != null);
  } catch (e: any) {
    check('maxContinuationTurns=1', false, String(e));
  }

  // --- 2e: combined new options ---
  console.log('\n  2e. Combined new options');
  try {
    const agent = await Agent.create(KIMI_ACL_CONFIG);
    const session = agent.session(cwd, {
      model: 'openai/kimi-k2.5',
      temperature: 0.3,
      continuationEnabled: true,
      maxContinuationTurns: 2,
      maxParseRetries: 3,
      toolTimeoutMs: 30000,
      circuitBreakerThreshold: 5,
    });
    const result = await session.send('Reply with exactly: COMBINED_OK');
    check('combined options accepted', result != null);
    check('combined options has text', Boolean(result?.text),
      `got: ${result?.text?.slice(0, 80)}`);
  } catch (e: any) {
    check('combined options', false, String(e));
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main() {
  if (!apiKey) {
    console.log('\n  SKIP: KIMI_API_KEY not set — skipping live LLM tests');
    skip('live LLM tests', 'KIMI_API_KEY not set');
  } else {
    await runLiveTests(apiKey);
  }

  // ---------------------------------------------------------------------------
  // Summary
  // ---------------------------------------------------------------------------

  section('Summary');

  const total = results.length;
  const passed = results.filter((r) => r.status === 'PASS').length;
  const failed = results.filter((r) => r.status === 'FAIL').length;
  const skipped = results.filter((r) => r.status === 'SKIP').length;

  console.log(`\n  Total: ${total}  Passed: ${passed}  Failed: ${failed}  Skipped: ${skipped}`);

  if (failed > 0) {
    console.log('\n  Failed tests:');
    for (const { name, status, detail } of results) {
      if (status === 'FAIL') {
        console.log(`    ✗ ${name}: ${detail}`);
      }
    }
  }

  console.log();
  process.exit(failed === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error(`Unexpected error: ${e}`);
  process.exit(1);
});
