#!/usr/bin/env node
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, writeFileSync, mkdirSync, realpathSync, rmSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import {
  Agent,
  DefaultSecurityProvider,
  FileMemoryStore,
  FileSessionStore,
  HttpTransport,
  MemorySessionStore,
  StdioTransport,
  UnixSocketTransport,
  WebSocketTransport,
  formatVerificationSummary,
} from '../sdk/node/index.js';

function startFakeOpenAiServer() {
  const requests = [];
  const server = http.createServer((req, res) => {
    let body = '';
    req.setEncoding('utf8');
    req.on('data', chunk => {
      body += chunk;
    });
    req.on('end', () => {
      const parsed = body ? JSON.parse(body) : {};
      requests.push(parsed);

      if (req.url !== '/v1/chat/completions') {
        res.writeHead(404);
        res.end('not found');
        return;
      }

      if (parsed.stream) {
        res.writeHead(200, {
          'content-type': 'text/event-stream',
          'cache-control': 'no-cache',
          connection: 'keep-alive',
        });
        res.write(
          `data: ${JSON.stringify({
            id: 'chatcmpl-docs-stream',
            object: 'chat.completion.chunk',
            model: parsed.model,
            choices: [{ index: 0, delta: { content: 'docs smoke stream ok' }, finish_reason: null }],
          })}\n\n`,
        );
        res.write(
          `data: ${JSON.stringify({
            id: 'chatcmpl-docs-stream',
            object: 'chat.completion.chunk',
            model: parsed.model,
            choices: [{ index: 0, delta: {}, finish_reason: 'stop' }],
            usage: { prompt_tokens: 3, completion_tokens: 2, total_tokens: 5 },
          })}\n\n`,
        );
        res.end('data: [DONE]\n\n');
        return;
      }

      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          id: 'chatcmpl-docs',
          object: 'chat.completion',
          created: Math.floor(Date.now() / 1000),
          model: parsed.model,
          choices: [
            {
              index: 0,
              message: { role: 'assistant', content: `docs smoke response ${requests.length}` },
              finish_reason: 'stop',
            },
          ],
          usage: { prompt_tokens: 7, completion_tokens: 4, total_tokens: 11 },
        }),
      );
    });
  });

  return new Promise(resolve => {
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      resolve({
        baseUrl: `http://127.0.0.1:${port}`,
        requests,
        close: () => new Promise(done => server.close(done)),
      });
    });
  });
}

function makeWorkspace() {
  const root = mkdtempSync(path.join(tmpdir(), 'a3s-doc-contract-'));
  mkdirSync(path.join(root, 'src'));
  mkdirSync(path.join(root, 'tools'));
  mkdirSync(path.join(root, 'skills'));
  mkdirSync(path.join(root, 'agents'));
  writeFileSync(path.join(root, 'README.md'), '# Docs Smoke\n\nplanningMode appears here.\n');
  writeFileSync(
    path.join(root, 'AGENTS.md'),
    '# Project Instructions\n\nAlways mention docs-contract-agents-md-token when asked for project instructions.\n',
  );
  writeFileSync(path.join(root, 'src', 'main.rs'), 'fn main() { println!("PermissionPolicy"); }\n');
  writeFileSync(
    path.join(root, 'agents', 'docs-dynamic.yaml'),
    `name: docs-dynamic
description: Dynamically registered docs smoke agent
max_steps: 3
permissions:
  allow:
    - read
    - grep
  deny:
    - write
`,
  );
  writeFileSync(
    path.join(root, 'skills', 'release-review.md'),
    `---
name: release-review
description: Review release blockers and verification evidence
allowed-tools: "read(*), grep(*)"
tags: ["release", "docs"]
---

Check package metadata, changelog, release scripts, and CI status.
Return blockers first.
`,
  );
  writeFileSync(
    path.join(root, 'tools', 'mcp_echo_server.mjs'),
    `#!/usr/bin/env node
import readline from 'node:readline';

const secret = process.argv[2] || 'docs-secret';
const tools = [
  {
    name: 'echo',
    description: 'Echo the input message',
    inputSchema: {
      type: 'object',
      properties: { message: { type: 'string' } },
      required: ['message'],
    },
  },
  {
    name: 'get_secret',
    description: 'Return the server-side secret',
    inputSchema: { type: 'object', properties: {} },
  },
];

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
function send(obj) { process.stdout.write(JSON.stringify(obj) + '\\n'); }
rl.on('line', line => {
  const msg = JSON.parse(line);
  const { method, id, params } = msg;
  if (method === 'initialize') {
    send({
      jsonrpc: '2.0',
      id,
      result: {
        protocolVersion: '2024-11-05',
        capabilities: { tools: {} },
        serverInfo: { name: 'docs-echo', version: '0.1.0' },
      },
    });
  } else if (method?.startsWith('notifications/')) {
    return;
  } else if (method === 'tools/list') {
    send({ jsonrpc: '2.0', id, result: { tools } });
  } else if (method === 'tools/call') {
    const name = params?.name;
    const args = params?.arguments ?? {};
    if (name === 'echo') {
      send({ jsonrpc: '2.0', id, result: { content: [{ type: 'text', text: args.message ?? '' }] } });
    } else if (name === 'get_secret') {
      send({ jsonrpc: '2.0', id, result: { content: [{ type: 'text', text: secret }] } });
    } else {
      send({ jsonrpc: '2.0', id, error: { code: -32601, message: 'unknown tool' } });
    }
  } else if (id !== undefined) {
    send({ jsonrpc: '2.0', id, error: { code: -32601, message: 'method not found' } });
  }
});
`,
  );
  execFileSync('git', ['init'], { cwd: root, stdio: 'ignore' });
  execFileSync('git', ['config', 'user.email', 'docs@example.test'], { cwd: root });
  execFileSync('git', ['config', 'user.name', 'Docs Smoke'], { cwd: root });
  execFileSync('git', ['add', '.'], { cwd: root });
  execFileSync('git', ['commit', '-m', 'init'], { cwd: root, stdio: 'ignore' });
  return root;
}

async function collectStreamText(stream) {
  let text = '';
  while (true) {
    const { value, done } = await stream.next();
    if (done) break;
    if (value?.text) text += value.text;
  }
  return text;
}

const fake = await startFakeOpenAiServer();
const workspace = makeWorkspace();
const stores = mkdtempSync(path.join(tmpdir(), 'a3s-doc-stores-'));
let completed = false;

try {
  const acl = `
default_model = "openai/docs-fake"

providers "openai" {
  apiKey = "test-key"
  baseUrl = "${fake.baseUrl}"

  models "docs-fake" {
    name = "Docs Fake"
    tool_call = true
  }

  models "docs-alt" {
    name = "Docs Alt"
    tool_call = true
  }
}

storage_backend = "memory"
`;

  const agent = await Agent.create(acl);
  assert.equal(typeof agent.refreshMcpTools, 'function');

  const aliasAgent = await Agent.create(`
default_model = "openai/docs-fake"

providers "openai" {
  api_key = "alias-key"
  base_url = "${fake.baseUrl}"

  models "docs-fake" {
    name = "Docs Fake"
    tool_call = true
  }
}

storage_backend = "memory"
`);
  const aliasSession = aliasAgent.session(workspace, { planningMode: 'disabled' });
  assert.match((await aliasSession.send('Provider alias smoke.')).text, /docs smoke stream ok/);
  aliasSession.close();

  const aclSessionDir = path.join(stores, 'acl-storage');
  const storageAgent = await Agent.create(`
default_model = "openai/docs-fake"

providers "openai" {
  apiKey = "storage-key"
  baseUrl = "${fake.baseUrl}"

  models "docs-fake" {
    name = "Docs Fake"
    tool_call = true
  }
}

storage_backend = "file"
sessions_dir = "${aclSessionDir}"
`);
  const storageSession = storageAgent.session(workspace, {
    planningMode: 'disabled',
    sessionId: 'acl-storage-contract',
    autoSave: true,
  });
  assert.equal(typeof storageSession.sessionId, 'string');
  await storageSession.send('ACL sessions_dir persistence smoke.');
  storageSession.close();
  const restoredStorageSession = storageAgent.resumeSession('acl-storage-contract', {
    sessionStore: new FileSessionStore(aclSessionDir),
    planningMode: 'disabled',
  });
  assert.ok(restoredStorageSession.history().length >= 1);
  restoredStorageSession.close();

  const namedAgentSession = agent.sessionForAgent(workspace, 'explore', [], {
    builtinSkills: true,
    planningMode: 'disabled',
  });
  assert.equal(typeof namedAgentSession.sessionId, 'string');
  namedAgentSession.close();

  const policySession = agent.session(workspace, {
    permissionPolicy: {
      deny: ['write(**/.env*)', 'bash(rm -rf*)'],
      ask: ['bash(git push*)', 'bash(npm publish*)'],
      allow: ['read(*)', 'grep(*)', 'glob(*)', 'bash(npm run build*)'],
      defaultDecision: 'ask',
      enabled: true,
    },
  });
  assert.equal(typeof policySession.sessionId, 'string');
  policySession.close();

  const promptSlotSession = agent.session(workspace, {
    role: 'release-readiness reviewer',
    guidelines: 'Find blockers before improvements. Require command evidence for done claims.',
    responseStyle: 'concise, findings first',
    goalTracking: true,
  });
  assert.equal(typeof promptSlotSession.sessionId, 'string');
  promptSlotSession.close();

  const session = agent.session(workspace, {
    builtinSkills: true,
    planningMode: 'disabled',
    memoryStore: new FileMemoryStore(path.join(stores, 'memory')),
    sessionStore: new FileSessionStore(path.join(stores, 'sessions')),
    sessionId: 'docs-contract',
    autoSave: true,
    securityProvider: new DefaultSecurityProvider(),
    skillDirs: [path.join(workspace, 'skills')],
    inlineSkills: [
      {
        name: 'strict-release-review',
        kind: 'instruction',
        content: 'Always separate blockers from nice-to-have improvements.',
      },
    ],
    maxToolRounds: 24,
    maxParseRetries: 3,
    toolTimeoutMs: 120000,
    circuitBreakerThreshold: 4,
    autoCompact: true,
    autoCompactThreshold: 0.75,
    continuationEnabled: true,
    maxContinuationTurns: 3,
    maxExecutionTimeMs: 300000,
    confirmationPolicy: {
      enabled: true,
      defaultTimeoutMs: 60000,
      timeoutAction: 'reject',
    },
  });

  assert.equal(session.hasMemory, true);
  assert.equal(session.sessionId, 'docs-contract');
  assert.equal(session.workspace, realpathSync(workspace));
  assert.equal(session.initWarning, null);
  assert.equal(session.cancel(), false);
  assert.equal(Array.isArray(session.history()), true);

  const modelOverrideSession = agent.session(workspace, {
    model: 'openai/docs-alt',
    planningMode: 'disabled',
  });
  assert.match((await modelOverrideSession.send('Model override smoke.')).text, /docs smoke stream ok/);
  assert.ok(fake.requests.some(request => request.model === 'docs-alt'));
  modelOverrideSession.close();
  const initialToolNames = session.toolNames();
  for (const name of [
    'read',
    'write',
    'edit',
    'patch',
    'grep',
    'glob',
    'ls',
    'bash',
    'task',
    'parallel_task',
    'search_skills',
    'Skill',
    'program',
    'git',
    'batch',
    'web_fetch',
    'web_search',
  ]) {
    assert.ok(initialToolNames.includes(name), `toolNames() should include ${name}`);
  }
  assert.ok(session.toolDefinitions().some(tool => tool.name === 'program'));
  const fileSkillSearch = await session.tool('search_skills', { query: 'release blockers', limit: 5 });
  assert.equal(fileSkillSearch.exitCode, 0);
  assert.match(fileSkillSearch.output, /release-review/);
  const inlineSkillSearch = await session.tool('search_skills', { query: 'strict release review', limit: 5 });
  assert.equal(inlineSkillSearch.exitCode, 0);
  assert.match(inlineSkillSearch.output, /strict-release-review/);

  const directRead = await session.readFile('README.md');
  assert.match(directRead, /planningMode/);
  assert.ok((await session.glob('src/*.rs')).some(file => file.endsWith('src/main.rs')));
  assert.match(await session.grep('PermissionPolicy'), /src\/main\.rs/);
  assert.match(await session.bash('printf docs-bash'), /docs-bash/);
  assert.equal((await session.tool('read', { file_path: 'README.md' })).exitCode, 0);
  assert.equal((await session.git('status')).exitCode, 0);
  assert.equal((await session.git('diff')).exitCode, 0);
  assert.equal((await session.git('log', undefined, undefined, undefined, undefined, undefined, undefined, 5)).exitCode, 0);
  assert.equal(session.registerAgentDir(path.join(workspace, 'agents')), 1);

  session.registerHook(
    'docs-block-bash',
    'pre_tool_use',
    { tool: 'bash', commandPattern: 'docs-hook-blocked' },
    { priority: 1, timeoutMs: 1000 },
    () => ({ action: 'continue' }),
  );
  assert.equal(session.hookCount(), 1);
  assert.equal(session.unregisterHook('docs-block-bash'), true);

  session.registerCommand('docs_status', 'Return docs command status', (args, ctx) => {
    return `status args=${args}; session=${ctx.sessionId}; workspace=${ctx.workspace}`;
  });
  assert.ok(session.listCommands().some(command => command.name === 'docs_status'));
  const commandResult = await session.send('/docs_status smoke');
  assert.match(commandResult.text, /status args=smoke/);

  const program = await session.program({
    source: `
      export default async function run(ctx, inputs) {
        const readText = await ctx.readFile("README.md");
        const readResult = await ctx.read("README.md");
        const hits = await ctx.grep(inputs.q, { glob: "*.md" });
        const globText = await ctx.glob("src/*.rs");
        const lsText = await ctx.ls(".");
        const bashText = await ctx.bash("printf ptc-bash");
        const gitStatus = await ctx.git({ command: "status" });
        const explicitTool = await ctx.tool("grep", { pattern: inputs.q, glob: "*.md" });
        return {
          summary: "ok",
          hasReadText: readText.includes(inputs.q),
          readExitCode: readResult.exitCode,
          hasHits: hits.includes(inputs.q),
          hasGlob: globText.includes("src/main.rs"),
          hasLs: lsText.includes("README.md"),
          bashText,
          gitOk: gitStatus.exitCode === 0,
          explicitToolOk: explicitTool.exitCode === 0,
        };
      }
    `,
    inputs: { q: 'planningMode' },
    allowedTools: ['read', 'grep', 'glob', 'ls', 'bash', 'git'],
    limits: { timeoutMs: 30000, maxToolCalls: 12, maxOutputBytes: 65536 },
  });
  assert.equal(program.exitCode, 0);
  const programMetadata = JSON.parse(program.metadataJson);
  assert.equal(programMetadata.script_result.hasReadText, true);
  assert.equal(programMetadata.script_result.readExitCode, 0);
  assert.equal(programMetadata.script_result.hasHits, true);
  assert.equal(programMetadata.script_result.hasGlob, true);
  assert.equal(programMetadata.script_result.hasLs, true);
  assert.equal(programMetadata.script_result.bashText.trim(), 'ptc-bash');
  assert.equal(programMetadata.script_result.gitOk, true);
  assert.equal(programMetadata.script_result.explicitToolOk, true);
  assert.deepEqual(
    programMetadata.program.tool_calls.map(call => call.tool_name),
    ['read', 'read', 'grep', 'glob', 'ls', 'bash', 'git', 'grep'],
  );

  const verification = await session.verifyCommands('docs smoke', [
    { id: 'echo', kind: 'command', description: 'echo works', command: 'printf verify', required: true },
  ]);
  assert.equal(verification.subject, 'docs smoke');
  assert.equal(Array.isArray(session.verificationPresets()), true);
  assert.equal(Array.isArray(session.verificationReports()), true);
  assert.equal(typeof session.verificationSummaryText(), 'string');
  assert.equal(typeof formatVerificationSummary(session.verificationSummary()), 'string');

  await session.rememberSuccess('docs memory success', ['grep'], 'remembered');
  await session.rememberFailure('docs memory failure', 'expected failure', ['bash']);
  assert.ok((await session.memoryRecent(10)).length >= 2);
  assert.ok((await session.recallSimilar('docs memory', 5)).length >= 1);
  assert.ok(Array.isArray(await session.recallByTags(['grep'], 10)));
  assert.equal(typeof session.recallRecent, 'undefined');

  const result = await session.send('Return a short docs smoke response.');
  assert.match(result.text, /docs smoke stream ok/);
  assert.equal(result.totalTokens, 5);
  assert.ok(fake.requests.some(request => JSON.stringify(request).includes('docs-contract-agents-md-token')));

  const streamText = await collectStreamText(await session.stream('Stream one sentence.'));
  assert.match(streamText, /docs smoke stream ok/);

  const sideHistory = session.history();
  const sideResult = await session.send('Answer this isolated side question.', sideHistory);
  assert.ok(sideResult.text.length > 0);
  assert.deepEqual(session.history(), sideHistory);

  const delegated = await session.task({
    agent: 'general',
    description: 'docs delegated smoke',
    prompt: 'Return a short docs delegated response.',
    maxSteps: 1,
  });
  assert.equal(delegated.name, 'task');
  assert.equal(delegated.exitCode, 0);

  const parallel = await session.tasks([
    {
      agent: 'general',
      description: 'docs parallel smoke one',
      prompt: 'Return one short response.',
      maxSteps: 1,
    },
    {
      agent: 'general',
      description: 'docs parallel smoke two',
      prompt: 'Return another short response.',
      maxSteps: 1,
    },
  ]);
  assert.equal(parallel.name, 'parallel_task');
  assert.equal(parallel.exitCode, 0);

  assert.ok((await session.runs()).length >= 2);
  assert.equal(Array.isArray(session.traceEvents()), true);
  const latest = (await session.runs()).at(-1);
  assert.ok(latest.id);
  assert.ok(await session.runSnapshot(latest.id));
  assert.equal(Array.isArray(await session.runEvents(latest.id)), true);
  const current = await session.currentRun();
  if (current !== null) {
    assert.ok(current.id);
    assert.ok(['running', 'completed', 'failed', 'cancelled'].includes(current.status));
  }
  assert.equal(await session.cancelRun('not-active'), false);

  await session.save();
  const resumed = agent.resumeSession('docs-contract', {
    sessionStore: new FileSessionStore(path.join(stores, 'sessions')),
  });
  assert.equal(resumed.history().length >= 1, true);

  const queued = agent.session(workspace, {
    queueConfig: { enableDlq: true, enableMetrics: true },
  });
  assert.equal(queued.hasQueue(), true);
  await queued.setLaneHandler('execute', { mode: 'external', timeoutMs: 1000 });
  assert.equal(Array.isArray(await queued.pendingExternalTasks()), true);
  assert.equal(await queued.completeExternalTask('missing', { success: true, result: { ok: true } }), false);
  assert.equal(typeof (await queued.queueStats()).totalPending, 'number');
  assert.equal(await queued.queueMetrics() !== null, true);
  assert.equal(Array.isArray(await queued.deadLetters()), true);

  const mcpSecret = 'docs-mcp-secret';
  const mcpCount = await session.addMcp({
    name: 'echo',
    transport: {
      type: 'stdio',
      command: process.execPath,
      args: [path.join(workspace, 'tools', 'mcp_echo_server.mjs'), mcpSecret],
    },
  });
  assert.equal(mcpCount >= 2, true);
  assert.ok(session.toolNames().includes('mcp__echo__echo'));
  const mcps = await session.mcps();
  assert.equal(mcps.find(server => server.name === 'echo')?.connected, true);
  const mcpEcho = await session.tool('mcp__echo__echo', { message: 'docs mcp ok' });
  assert.equal(mcpEcho.exitCode, 0);
  assert.match(mcpEcho.output, /docs mcp ok/);
  const mcpSecretResult = await session.tool('mcp__echo__get_secret', {});
  assert.match(mcpSecretResult.output, new RegExp(mcpSecret));
  await session.removeMcp('echo');
  assert.equal(session.toolNames().some(name => name.startsWith('mcp__echo__')), false);

  assert.equal(new MemorySessionStore().backend, 'memory');
  assert.equal(new HttpTransport('http://localhost:8080/ahp', 'token').kind, 'http');
  assert.equal(new WebSocketTransport('ws://localhost:8080/ahp', 'token').kind, 'websocket');
  assert.equal(new StdioTransport('node', ['server.mjs']).kind, 'stdio');
  assert.equal(new UnixSocketTransport('/tmp/a3s.sock').kind, 'unix_socket');

  session.close();
  resumed.close();
  queued.close();

  console.log(
    JSON.stringify(
      {
        status: 'ok',
        fakeRequests: fake.requests.length,
        workspace,
        stores,
      },
      null,
      2,
    ),
  );
  completed = true;
} finally {
  await fake.close();
  rmSync(workspace, { recursive: true, force: true });
  rmSync(stores, { recursive: true, force: true });
  if (completed) process.exit(0);
}
