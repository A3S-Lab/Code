#!/usr/bin/env npx tsx
/**
 * Minimal streaming HITL confirmation loop.
 *
 * Usage:
 *   A3S_CONFIG_FILE=./agent.acl npx tsx streaming/hitl_confirmation_loop.ts
 *   A3S_CONFIG_FILE=./agent.acl A3S_CODE_HITL_AUTO_APPROVE=1 npx tsx streaming/hitl_confirmation_loop.ts
 */

import { createInterface } from 'node:readline/promises';
import { stdin as input, stdout as output } from 'node:process';
import { Agent } from '../../index.js';
import type { AgentEvent, PendingConfirmation, Session, SessionOptions } from '@a3s-lab/code';

type ConfirmationEventData = {
  args?: unknown;
  timeout_ms?: number;
};

function eventData(event: AgentEvent): ConfirmationEventData {
  if (!event.data) return {};
  try {
    return JSON.parse(event.data) as ConfirmationEventData;
  } catch {
    return {};
  }
}

async function pendingByToolId(session: Session, toolId?: string | null): Promise<PendingConfirmation | null> {
  const pending = await session.pendingConfirmations();
  return pending.find(item => item.toolId === toolId) ?? pending[0] ?? null;
}

async function confirmFromCli(
  session: Session,
  event: AgentEvent,
  ask: (question: string) => Promise<string>,
): Promise<void> {
  const pending = await pendingByToolId(session, event.toolId);
  const data = eventData(event);
  const toolId = pending?.toolId ?? event.toolId;

  if (!toolId) {
    console.warn('[hitl] confirmation_required event has no tool id; skipping');
    return;
  }

  const toolName = pending?.toolName ?? event.toolName ?? 'unknown';
  const args = pending?.args ?? data.args ?? {};
  const remainingMs = pending?.remainingMs ?? data.timeout_ms;

  console.log('\n[hitl] Tool requires confirmation');
  console.log(`tool: ${toolName}`);
  console.log(`toolId: ${toolId}`);
  if (remainingMs !== undefined) console.log(`remainingMs: ${remainingMs}`);
  console.log(JSON.stringify(args, null, 2));

  const autoApprove = process.env.A3S_CODE_HITL_AUTO_APPROVE === '1';
  const answer = autoApprove ? 'y' : await ask('Approve this tool call? [y/N] ');
  const approved = answer.trim().toLowerCase().startsWith('y');

  const completed = await session.confirmToolUse(
    toolId,
    approved,
    approved ? 'Approved from Node HITL example' : 'Rejected from Node HITL example',
  );
  console.log(`[hitl] confirmation ${completed ? 'resolved' : 'not found'} (${approved ? 'approved' : 'rejected'})`);
}

async function main(): Promise<void> {
  const configPath = process.env.A3S_CONFIG_FILE ?? process.argv[2];
  if (!configPath) {
    throw new Error('Set A3S_CONFIG_FILE or pass an agent.acl path as the first argument.');
  }

  const workspace = process.env.A3S_CODE_WORKSPACE ?? process.cwd();
  const prompt = process.env.A3S_CODE_HITL_PROMPT
    ?? 'Use the bash tool to run: echo A3S_CODE_HITL_OK. Then summarize the result.';

  const options: SessionOptions = {
    planningMode: 'disabled',
    permissionPolicy: {
      ask: ['bash*'],
      defaultDecision: 'allow',
    },
    confirmationPolicy: {
      enabled: true,
      defaultTimeoutMs: 120000,
      timeoutAction: 'reject',
      yoloLanes: ['query'],
    },
  };

  const agent = await Agent.create(configPath);
  const session = agent.session(workspace, options);
  const rl = createInterface({ input, output });

  try {
    const stream = await session.stream(prompt);

    while (true) {
      const next = await stream.next();
      if (next.done || !next.value) break;

      const event = next.value;
      if (event.type === 'text_delta' && event.text) {
        process.stdout.write(event.text);
      } else if (event.type === 'tool_start') {
        console.log(`\n[tool:start] ${event.toolName ?? 'unknown'}`);
      } else if (event.type === 'tool_end') {
        console.log(`\n[tool:end] ${event.toolName ?? 'unknown'} exit=${event.exitCode ?? 0}`);
      } else if (event.type === 'confirmation_required') {
        await confirmFromCli(session, event, question => rl.question(question));
      } else if (event.type === 'confirmation_received') {
        console.log(`\n[hitl] confirmation_received ${event.toolId ?? ''}`);
      } else if (event.type === 'confirmation_timeout') {
        console.log(`\n[hitl] confirmation_timeout ${event.toolId ?? ''}`);
      } else if (event.type === 'error') {
        throw new Error(event.error ?? 'stream error');
      }
    }

    console.log('\n[hitl] stream complete');
  } finally {
    rl.close();
    await session.cancelConfirmations();
  }
}

main().catch(err => {
  console.error(err);
  process.exit(1);
});
