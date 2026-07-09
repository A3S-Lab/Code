#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const INTENTIONAL_AGENT_OMISSIONS = new Map([
  [
    'from_config',
    'Rust-only constructor that accepts CodeConfig; SDKs construct from string config sources.',
  ],
]);

const INTENTIONAL_SESSION_OMISSIONS = new Map([
  ['command_registry', 'Rust MutexGuard; SDKs expose list_commands/register_command instead.'],
  ['session_cancel_token', 'Tokio CancellationToken; SDKs expose cancel/close instead.'],
  ['budget_guard', 'Rust trait-object getter; SDKs expose SessionOptions and set_budget_guard.'],
  ['subagent_tracker', 'Rust tracker handle for custom in-process executors.'],
  ['memory', 'Rust memory handle; SDKs expose typed memory methods and has_memory.'],
  ['id', 'Redundant with session_id in SDKs.'],
  ['agent_executor', 'Rust trait object; SDKs expose parallel/pipeline helpers.'],
  ['workflow', 'Rust fluent workflow object; SDKs expose parallel/pipeline helpers.'],
  [
    'workflow_with_token_budget',
    'Rust fluent workflow object; SDK parallel exposes the shared budget overload.',
  ],
  ['session_store', 'Rust trait-object getter; SDKs expose resumable operations.'],
  ['register_hook_handler', 'SDK register_hook accepts the handler in one call.'],
  ['unregister_hook_handler', 'SDK unregister_hook removes the hook and handler together.'],
  ['read_file_with_options', 'SDKs fold ReadFileOptions into read_file/readFile.'],
  [
    'tool_with_events',
    'Rust multi-handle API; not part of the stable cross-language SDK contract yet.',
  ],
  [
    'register_dynamic_tool',
    'Requires a Rust Tool trait object; SDK-safe dynamic tools need typed provider APIs.',
  ],
]);

const AGENT_ALIASES = new Map([['new', 'create']]);
const SESSION_ALIASES = new Map([['read_file_with_options', 'read_file']]);

const INTENTIONAL_SESSION_OPTION_OMISSIONS = new Map([
  ['llm_client', 'Rust LlmClient trait object; no cross-language provider callback shape yet.'],
  ['context_providers', 'Rust ContextProvider trait objects need typed SDK provider APIs.'],
  [
    'confirmation_manager',
    'Rust ConfirmationProvider trait object; SDKs expose serializable confirmation_policy.',
  ],
  [
    'permission_checker',
    'Rust PermissionChecker trait object; SDKs expose serializable permission_policy.',
  ],
  ['skill_registry', 'Rust SkillRegistry handle; SDKs expose builtin/dir/inline skill inputs.'],
  [
    'budget_guard',
    'Node cannot carry JS functions in value-typed SessionOptions; SDKs expose set_budget_guard/setBudgetGuard.',
  ],
  ['host_env', 'Rust HostEnv ID/Clock pair; no SDK-safe deterministic replay provider yet.'],
  ['sandbox_handle', 'Rust BashSandbox trait object; no SDK-safe sandbox provider yet.'],
  ['mcp_manager', 'Rust McpManager handle; SDKs expose add_mcp/remove_mcp runtime APIs.'],
]);

const SESSION_OPTION_ALIASES = new Map([
  ['workspace_services', 'workspace_backend'],
  ['auto_parallel_delegation', 'auto_parallel'],
  ['rl_trajectory', 'trajectory_path'],
  ['prompt_slots', 'role'],
  ['hook_executor', 'ahp_transport'],
]);

const SDK_AGENT_EXTRAS = ['serve_agent_dir'];
const SDK_SESSION_EXTRAS = [
  'run',
  'send_request',
  'stream_request',
  'task',
  'delegate_task',
  'tasks',
  'parallel_task',
  'program',
  'web_search',
  'git',
  'git_command',
  'add_mcp',
  'remove_mcp',
  'mcps',
  'list_commands',
  'has_memory',
  'memory_recent',
  'memory_stats',
  'get_working',
  'clear_working',
  'get_short_term',
  'clear_short_term',
];
const SDK_SESSION_OPTION_EXTRAS = [
  'builtin_skills',
  'remote_git',
  'inline_skills',
  'role',
  'guidelines',
  'response_style',
  'extra',
  'auto_parallel',
  'planning',
  'trajectory_path',
  'trajectory_mode',
  'trajectory_max_text_bytes',
  'trajectory_include_messages',
  'ahp_transport',
];

function read(rel) {
  return readFileSync(path.join(root, rel), 'utf8');
}

function stripLineForBraceCounting(line) {
  let out = '';
  let inString = false;
  let escaped = false;

  for (let i = 0; i < line.length; i += 1) {
    const ch = line[i];
    const next = line[i + 1];

    if (!inString && ch === '/' && next === '/') {
      break;
    }

    if (inString) {
      if (escaped) {
        escaped = false;
      } else if (ch === '\\') {
        escaped = true;
      } else if (ch === '"') {
        inString = false;
      }
      out += ' ';
      continue;
    }

    if (ch === '"') {
      inString = true;
      out += ' ';
      continue;
    }

    out += ch;
  }

  return out;
}

function extractImpl(src, name) {
  const lines = src.split(/\r?\n/);
  const start = lines.findIndex((line) => new RegExp(`^\\s*impl\\s+${name}\\s*\\{`).test(line));
  assert.notEqual(start, -1, `could not find impl ${name}`);

  const block = [];
  let depth = 0;
  let opened = false;
  for (let i = start; i < lines.length; i += 1) {
    const line = lines[i];
    const stripped = stripLineForBraceCounting(line);
    if (opened) {
      block.push(line);
    }
    for (const ch of stripped) {
      if (ch === '{') {
        depth += 1;
        opened = true;
      } else if (ch === '}') {
        depth -= 1;
      }
    }
    if (opened && depth === 0) {
      block.pop();
      break;
    }
  }
  return block.join('\n');
}

function extractStruct(src, name) {
  const lines = src.split(/\r?\n/);
  const start = lines.findIndex((line) =>
    new RegExp(`^\\s*(?:pub\\s+)?struct\\s+${name}\\s*\\{`).test(line),
  );
  assert.notEqual(start, -1, `could not find struct ${name}`);

  const block = [];
  let depth = 0;
  let opened = false;
  for (let i = start; i < lines.length; i += 1) {
    const line = lines[i];
    const stripped = stripLineForBraceCounting(line);
    if (opened) {
      block.push(line);
    }
    for (const ch of stripped) {
      if (ch === '{') {
        depth += 1;
        opened = true;
      } else if (ch === '}') {
        depth -= 1;
      }
    }
    if (opened && depth === 0) {
      block.pop();
      break;
    }
  }
  return block.join('\n');
}

function rustPublicMethods(block) {
  const methods = [];
  for (const line of block.split(/\r?\n/)) {
    const match = line.match(/^\s*pub\s+(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)/);
    if (match) {
      methods.push(match[1]);
    }
  }
  return [...new Set(methods)];
}

function pythonMethods(block) {
  const methods = [];
  for (const line of block.split(/\r?\n/)) {
    const match = line.match(/^\s*fn\s+([A-Za-z_][A-Za-z0-9_]*)/);
    if (match && !match[1].startsWith('__')) {
      methods.push(match[1]);
    }
  }
  return [...new Set(methods)];
}

function rustPublicFields(block) {
  const fields = [];
  for (const line of block.split(/\r?\n/)) {
    const match = line.match(/^\s*pub\s+([A-Za-z_][A-Za-z0-9_]*)\s*:/);
    if (match) {
      fields.push(match[1]);
    }
  }
  return [...new Set(fields)];
}

function pythonFields(block) {
  const fields = [];
  for (const line of block.split(/\r?\n/)) {
    const match = line.match(/^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/);
    if (match) {
      fields.push(match[1]);
    }
  }
  return [...new Set(fields)];
}

function toLowerCamel(name) {
  return name.replace(/_([a-z0-9])/g, (_, ch) => ch.toUpperCase());
}

function extractTsBlock(src, kind, name) {
  const re = new RegExp(`^export\\s+${kind}\\s+${name}\\s*\\{`, 'm');
  const match = re.exec(src);
  assert(match, `could not find TypeScript ${kind} ${name}`);
  const start = match.index + match[0].length;
  const end = src.slice(start).search(/^}/m);
  assert.notEqual(end, -1, `could not find end of TypeScript ${kind} ${name}`);
  return src.slice(start, start + end);
}

function tsClassMethods(block) {
  const methods = [];
  for (const line of block.split(/\r?\n/)) {
    const methodMatch = line.match(/^\s*(?:static\s+)?([A-Za-z_$][A-Za-z0-9_$]*)\s*\(/);
    if (methodMatch && methodMatch[1] !== 'constructor') {
      methods.push(methodMatch[1]);
      continue;
    }

    const getterMatch = line.match(/^\s*get\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*\(/);
    if (getterMatch) {
      methods.push(getterMatch[1]);
    }
  }
  return [...new Set(methods)];
}

function tsInterfaceFields(block) {
  const fields = [];
  for (const line of block.split(/\r?\n/)) {
    const match = line.match(/^\s*([A-Za-z_$][A-Za-z0-9_$]*)\??\s*:/);
    if (match) {
      fields.push(match[1]);
    }
  }
  return [...new Set(fields)];
}

function expected(coreMethods, omissions, aliases) {
  return [
    ...new Set(
      coreMethods
        .filter((name) => !omissions.has(name))
        .map((name) => aliases.get(name) ?? name),
    ),
  ].sort();
}

function assertContainsAll(label, actual, required) {
  const actualSet = new Set(actual);
  const missing = required.filter((name) => !actualSet.has(name));
  if (missing.length > 0) {
    throw new Error(`${label} is missing SDK methods: ${missing.join(', ')}`);
  }
}

const core = read('core/src/agent_api.rs');
const node = read('sdk/node/src/lib.rs');
const nodeTypes = read('sdk/node/generated.d.ts');
const python = read('sdk/python/src/lib.rs');

const coreAgent = rustPublicMethods(extractImpl(core, 'Agent'));
const coreSession = rustPublicMethods(extractImpl(core, 'AgentSession'));
const coreSessionOptions = rustPublicFields(extractStruct(core, 'SessionOptions'));
const nodeAgent = rustPublicMethods(extractImpl(node, 'Agent'));
const nodeSession = rustPublicMethods(extractImpl(node, 'Session'));
const nodeSessionOptions = rustPublicFields(extractStruct(node, 'SessionOptions'));
const nodeTypeAgent = tsClassMethods(extractTsBlock(nodeTypes, 'declare class', 'Agent'));
const nodeTypeSession = tsClassMethods(extractTsBlock(nodeTypes, 'declare class', 'Session'));
const nodeTypeSessionOptions = tsInterfaceFields(
  extractTsBlock(nodeTypes, 'interface', 'SessionOptions'),
);
const pythonAgent = pythonMethods(extractImpl(python, 'PyAgent'));
const pythonSession = pythonMethods(extractImpl(python, 'PySession'));
const pythonSessionOptions = pythonFields(extractStruct(python, 'PySessionOptions'));

const expectedAgent = expected(coreAgent, INTENTIONAL_AGENT_OMISSIONS, AGENT_ALIASES);
const expectedSession = expected(coreSession, INTENTIONAL_SESSION_OMISSIONS, SESSION_ALIASES);
const expectedSessionOptions = expected(
  coreSessionOptions,
  INTENTIONAL_SESSION_OPTION_OMISSIONS,
  SESSION_OPTION_ALIASES,
);
const requiredAgent = [...new Set([...expectedAgent, ...SDK_AGENT_EXTRAS])].sort();
const requiredSession = [...new Set([...expectedSession, ...SDK_SESSION_EXTRAS])].sort();
const requiredSessionOptions = [
  ...new Set([...expectedSessionOptions, ...SDK_SESSION_OPTION_EXTRAS]),
].sort();

assertContainsAll('Node Agent', nodeAgent, requiredAgent);
assertContainsAll('Python Agent', pythonAgent, requiredAgent);
assertContainsAll('Node Session', nodeSession, requiredSession);
assertContainsAll('Python Session', pythonSession, requiredSession);
assertContainsAll('Node SessionOptions', nodeSessionOptions, requiredSessionOptions);
assertContainsAll('Python SessionOptions', pythonSessionOptions, requiredSessionOptions);
assertContainsAll('Node generated.d.ts Agent', nodeTypeAgent, requiredAgent.map(toLowerCamel));
assertContainsAll(
  'Node generated.d.ts Session',
  nodeTypeSession,
  requiredSession.map(toLowerCamel),
);
assertContainsAll(
  'Node generated.d.ts SessionOptions',
  nodeTypeSessionOptions,
  requiredSessionOptions.map(toLowerCamel),
);

console.log(
  [
    'sdk api alignment ok',
    `core Agent required=${expectedAgent.length}`,
    `core Session required=${expectedSession.length}`,
    `core SessionOptions required=${expectedSessionOptions.length}`,
    `intentional omissions=${
      INTENTIONAL_AGENT_OMISSIONS.size
      + INTENTIONAL_SESSION_OMISSIONS.size
      + INTENTIONAL_SESSION_OPTION_OMISSIONS.size
    }`,
  ].join(' | '),
);
