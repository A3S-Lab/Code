#!/usr/bin/env node
/**
 * Minimal stdio MCP server for integration testing.
 *
 * Exposes two tools:
 *   echo(message)   → returns the message verbatim
 *   get_secret()    → returns a server-side secret (passed as argv[2])
 *
 * The secret is unknown to the LLM unless it actually calls get_secret().
 * This prevents the LLM from faking a tool call by guessing the expected answer.
 *
 * Protocol: newline-delimited JSON-RPC 2.0
 * No external dependencies required.
 */

import * as readline from 'readline';

// Secret is passed at spawn time so the test runner knows it but the LLM does not.
const SECRET = process.argv[2] || 'default-secret';

interface Tool {
  name: string;
  description: string;
  inputSchema: {
    type: string;
    properties: Record<string, any>;
    required?: string[];
  };
}

const TOOLS: Tool[] = [
  {
    name: 'echo',
    description: 'Echo the input message back verbatim',
    inputSchema: {
      type: 'object',
      properties: {
        message: { type: 'string', description: 'The message to echo' },
      },
      required: ['message'],
    },
  },
  {
    name: 'get_secret',
    description: 'Retrieve the server-side secret value',
    inputSchema: { type: 'object', properties: {} },
  },
];

const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });

function send(obj: any): void {
  process.stdout.write(JSON.stringify(obj) + '\n');
}

interface JsonRpcMessage {
  method?: string;
  id?: string | number;
  params?: any;
}

function handle(msg: JsonRpcMessage): void {
  const { method, id, params } = msg;

  if (method === 'initialize') {
    send({
      jsonrpc: '2.0',
      id,
      result: {
        protocolVersion: '2024-11-05',
        capabilities: { tools: {} },
        serverInfo: { name: 'echo', version: '0.1.0' },
      },
    });

  } else if (method?.startsWith('notifications/')) {
    // notifications require no response

  } else if (method === 'tools/list') {
    send({ jsonrpc: '2.0', id, result: { tools: TOOLS } });

  } else if (method === 'tools/call') {
    const tool = params?.name ?? '';
    const args = params?.arguments ?? {};

    if (tool === 'echo') {
      send({
        jsonrpc: '2.0',
        id,
        result: { content: [{ type: 'text', text: args.message ?? '' }] },
      });
    } else if (tool === 'get_secret') {
      send({
        jsonrpc: '2.0',
        id,
        result: { content: [{ type: 'text', text: SECRET }] },
      });
    } else {
      send({
        jsonrpc: '2.0',
        id,
        error: { code: -32601, message: `Unknown tool: ${tool}` },
      });
    }

  } else if (id !== undefined) {
    send({
      jsonrpc: '2.0',
      id,
      error: { code: -32601, message: `Method not found: ${method}` },
    });
  }
}

rl.on('line', (line: string) => {
  line = line.trim();
  if (!line) return;
  try {
    handle(JSON.parse(line));
  } catch {}
});
