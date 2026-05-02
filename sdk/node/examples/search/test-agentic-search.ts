/**
 * Agentic Search - Full Feature Test (Kimi K2.5)
 *
 * Tests all agentic_search modes and parameters using the Kimi model:
 *   - FAST mode (default)
 *   - DEEP mode (Monte Carlo sampling)
 *   - FILENAME_ONLY mode
 *   - include glob filter
 *   - context_lines adjustment
 *   - max_results limit
 *
 * Prerequisites:
 *   export KIMI_API_KEY="your-api-key"
 *   export KIMI_BASE_URL="http://your-kimi-endpoint/v1"
 */

import { Agent } from '../../index.js';
import { dirname, resolve } from 'path';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// ── Config ────────────────────────────────────────────────────────────────────

const CONFIG = resolve(__dirname, 'agent_kimi_k2.5.acl');
// Search target: the code crate itself (rich Rust codebase)
const WORKSPACE = resolve(__dirname, '../../core');

// ── Helpers ───────────────────────────────────────────────────────────────────

function section(title: string): void {
  console.log(`\n${'─'.repeat(60)}`);
  console.log(`  ${title}`);
  console.log(`${'─'.repeat(60)}`);
}

async function runTest(
  session: any,
  name: string,
  prompt: string
): Promise<boolean> {
  console.log(`\n▶ ${name}`);
  const t0 = Date.now();
  try {
    const result = await session.send(prompt);
    const elapsed = ((Date.now() - t0) / 1000).toFixed(1);
    console.log(`  [${elapsed}s] ${result.text.slice(0, 300).trim()}`);
    console.log(`  ✅ PASS`);
    return true;
  } catch (e: any) {
    const elapsed = ((Date.now() - t0) / 1000).toFixed(1);
    console.log(`  [${elapsed}s] ❌ FAIL: ${e.message}`);
    return false;
  }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

async function testFastMode(session: any): Promise<boolean> {
  return runTest(
    session,
    'FAST mode — natural language query',
    `Use the agentic_search tool on workspace '${WORKSPACE}' ` +
      'with these exact parameters:\n' +
      '  query: "tool execution context"\n' +
      '  mode: "fast"\n' +
      '  max_results: 5\n' +
      '  context_lines: 2\n' +
      'Show the file names and relevance scores from the result.'
  );
}

async function testDeepMode(session: any): Promise<boolean> {
  return runTest(
    session,
    'DEEP mode — Monte Carlo evidence sampling',
    `Use the agentic_search tool on workspace '${WORKSPACE}' ` +
      'with these exact parameters:\n' +
      '  query: "agent loop LLM turn execution"\n' +
      '  mode: "deep"\n' +
      '  max_results: 3\n' +
      'Show the evidence scores from the result.'
  );
}

async function testFilenameOnly(session: any): Promise<boolean> {
  return runTest(
    session,
    'FILENAME_ONLY mode — quick file discovery',
    `Use the agentic_search tool on workspace '${WORKSPACE}' ` +
      'with these exact parameters:\n' +
      '  query: "builtin"\n' +
      '  mode: "filename_only"\n' +
      '  max_results: 10\n' +
      'List all file paths returned.'
  );
}

async function testIncludeGlob(session: any): Promise<boolean> {
  return runTest(
    session,
    'include glob — restrict to *.rs files',
    `Use the agentic_search tool on workspace '${WORKSPACE}' ` +
      'with these exact parameters:\n' +
      '  query: "permission policy checker"\n' +
      '  include: "*.rs"\n' +
      '  max_results: 5\n' +
      '  context_lines: 1\n' +
      'Show the file names found.'
  );
}

async function testContextLines(session: any): Promise<boolean> {
  return runTest(
    session,
    'context_lines — wide context window',
    `Use the agentic_search tool on workspace '${WORKSPACE}' ` +
      'with these exact parameters:\n' +
      '  query: "session store save load"\n' +
      '  max_results: 2\n' +
      '  context_lines: 5\n' +
      'Show the matching lines with their surrounding context.'
  );
}

async function testMaxResultsLimit(session: any): Promise<boolean> {
  return runTest(
    session,
    'max_results — enforce result cap',
    `Use the agentic_search tool on workspace '${WORKSPACE}' ` +
      'with these exact parameters:\n' +
      '  query: "pub fn"\n' +
      '  max_results: 2\n' +
      'Confirm that no more than 2 files are returned.'
  );
}

async function testNoResults(session: any): Promise<boolean> {
  return runTest(
    session,
    'no results — graceful empty response',
    `Use the agentic_search tool on workspace '${WORKSPACE}' ` +
      'with these exact parameters:\n' +
      '  query: "xyzzy_nonexistent_term_12345"\n' +
      '  mode: "fast"\n' +
      'Confirm that no results were found.'
  );
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  console.log('='.repeat(60));
  console.log('  Agentic Search — Full Feature Test (Kimi K2.5)');
  console.log('='.repeat(60));
  console.log(`  Config:    ${CONFIG}`);
  console.log(`  Workspace: ${WORKSPACE}`);

  // Check environment variables
  if (!process.env.KIMI_API_KEY) {
    console.log('\n❌ Error: KIMI_API_KEY environment variable not set');
    console.log("   export KIMI_API_KEY='your-api-key'");
    process.exit(1);
  }
  if (!process.env.KIMI_BASE_URL) {
    console.log('\n❌ Error: KIMI_BASE_URL environment variable not set');
    console.log("   export KIMI_BASE_URL='http://your-endpoint/v1'");
    process.exit(1);
  }

  const agent = await Agent.create(CONFIG);

  const session = agent.session(WORKSPACE, {
    permissionPolicy: { defaultDecision: 'allow' }, // auto-approve all tool calls
    maxToolRounds: 5,
  });
  console.log('\n  ✓ Session ready\n');

  const tests = [
    testFastMode,
    testDeepMode,
    testFilenameOnly,
    testIncludeGlob,
    testContextLines,
    testMaxResultsLimit,
    testNoResults,
  ];

  section('Running tests');
  const results = await Promise.all(tests.map((t) => t(session)));

  section('Summary');
  const passed = results.filter((r) => r).length;
  const total = results.length;
  const labels = [
    'FAST mode',
    'DEEP mode',
    'FILENAME_ONLY mode',
    'include glob',
    'context_lines',
    'max_results limit',
    'no results',
  ];
  labels.forEach((label, i) => {
    const status = results[i] ? '✅' : '❌';
    console.log(`  ${status}  ${label}`);
  });

  console.log(`\n  ${passed}/${total} tests passed`);
  process.exit(passed === total ? 0 : 1);
}

main().catch((e) => {
  console.error('\n❌ Fatal error:', e);
  process.exit(1);
});
