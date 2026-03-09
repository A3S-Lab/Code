#!/usr/bin/env node
/**
 * TypeScript AHP (Agent Harness Protocol) reference server
 *
 * This server demonstrates:
 * - Depth-aware policy enforcement (stricter rules for sub-agents)
 * - Pattern-based command blocking
 * - Sensitive output detection
 * - JSON-RPC 2.0 over stdio transport
 *
 * Usage:
 *   npx ts-node ahp_server.ts
 *
 * Attach to A3S Code session:
 *   import { Agent, HarnessServer } from '@a3s-lab/code';
 *   const session = agent.session('.', {
 *     harnessServer: new HarnessServer('npx', ['ts-node', 'ahp_server.ts'])
 *   });
 */

import * as readline from 'readline';

interface AhpMessage {
  jsonrpc: string;
  id?: number;
  method: string;
  params: {
    event_type: string;
    payload: any;
    meta: { depth: number };
  };
}

interface AhpResponse {
  action: 'continue' | 'block' | 'skip' | 'retry';
  reason?: string;
  modified?: any;
  retry_delay_ms?: number;
}

// Dangerous command patterns (depth-aware blocking)
const BLOCKED_PATTERNS = [
  /rm\s+-rf\s+\//,           // rm -rf /
  /:\(\)\{.*\};\s*:/,        // fork bomb
  /curl.*\|\s*bash/,         // curl | bash
  /wget.*\|\s*sh/,           // wget | sh
  /dd\s+if=.*of=\/dev/,      // dd to block device
];

// Sensitive data patterns (for post-tool detection)
const SENSITIVE_OUTPUT = [
  /sk-[a-zA-Z0-9]{48}/,      // OpenAI API key
  /sk-ant-[a-zA-Z0-9-]{95}/, // Anthropic API key
  /-----BEGIN.*PRIVATE KEY-----/, // Private keys
  /ghp_[a-zA-Z0-9]{36}/,     // GitHub personal access token
];

function isDangerous(command: string): boolean {
  return BLOCKED_PATTERNS.some(pattern => pattern.test(command));
}

function hasSensitiveOutput(output: string): boolean {
  return SENSITIVE_OUTPUT.some(pattern => pattern.test(output));
}

function handlePreToolUse(payload: any, depth: number): AhpResponse {
  const tool = payload.tool || '';
  const command = payload.args?.command || '';

  // Depth-aware: stricter policy for sub-agents
  if (tool === 'Bash') {
    if (isDangerous(command)) {
      const reason = depth > 0
        ? `Blocked dangerous command at depth ${depth}: ${command.slice(0, 50)}`
        : `Blocked dangerous command: ${command.slice(0, 50)}`;
      console.error(`[BLOCK] ${reason}`);
      return { action: 'block', reason };
    }

    // Block network access for depth > 1
    if (depth > 1 && /curl|wget|nc|telnet|ssh/.test(command)) {
      console.error(`[BLOCK] Network access blocked at depth ${depth}`);
      return { action: 'block', reason: 'Network access not allowed for nested agents' };
    }

    // Block file system modifications for depth > 2
    if (depth > 2 && /rm|mv|cp|chmod|chown/.test(command)) {
      console.error(`[BLOCK] File system modification blocked at depth ${depth}`);
      return { action: 'block', reason: 'File modifications not allowed at this depth' };
    }
  }

  return { action: 'continue' };
}

function handlePrePrompt(payload: any, depth: number): AhpResponse {
  // Could inject additional context or modify the prompt here
  console.error(`[INFO] Pre-prompt at depth ${depth}`);
  return { action: 'continue' };
}

function handlePostToolUse(payload: any, depth: number): void {
  const tool = payload.tool || '';
  const output = payload.output || '';

  if (hasSensitiveOutput(output)) {
    console.error(`[ALERT] Sensitive data detected in ${tool} output at depth ${depth}`);
    // In production: send to audit log, trigger alert, redact output, etc.
  }
}

function handleNotification(eventType: string, payload: any, depth: number): void {
  switch (eventType) {
    case 'post_tool_use':
      handlePostToolUse(payload, depth);
      break;
    case 'session_start':
      console.error(`[INFO] Session started: ${payload.session_id} (depth ${depth})`);
      break;
    case 'session_end':
      console.error(`[INFO] Session ended: ${payload.session_id} (depth ${depth})`);
      break;
    case 'generate_start':
      console.error(`[INFO] LLM generation started (depth ${depth})`);
      break;
    case 'generate_end':
      console.error(`[INFO] LLM generation ended (depth ${depth})`);
      break;
    case 'on_error':
      console.error(`[ERROR] Error in session: ${payload.error} (depth ${depth})`);
      break;
    default:
      // Ignore other notifications
      break;
  }
}

function handleRequest(eventType: string, payload: any, depth: number): AhpResponse {
  switch (eventType) {
    case 'pre_tool_use':
      return handlePreToolUse(payload, depth);
    case 'pre_prompt':
      return handlePrePrompt(payload, depth);
    default:
      return { action: 'continue' };
  }
}

// Main loop: read newline-delimited JSON from stdin
const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
  terminal: false,
});

console.error('[INFO] TypeScript AHP harness server started');
console.error('[INFO] Listening for JSON-RPC 2.0 messages on stdin...');

rl.on('line', (line: string) => {
  try {
    const msg: AhpMessage = JSON.parse(line);
    const { event_type, payload, meta } = msg.params;
    const depth = meta?.depth ?? 0;
    const reqId = msg.id;

    if (reqId === undefined) {
      // Notification (fire-and-forget)
      handleNotification(event_type, payload, depth);
    } else {
      // Request (blocking)
      const result = handleRequest(event_type, payload, depth);
      const response = {
        jsonrpc: '2.0',
        id: reqId,
        result,
      };
      console.log(JSON.stringify(response));
    }
  } catch (err) {
    console.error(`[ERROR] Failed to parse message: ${err}`);
  }
});

rl.on('close', () => {
  console.error('[INFO] Harness server shutting down');
  process.exit(0);
});
