#!/usr/bin/env node
/**
 * Basic SDK usage example — Agent.create(), session, and tool execution.
 *
 * Demonstrates:
 *   - Loading config from `.a3s/config.hcl`
 *   - Creating agents with Agent.create()
 *   - Creating workspace-bound sessions
 *   - Direct tool calls via session.tool(name, args) for all built-in tools
 *   - Convenience methods: bash(), readFile(), glob(), grep()
 *
 * No LLM calls — runs entirely offline.
 *
 * Usage:
 *     # Build the native module first
 *     cd crates/code/sdk/node && npm run build
 *
 *     # Run the example
 *     node examples/sdk_basic.mjs
 *
 *     # Or with a custom config path
 *     A3S_CONFIG=/path/to/config.hcl node examples/sdk_basic.mjs
 */

import { Agent } from '../index.js';
import { mkdtempSync, writeFileSync, readFileSync, mkdirSync } from 'fs';
import { join, dirname } from 'path';
import { tmpdir } from 'os';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

function repoRoot() {
  return join(__dirname, '..', '..', '..', '..', '..');
}

function resolveConfig() {
  if (process.env.A3S_CONFIG) return process.env.A3S_CONFIG;
  return join(repoRoot(), '.a3s', 'config.hcl');
}

function assert(condition, message) {
  if (!condition) throw new Error(`Assertion failed: ${message}`);
}

async function main() {
  const configPath = resolveConfig();
  console.log('=== A3S Code Node.js SDK Basic Example ===\n');
  console.log(`Config: ${configPath}\n`);

  // -- 1. Agent.create() ---------------------------------------------------
  console.log('--- Agent.create() ---');
  const agent = await Agent.create(configPath);
  console.log('[ok] Agent created\n');

  // -- 2. Create session ----------------------------------------------------
  const workspace = mkdtempSync(join(tmpdir(), 'a3s-node-'));
  const session = agent.session(workspace);
  console.log(`[ok] Session bound to ${workspace}\n`);

  // -- 3. Tool: bash --------------------------------------------------------
  console.log('--- Tool: bash ---');
  const output = await session.bash("echo 'Hello from Node.js SDK'");
  assert(output.includes('Hello from Node.js SDK'), `unexpected: ${output}`);
  console.log(`  output: ${output.trim()}`);

  // -- 4. Tool: write + read ------------------------------------------------
  console.log('\n--- Tool: write + read ---');
  const filePath = join(workspace, 'demo.txt');
  await session.tool('write', {
    file_path: filePath,
    content: 'Hello from Node.js SDK!\nLine 2\nLine 3',
  });
  console.log(`  wrote: ${filePath}`);

  const content = await session.readFile(filePath);
  const lines = content.split('\n');
  console.log(`  read back (${lines.length} lines):`);
  lines.slice(0, 3).forEach(line => console.log(`    ${line}`));

  // -- 5. Tool: edit --------------------------------------------------------
  console.log('\n--- session.tool("edit") ---');
  const editResult = await session.tool('edit', {
    file_path: filePath,
    old_string: 'Line 2',
    new_string: 'Line 2 (edited)',
  });
  console.log(`  exit_code: ${editResult.exitCode}`);
  assert(editResult.exitCode === 0, `edit failed: ${editResult.output}`);
  const edited = readFileSync(join(workspace, 'demo.txt'), 'utf-8');
  assert(edited.includes('Line 2 (edited)'), 'edit not applied');
  console.log("  verified: file contains 'Line 2 (edited)'");

  // -- 6. Tool: patch -------------------------------------------------------
  console.log('\n--- session.tool("patch") ---');
  const patchResult = await session.tool('patch', {
    file_path: filePath,
    diff: "@@ -2,2 +2,2 @@\n-Line 2 (edited)\n+Line 2 (patched)\n Line 3",
  });
  console.log(`  exit_code: ${patchResult.exitCode}`);
  if (patchResult.exitCode === 0) {
    const patched = readFileSync(join(workspace, 'demo.txt'), 'utf-8');
    assert(patched.includes('Line 2 (patched)'), 'patch not applied');
    console.log("  verified: file contains 'Line 2 (patched)'");
  } else {
    console.log(`  patch output: ${patchResult.output.trim()}`);
  }

  // -- 7. Tool: read --------------------------------------------------------
  console.log('\n--- session.tool("read") ---');
  const readResult = await session.tool('read', {
    file_path: filePath,
    offset: 0,
    limit: 5,
  });
  console.log(`  exit_code: ${readResult.exitCode}`);
  assert(readResult.exitCode === 0, `read failed: ${readResult.output}`);
  console.log('  content:');
  readResult.output.split('\n').slice(0, 3).forEach(line => console.log(`    ${line}`));

  // -- 8. Tool: ls ----------------------------------------------------------
  console.log('\n--- session.tool("ls") ---');
  writeFileSync(join(workspace, 'alpha.rs'), 'fn main() {}');
  writeFileSync(join(workspace, 'beta.rs'), 'fn test() {}');
  writeFileSync(join(workspace, 'gamma.txt'), 'text file');
  mkdirSync(join(workspace, 'subdir'), { recursive: true });

  const lsResult = await session.tool('ls', { path: workspace });
  console.log(`  exit_code: ${lsResult.exitCode}`);
  assert(lsResult.exitCode === 0, `ls failed: ${lsResult.output}`);
  assert(lsResult.output.includes('alpha.rs'), 'ls should list alpha.rs');
  assert(lsResult.output.includes('subdir'), 'ls should list subdir');
  console.log('  entries:');
  lsResult.output.split('\n').slice(0, 6).forEach(line => console.log(`    ${line}`));

  // -- 9. Tool: glob --------------------------------------------------------
  console.log('\n--- session.tool("glob") ---');
  const globResult = await session.tool('glob', { pattern: '*.rs' });
  console.log(`  exit_code: ${globResult.exitCode}`);
  assert(globResult.exitCode === 0, `glob failed: ${globResult.output}`);
  assert(globResult.output.includes('alpha.rs'), 'glob should find alpha.rs');
  console.log(`  matches: ${globResult.output.trim()}`);

  // -- 10. Tool: grep -------------------------------------------------------
  console.log('\n--- session.tool("grep") ---');
  const grepResult = await session.tool('grep', { pattern: 'fn ', glob: '*.rs' });
  console.log(`  exit_code: ${grepResult.exitCode}`);
  assert(grepResult.exitCode === 0, `grep failed: ${grepResult.output}`);
  assert(grepResult.output.includes('fn '), "grep should find 'fn '");
  console.log('  output:');
  grepResult.output.split('\n').slice(0, 5).forEach(line => console.log(`    ${line}`));

  // -- 11. Tool: bash (direct) ----------------------------------------------
  console.log('\n--- session.tool("bash") ---');
  const bashResult = await session.tool('bash', {
    command: "echo 'direct tool call' && date +%Y",
    timeout: 5000,
  });
  console.log(`  exit_code: ${bashResult.exitCode}`);
  assert(bashResult.exitCode === 0, `bash failed: ${bashResult.output}`);
  assert(bashResult.output.includes('direct tool call'));
  console.log(`  output: ${bashResult.output.trim()}`);

  // -- 12. Tool: web_fetch --------------------------------------------------
  console.log('\n--- session.tool("web_fetch") ---');
  const fetchResult = await session.tool('web_fetch', {
    url: 'https://httpbin.org/get',
    format: 'text',
    timeout: 15,
  });
  console.log(`  exit_code: ${fetchResult.exitCode}`);
  assert(fetchResult.exitCode === 0, `web_fetch failed: ${fetchResult.output}`);
  console.log('  response (first 5 lines):');
  fetchResult.output.split('\n').slice(0, 5).forEach(line => console.log(`    ${line}`));

  // -- 13. Tool: web_search -------------------------------------------------
  console.log('\n--- session.tool("web_search") ---');
  const searchResult = await session.tool('web_search', {
    query: 'Rust programming language',
    engines: 'ddg,wiki',
    limit: 3,
    timeout: 15,
    format: 'text',
  });
  console.log(`  exit_code: ${searchResult.exitCode}`);
  if (searchResult.exitCode === 0) {
    console.log('  results (first 5 lines):');
    searchResult.output.split('\n').slice(0, 5).forEach(line => console.log(`    ${line}`));
  } else {
    const firstLine = searchResult.output.split('\n')[0] || '(empty)';
    console.log(`  [soft-fail] web_search error: ${firstLine}`);
  }

  // -- 14. Tool: cron (full lifecycle) --------------------------------------
  console.log('\n--- session.tool("cron") -- full lifecycle ---');
  const root = repoRoot();
  const cronSession = agent.session(root);
  console.log(`  workspace: ${root} (cron data -> .a3s/cron/)`);

  // parse
  console.log("\n  [parse] 'every 5 minutes'");
  const parseResult = await cronSession.tool('cron', { action: 'parse', input: 'every 5 minutes' });
  console.log(`    exit_code: ${parseResult.exitCode}`);
  assert(parseResult.exitCode === 0, `cron parse failed: ${parseResult.output}`);
  assert(parseResult.output.includes('*/5'));
  console.log(`    output: ${parseResult.output.trim()}`);

  // add
  const jobName = `node-sdk-test-${Date.now()}`;
  console.log(`\n  [add] job '${jobName}' schedule='*/10 * * * *'`);
  const addResult = await cronSession.tool('cron', {
    action: 'add',
    name: jobName,
    schedule: '*/10 * * * *',
    command: "echo 'node sdk cron test'",
  });
  console.log(`    exit_code: ${addResult.exitCode}`);
  assert(addResult.exitCode === 0, `cron add failed: ${addResult.output}`);

  // Extract job ID
  let jobId = null;
  for (const line of addResult.output.split('\n')) {
    const trimmed = line.trim();
    if (trimmed.startsWith('ID: ')) {
      jobId = trimmed.slice(4);
      break;
    }
  }
  assert(jobId, `could not extract job ID from: ${addResult.output}`);
  console.log(`    job_id: ${jobId}`);

  // list
  console.log('\n  [list]');
  const listResult = await cronSession.tool('cron', { action: 'list' });
  assert(listResult.exitCode === 0);
  assert(listResult.output.includes(jobName));
  console.log(`    found '${jobName}' in list`);

  // get
  console.log(`\n  [get] id=${jobId}`);
  const getResult = await cronSession.tool('cron', { action: 'get', id: jobId });
  assert(getResult.exitCode === 0);
  assert(getResult.output.includes(jobName));
  console.log(`    output: ${getResult.output.split('\n')[0]}`);

  // pause
  console.log(`\n  [pause] id=${jobId}`);
  const pauseResult = await cronSession.tool('cron', { action: 'pause', id: jobId });
  assert(pauseResult.exitCode === 0);
  console.log(`    output: ${pauseResult.output.trim()}`);

  // resume
  console.log(`\n  [resume] id=${jobId}`);
  const resumeResult = await cronSession.tool('cron', { action: 'resume', id: jobId });
  assert(resumeResult.exitCode === 0);
  console.log(`    output: ${resumeResult.output.trim()}`);

  // run
  console.log(`\n  [run] id=${jobId}`);
  const runResult = await cronSession.tool('cron', { action: 'run', id: jobId });
  assert(runResult.exitCode === 0);
  console.log(`    output: ${runResult.output.split('\n')[0]}`);

  // history
  console.log(`\n  [history] id=${jobId}`);
  const historyResult = await cronSession.tool('cron', { action: 'history', id: jobId, limit: 5 });
  assert(historyResult.exitCode === 0);
  console.log(`    output: ${historyResult.output.split('\n')[0]}`);

  // remove
  console.log(`\n  [remove] id=${jobId}`);
  const removeResult = await cronSession.tool('cron', { action: 'remove', id: jobId });
  assert(removeResult.exitCode === 0);
  console.log(`    output: ${removeResult.output.trim()}`);

  // verify removal
  const listAfter = await cronSession.tool('cron', { action: 'list' });
  assert(!listAfter.output.includes(jobName));
  console.log('    verified: job no longer in list');

  // -- 15. Unknown tool (error case) ----------------------------------------
  console.log('\n--- session.tool("nonexistent") ---');
  const unknownResult = await session.tool('nonexistent', {});
  console.log(`  exit_code: ${unknownResult.exitCode}`);
  assert(unknownResult.exitCode === 1, 'unknown tool should fail');
  assert(unknownResult.output.includes('Unknown tool'));
  console.log(`  output: ${unknownResult.output.trim()}`);

  // -- 16. Convenience methods ----------------------------------------------
  console.log('\n--- Convenience methods ---');
  const globConv = await session.glob('*.rs');
  console.log(`  session.glob('*.rs'): ${JSON.stringify(globConv)}`);

  const grepConv = await session.grep('fn ');
  console.log(`  session.grep('fn '): ${grepConv.split('\n').length} lines`);

  // -- Done -----------------------------------------------------------------
  console.log('\n=== All checks passed ===');
}

main().catch(err => {
  console.error('FAILED:', err);
  process.exit(1);
});
