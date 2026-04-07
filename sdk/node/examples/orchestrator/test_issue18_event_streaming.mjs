/**
 * Live verification for the sub-agent event-streaming fix.
 *
 * This validates the issue #18 behavior using a real Kimi-backed agent:
 *   1. Subscribe late via `SubAgentHandle.events()`
 *   2. Confirm early events are replayed
 *   3. Confirm `tool_execution_started.args` is populated
 *   4. Confirm `tool_execution_completed.durationMs` is > 0
 *   5. Confirm `text_delta` events are forwarded
 *
 * It reads the Kimi endpoint and API key from the repo's `.a3s/config.hcl` and
 * injects them into the SDK example config via `KIMI_API_KEY` / `KIMI_BASE_URL`.
 */

import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';
import { Agent, Orchestrator } from '../index.js';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, '..', '..', '..', '..', '..');
const appConfig = path.join(repoRoot, '.a3s', 'config.hcl');
const sdkConfig = path.join(__dirname, 'agent_kimi_k2.5.hcl');

function loadKimiEnvFromRepoConfig() {
  const raw = fs.readFileSync(appConfig, 'utf8');
  const apiKey = raw.match(/"id"\s*=\s*"kimi-k2\.5"[\s\S]*?"apiKey"\s*=\s*"([^"]+)"/);
  const baseUrl = raw.match(/"id"\s*=\s*"kimi-k2\.5"[\s\S]*?"baseUrl"\s*=\s*"([^"]+)"/);
  if (!apiKey || !baseUrl) {
    throw new Error(`Failed to extract kimi-k2.5 config from ${appConfig}`);
  }
  process.env.KIMI_API_KEY = apiKey[1];
  process.env.KIMI_BASE_URL = baseUrl[1];
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function main() {
  console.log('\n=== Node SDK live sub-agent event-stream test ===\n');
  loadKimiEnvFromRepoConfig();

  const agent = await Agent.create(sdkConfig);
  const orchestrator = Orchestrator.create(agent);
  const handle = orchestrator.spawnSubagent({
    agentType: 'general',
    prompt: "Use bash to run: printf 'hello-from-node-sdk'. Then briefly explain the result.",
    description: 'issue18-node-live-test',
    permissive: true,
    maxSteps: 5,
  });

  await sleep(2000);
  const events = handle.events();

  const counts = {};
  const textDeltas = [];
  const toolStarts = [];
  const toolEnds = [];

  const startedAt = Date.now();
  while (Date.now() - startedAt < 60000) {
    const event = await events.recv(2000);
    if (!event) continue;

    const eventType = event.event_type || 'unknown';
    counts[eventType] = (counts[eventType] || 0) + 1;

    if (eventType === 'sub_agent_internal_event' && event.type === 'text_delta') {
      textDeltas.push(event.text || '');
    } else if (eventType === 'tool_execution_started') {
      toolStarts.push({ toolName: event.tool_name, args: event.args });
    } else if (eventType === 'tool_execution_completed') {
      toolEnds.push({
        toolName: event.tool_name,
        durationMs: event.duration_ms,
        result: (event.result || '').slice(0, 120),
      });
    } else if (eventType === 'sub_agent_completed') {
      break;
    }
  }

  const result = handle.wait();
  const summary = {
    counts,
    toolStarts,
    toolEnds,
    textDeltaChars: textDeltas.join('').length,
    resultPreview: result.slice(0, 200),
  };
  console.log(JSON.stringify(summary, null, 2));

  if (!(counts.sub_agent_started >= 1)) throw new Error('missing sub_agent_started replay');
  if (!(counts.tool_execution_started >= 1)) throw new Error('missing tool_execution_started');
  if (!(counts.tool_execution_completed >= 1)) throw new Error('missing tool_execution_completed');
  if (!(counts.sub_agent_internal_event >= 1)) throw new Error('missing sub_agent_internal_event');
  if (!toolStarts.some((item) => item.args && Object.keys(item.args).length > 0)) {
    throw new Error('tool args were empty');
  }
  if (!toolEnds.some((item) => (item.durationMs || 0) > 0)) {
    throw new Error('tool durationMs was not > 0');
  }
  if (!(textDeltas.join('').length > 0)) throw new Error('missing text_delta events');

  console.log('\nPASS\n');
}

main().catch((error) => {
  console.error(`\nFAIL: ${error.message}\n`);
  process.exitCode = 1;
});
