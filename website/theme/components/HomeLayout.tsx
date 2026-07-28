import { useEffect, useRef, useState } from 'react';
import { useLang, useSite, useVersion, withBase } from '@rspress/core/runtime';
import {
  InnerLine,
  Pre,
  type AnnotationHandler,
  type HighlightedCode,
} from 'codehike/code';
import {
  Selectable,
  SelectionProvider,
  useSelectedIndex,
} from 'codehike/utils/selection';
import runtimeTutorialData from '../generated/runtime-tutorial.json';

type Locale = 'zh' | 'en';

type Localized = {
  zh: string;
  en: string;
};

type Feature = {
  index: string;
  title: Localized;
  body: Localized;
  tags: string[];
};

type RuntimeTutorialStep = {
  id: string;
  layer: string;
  filename: string;
  language: string;
  title: Localized;
  body: Localized;
  note: Localized;
  tags: string[];
  focus: [number, number];
  code: string;
  highlighted: HighlightedCode;
};

const runtimeTutorialSteps =
  runtimeTutorialData as unknown as RuntimeTutorialStep[];

const installCommands = [
  {
    id: 'unix',
    label: 'macOS / Linux',
    command:
      "curl --proto '=https' --tlsv1.2 -LsSf https://raw.githubusercontent.com/A3S-Lab/a3s/main/install.sh | sh\n\na3s code",
  },
  {
    id: 'windows',
    label: 'Windows',
    command:
      '[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12\nirm https://raw.githubusercontent.com/A3S-Lab/a3s/main/install.ps1 | iex\n\na3s code',
  },
  {
    id: 'rust',
    label: 'Rust',
    command: 'cargo add a3s-code-core',
  },
  {
    id: 'node',
    label: 'Node.js',
    command: 'npm install @a3s-lab/code',
  },
  {
    id: 'python',
    label: 'Python',
    command: 'python -m pip install a3s-code',
  },
] as const;

const governanceFeatures: Feature[] = [
  {
    index: '01',
    title: {
      zh: '统一检查文件、Shell、Git 与外部请求',
      en: 'Check files, shell, Git, and external requests',
    },
    body: {
      zh: '模型提交工具参数后，Runtime 依次检查参数、Workspace 能力和权限规则。需要用户确认的调用会先暂停，不会直接执行。',
      en: 'After the model submits tool arguments, the runtime checks the arguments, workspace capability, and permission policy. Calls that need approval pause before execution.',
    },
    tags: ['policy', 'HITL', 'sandbox'],
  },
  {
    index: '02',
    title: {
      zh: '大输出保存为 Artifact',
      en: 'Store large output as artifacts',
    },
    body: {
      zh: '文件、搜索、命令和网页结果都支持范围或游标。超过限制的内容会写入 Artifact，模型只收到预览、大小、哈希和取回地址。',
      en: 'File, search, command, and web results support ranges or cursors. Oversized output is written to an artifact; the model receives a preview, size, hash, and retrieval URI.',
    },
    tags: ['cursor', 'artifact', 'hash'],
  },
  {
    index: '03',
    title: {
      zh: '界面订阅 AgentEvent',
      en: 'Render the AgentEvent stream',
    },
    body: {
      zh: '文本、工具调用、计划、确认和生命周期变化都有明确的事件类型。终端、IDE 和网页可以消费同一条事件流。',
      en: 'Text, tool calls, plans, approvals, and lifecycle changes have explicit event types. A terminal, IDE, or web app can consume the same stream.',
    },
    tags: ['AgentEvent', 'EventEnvelopeV1'],
  },
  {
    index: '04',
    title: {
      zh: 'SessionSnapshotV1 保存恢复数据',
      en: 'Resume from SessionSnapshotV1',
    },
    body: {
      zh: '会话、Run、Artifact、Trace、验证结果和子任务记录按同一代快照提交。恢复时直接读取已保存状态。',
      en: 'Sessions, runs, artifacts, traces, verification results, and child-task records are committed as one snapshot generation and loaded directly on resume.',
    },
    tags: ['snapshot', 'replay', 'verification'],
  },
];

const capabilityCards = [
  {
    className: 'a3s-bento-card--wide a3s-bento-card--policy',
    eyebrow: { zh: '工具调用', en: 'TOOL CALLS' },
    title: {
      zh: '工具列表由 Workspace 和权限共同确定',
      en: 'Workspace and policy determine the tool list',
    },
    body: {
      zh: '文件、搜索、Shell、Git、Web、Batch、QuickJS、结构化输出和子任务，只有在当前 Workspace 支持且规则允许时才会提供给模型。',
      en: 'Files, search, shell, Git, web, batch, QuickJS, structured output, and child tasks are exposed only when the current workspace supports them and policy allows them.',
    },
    tags: ['files', 'shell', 'git', 'web', 'program', 'task'],
  },
  {
    className: 'a3s-bento-card--models',
    eyebrow: { zh: '模型', en: 'MODELS' },
    title: {
      zh: '更换模型适配器，不改 Session API',
      en: 'Change the model adapter, not the Session API',
    },
    body: {
      zh: '支持 Anthropic、智谱、OpenAI-compatible API，也可以注入自己的 LlmClient。',
      en: 'Use Anthropic, Zhipu, OpenAI-compatible APIs, or inject your own LlmClient.',
    },
    tags: ['streaming', 'tools', 'structured output'],
  },
  {
    className: 'a3s-bento-card--state',
    eyebrow: { zh: '任务记录', en: 'RUN DATA' },
    title: {
      zh: 'Run、事件与快照使用稳定格式',
      en: 'Runs, events, and snapshots use stable formats',
    },
    body: {
      zh: '一次任务可以保存 Snapshot、事件、Trace、Artifact、验证结果和 Checkpoint；应用可以据此查询、审计或恢复。',
      en: 'A task can save snapshots, events, traces, artifacts, verification results, and checkpoints for queries, audit, or recovery.',
    },
    tags: ['atomic', 'replayable', 'auditable'],
  },
  {
    className: 'a3s-bento-card--extend',
    eyebrow: { zh: '扩展', en: 'EXTENSIONS' },
    title: {
      zh: '工具、上下文与存储都有扩展接口',
      en: 'Extend tools, context, and storage',
    },
    body: {
      zh: 'MCP、Skills、ContextProvider、MemoryStore、SessionStore、Workspace 服务和自定义工具都可以替换或扩展。',
      en: 'Replace or extend MCP, Skills, ContextProvider, MemoryStore, SessionStore, workspace services, and custom tools.',
    },
    tags: ['MCP', 'Skills', 'traits'],
  },
  {
    className: 'a3s-bento-card--workspace',
    eyebrow: { zh: '工作区', en: 'WORKSPACE' },
    title: {
      zh: '本地、S3 与远程 Workspace 分开声明能力',
      en: 'Local, S3, and remote workspaces declare capabilities',
    },
    body: {
      zh: '代码导航与文件工具都来自你选择的 Workspace。远程或对象存储后端不能运行本地命令时，Bash 和 Git 就不会暴露给模型。',
      en: 'Code navigation and file tools come from the workspace you select. If a remote or object-backed workspace cannot run local commands, Bash and Git are not shown to the model.',
    },
    tags: ['symbols', 'diagnostics', 'local / S3 / remote'],
  },
];

const surfaces = [
  {
    key: 'terminal',
    name: 'Terminal',
    packageName: 'a3s code',
    href: 'https://github.com/A3S-Lab/a3s',
    description: {
      zh: '开箱即用的终端界面，可以查看推理、工具调用、确认提示、任务进度和 Diff。',
      en: 'A ready-to-run terminal UI for reasoning, tool calls, approval prompts, task progress, and diffs.',
    },
    command: 'a3s code',
  },
  {
    key: 'rust',
    name: 'Rust',
    packageName: 'a3s-code-core',
    href: 'https://crates.io/crates/a3s-code-core',
    description: {
      zh: '完整的异步 Runtime API，以及用于接入自定义能力的公共 Trait。',
      en: 'The complete async runtime API, plus public traits for custom integrations.',
    },
    command: 'cargo add a3s-code-core',
  },
  {
    key: 'node',
    name: 'Node.js',
    packageName: '@a3s-lab/code',
    href: 'https://www.npmjs.com/package/@a3s-lab/code',
    description: {
      zh: '通过 N-API 提供原生绑定，覆盖会话、事件流、工具、存储、编排和 MCP。',
      en: 'Native N-API bindings for sessions, event streams, tools, storage, orchestration, and MCP.',
    },
    command: 'npm install @a3s-lab/code',
  },
  {
    key: 'python',
    name: 'Python',
    packageName: 'a3s-code',
    href: 'https://pypi.org/project/a3s-code/',
    description: {
      zh: '通过 PyO3 提供原生包，同时支持同步和异步 API。',
      en: 'A native PyO3 package with both synchronous and asynchronous APIs.',
    },
    command: 'python -m pip install a3s-code',
  },
];

const runtimeLayers = [
  {
    id: 'surfaces',
    code: 'L01 / SURFACES',
    title: { zh: '接入方式', en: 'Ways to use it' },
    body: {
      zh: '同一套 Runtime 可以直接跑在终端里，也可以通过 Rust、Node.js 或 Python 接进你的应用。接口不同，执行流程一致。',
      en: 'Run the same runtime in a terminal or embed it through Rust, Node.js, or Python. The APIs differ; the execution flow stays the same.',
    },
    tags: ['a3s code', 'Rust', 'Node.js', 'Python'],
  },
  {
    id: 'session',
    code: 'L02 / AGENT API',
    title: { zh: 'Agent 与 Session', en: 'Agent and session' },
    body: {
      zh: 'Agent 读取配置并准备共享能力；AgentSession 把这些能力连接到一个项目目录和一段对话。',
      en: 'Agent loads configuration and shared capabilities. AgentSession connects them to one project workspace and one conversation.',
    },
    tags: ['Agent', 'AgentSession', 'lifecycle'],
  },
  {
    id: 'context',
    code: 'L03 / INTELLIGENCE',
    title: { zh: '上下文、记忆与模型', en: 'Context, memory, and models' },
    body: {
      zh: 'ContextAssembler 挑选输入并控制大小，Memory 保存可复用信息，模型适配器负责流式输出、工具调用、结构化结果和取消。',
      en: 'ContextAssembler selects and sizes inputs, memory keeps reusable information, and model adapters handle streaming, tool calls, structured output, and cancellation.',
    },
    tags: ['ContextAssembler', 'Memory', 'LlmClient'],
  },
  {
    id: 'governance',
    code: 'L04 / GOVERNANCE',
    title: { zh: '权限与执行检查', en: 'Permission and execution checks' },
    body: {
      zh: '工具真正执行前，Runtime 会校验参数并检查能力和权限；再按配置进行用户确认、预算限制、沙箱隔离或取消。',
      en: 'Before a tool runs, the runtime validates its arguments and checks capabilities and permissions, then applies approval, budget, sandbox, and cancellation rules.',
    },
    tags: ['validate', 'permission', 'confirm', 'budget'],
  },
  {
    id: 'tools',
    code: 'L05 / WORKSPACE',
    title: { zh: '项目文件与工具', en: 'Project files and tools' },
    body: {
      zh: '文件、搜索、Shell、Git、网页、代码导航、MCP、Skills 和子任务，会按当前 Workspace 的能力和权限开放。',
      en: 'Files, search, shell, Git, web, code navigation, MCP, Skills, and child tasks are enabled according to the current workspace and its permissions.',
    },
    tags: ['files', 'git', 'web', 'MCP', 'Skills'],
  },
  {
    id: 'evidence',
    code: 'L06 / DURABILITY',
    title: { zh: '事件、记录与恢复', en: 'Events, records, and recovery' },
    body: {
      zh: 'AgentEvent 把执行过程交给界面；Run、Trace、Artifact、验证报告和 SessionSnapshotV1 用来排查问题、审计和恢复任务。',
      en: 'AgentEvent feeds the execution stream to your UI. Runs, traces, artifacts, verification reports, and SessionSnapshotV1 support debugging, audit, and recovery.',
    },
    tags: ['EventEnvelopeV1', 'Run', 'Artifact', 'Snapshot'],
  },
] satisfies Array<{
  id: string;
  code: string;
  title: Localized;
  body: Localized;
  tags: string[];
}>;

const tuiDemoPhases = [
  'slash',
  'mention',
  'shell',
  'research',
  'compose',
  'plan',
  'delegate',
  'track',
  'artifact',
  'remoteui',
  'answer',
] as const;
type TuiDemoPhase = (typeof tuiDemoPhases)[number];
const tuiDemoDurations = {
  slash: 1900,
  mention: 1900,
  shell: 2200,
  research: 2400,
  compose: 2400,
  plan: 1900,
  delegate: 2600,
  track: 2300,
  artifact: 2100,
  remoteui: 2600,
  answer: 4200,
} satisfies Record<TuiDemoPhase, number>;
const tuiDemoIndex = {
  compose: tuiDemoPhases.indexOf('compose'),
  plan: tuiDemoPhases.indexOf('plan'),
  delegate: tuiDemoPhases.indexOf('delegate'),
  track: tuiDemoPhases.indexOf('track'),
  artifact: tuiDemoPhases.indexOf('artifact'),
  remoteui: tuiDemoPhases.indexOf('remoteui'),
  answer: tuiDemoPhases.indexOf('answer'),
} as const;
type TuiComposerMode = 'default' | 'shell' | 'research';
type TuiDemoStatus = 'pending' | 'active' | 'done';
const tuiDemoStatusGlyph: Record<TuiDemoStatus, string> = {
  pending: '◻',
  active: '◼',
  done: '✔',
};
const tuiMascot = [
  '     .-^-.',
  '    /_____\\',
  '    ( o o )',
  '  |  /|_|\\  _',
  ' -+- |   | |#|',
  '  |  |___| \\#/',
  '     /   \\',
].join('\n');
const tuiWordmarkGlyphs = {
  A: ['01110', '10001', '10001', '11111', '10001', '10001', '10001'],
  '3': ['11110', '00001', '00001', '01110', '00001', '00001', '11110'],
  S: ['01111', '10000', '10000', '01110', '00001', '00001', '11110'],
  C: ['01111', '10000', '10000', '10000', '10000', '10000', '01111'],
  O: ['01110', '10001', '10001', '10001', '10001', '10001', '01110'],
  D: ['11110', '10001', '10001', '10001', '10001', '10001', '11110'],
  E: ['11111', '10000', '10000', '11110', '10000', '10000', '11111'],
} as const;

const tuiWordmarkVector = (() => {
  const commands: string[] = [];
  let offset = 0;

  for (const character of 'A3S CODE') {
    if (character === ' ') {
      offset += 3;
      continue;
    }

    const glyph =
      tuiWordmarkGlyphs[character as keyof typeof tuiWordmarkGlyphs];
    glyph.forEach((row, y) => {
      [...row].forEach((cell, x) => {
        if (cell === '1') commands.push(`M${offset + x} ${y}h1v1h-1z`);
      });
    });
    offset += 6;
  }

  return {
    path: commands.join(''),
    width: Math.max(offset - 1, 1),
  };
})();

function TuiWordmark() {
  return (
    <svg
      aria-hidden="true"
      className="a3s-tui-wordmark"
      focusable="false"
      preserveAspectRatio="xMinYMid meet"
      viewBox={`0 0 ${tuiWordmarkVector.width} 7`}
    >
      <path d={tuiWordmarkVector.path} />
    </svg>
  );
}

const copy = {
  zh: {
    eyebrow: 'OPEN SOURCE · EMBEDDABLE AGENT RUNTIME',
    titleLead: '把 A3S Code',
    titleAccent: '接进现有产品',
    subtitle:
      'A3S Code 提供 Agent 会话、工具调用、权限确认、事件流和任务恢复。你可以直接使用 a3s code，也可以通过 Rust、Node.js 或 Python SDK 嵌入现有应用。',
    docs: '开始使用',
    github: '查看 GitHub',
    copy: '复制',
    copied: '已复制',
    turn: '一次 Agent 执行会经过什么',
    proposal: '模型请求调用工具',
    governed: '执行前检查',
    result: '执行结果写入事件记录',
    context: '项目上下文 + 记忆',
    model: '模型',
    guard: '参数 → 权限 → 确认 → 预算 → 沙箱',
    evidence: 'Run · Trace · Artifact · Snapshot',
    record: '执行记录',
    surfacesLabel: '四种接入方式，同一套 Runtime',
    whyEyebrow: 'WHY A3S CODE',
    whyTitle: '工具执行之前，先检查参数、权限和确认状态',
    whyBody:
      '模型给出的工具调用不会直接落到文件系统或 Shell。Runtime 先完成检查，再把执行事件和结果交给应用。',
    architectureEyebrow: 'HOW IT RUNS',
    architectureTitle: '用 Python 走完一次执行',
    architectureBody:
      '示例使用仓库中实际提供的 a3s_code API。滚动或点击步骤，代码会逐步加入 Session、上下文限制、权限、事件流和持久化。',
    architectureAlt:
      'A3S Code 一次执行的交互流程图，展示任务规划、进度追踪、并行子智能体、报告制品和 RemoteUI 渐进式界面。',
    capabilitiesEyebrow: 'WHAT YOU GET',
    capabilitiesTitle: 'Runtime 提供的五类能力',
    capabilitiesBody:
      '工具、模型、任务记录、扩展接口和 Workspace 各自独立。应用可以只配置当前场景需要的部分。',
    surfacesEyebrow: 'USE IT YOUR WAY',
    surfacesTitle: '直接运行 CLI，或使用三种 SDK',
    surfacesBody:
      '终端版用于直接操作项目；Rust crate、Node.js 包和 Python 包用于 IDE、Runner、服务端或自有界面。',
    boundariesEyebrow: 'WHAT STAYS YOURS',
    boundariesTitle: 'Runtime 负责执行；应用负责账号、凭据和界面',
    boundaryItems: [
      'A3S Code Core 提供 Agent Runtime，不提供托管服务，也不规定界面应该长什么样。',
      'a3s code 的终端界面由独立的 A3S CLI 提供。',
      '账号、凭据、部署方式，以及哪些应用工具可以直接调用，仍由你的应用决定。',
    ],
    boundaryLink: '查看架构说明',
    boundaryCoreLabel: 'A3S CODE',
    boundaryCoreRole: '执行 Agent 与工具',
    boundaryContract: 'API 与事件',
    boundaryHostLabel: '你的应用',
    boundaryHostRole: '账号、权限与界面',
    stackTitle: 'TUI 生命周期',
    stackHint: '点击阶段查看过程',
    stackHintMobile: '点击步骤展开',
    stackTop: '产品',
    stackBottom: '记录',
    flowTask: '不联网检查这个仓库的发布风险，并生成报告',
    flowReplay: '重播',
    tuiWorkspace: '~/workspace/a3s',
    tuiMode: 'default',
    tuiTip: '输入消息 · / 打开命令 · Shift+Tab 切换模式 · Ctrl+C 两次退出',
    tuiSlashInput: '/effort',
    tuiSlashEffort: '调整推理强度',
    tuiSlashModel: '切换模型与 Provider',
    tuiSlashTheme: '选择终端主题',
    tuiMentionInput: '检查 @AGENTS.md',
    tuiFilePicker: '@ file · ↑/↓ · →/← folder · Enter · Esc',
    tuiFileInstructions: '项目指令',
    tuiFileManifest: 'Rust workspace',
    tuiFileWebsite: '文档应用',
    tuiShellInput: 'cargo test -p a3s-code-core',
    tuiResearchInput: '调研 A3S Code 最新发布与兼容性变化',
    tuiUser: 'You',
    tuiWorking: 'Working…',
    tuiPlan: 'Plan',
    tuiPlanInspect: '读取项目约束与发布配置',
    tuiPlanDelegate: '并行检查代码、测试与文档',
    tuiPlanPublish: '生成报告制品并打开 RemoteUI',
    tuiSubagents: 'Subagents',
    tuiParallelTask: '并行检查发布风险',
    tuiAgentExploreTask: '扫描约束与发布工作流',
    tuiAgentTestTask: '运行核心回归测试',
    tuiAgentReviewTask: '核对文档与包元数据',
    tuiArtifact: 'Artifact',
    tuiArtifactPath: 'release-risk-report/index.html',
    tuiArtifactWriting: 'publishing…',
    tuiArtifactReady: 'HTML · 18.4 KB',
    tuiOpenView: 'Open view',
    tuiRemoteUi: 'RemoteUI',
    tuiRemotePreparing: 'preparing',
    tuiRemoteStreaming: 'streaming',
    tuiRemoteReady: 'ready',
    tuiReportTitle: '发布风险报告',
    tuiReportSummary: '2 risks · 12 checks',
    tuiAssistant: 'A3S Code',
    tuiResponse:
      '检查完成。3 个子智能体并行完成；报告制品已生成，可通过 RemoteUI 打开。',
    tuiContext: 'ctx:12%',
    tutorialStep: '步骤',
    tutorialCode: '代码',
    tutorialLayers: '当前负责的层',
    tutorialScroll: '继续向下',
    ctaEyebrow: 'TRY IT',
    ctaTitle: '从一个只读任务开始',
    ctaBody:
      '安装 a3s code 后，在已有仓库里执行一次检查；需要嵌入时再选择 Rust、Node.js 或 Python SDK。',
    ctaPrimary: '查看快速开始',
    ctaSecondary: '查看 API',
    footer: 'MIT 开源 · Rust 编写 · 支持 Terminal / Rust / Node.js / Python',
  },
  en: {
    eyebrow: 'OPEN SOURCE · EMBEDDABLE AGENT RUNTIME',
    titleLead: 'Add A3S Code',
    titleAccent: 'to an existing product',
    subtitle:
      'A3S Code provides agent sessions, tool execution, approvals, event streaming, and task recovery. Run a3s code directly or embed the Rust, Node.js, or Python SDK.',
    docs: 'Get started',
    github: 'View on GitHub',
    copy: 'Copy',
    copied: 'Copied',
    turn: 'What happens during one agent turn',
    proposal: 'The model requests a tool',
    governed: 'Checks before execution',
    result: 'The result enters the event stream',
    context: 'project context + memory',
    model: 'model',
    guard: 'arguments → permission → approval → budget → sandbox',
    evidence: 'Run · Trace · Artifact · Snapshot',
    record: 'run record',
    surfacesLabel: 'Four ways in, one runtime',
    whyEyebrow: 'WHY A3S CODE',
    whyTitle: 'Check arguments, permissions, and approvals before execution',
    whyBody:
      'Model tool calls do not go straight to the filesystem or shell. The runtime completes its checks first, then sends execution events and results to the application.',
    architectureEyebrow: 'HOW IT RUNS',
    architectureTitle: 'Follow one complete run in Python',
    architectureBody:
      'The example uses the actual a3s_code API in this repository. Scroll or select a step to add the Session, context limits, policy, event stream, and persistence.',
    architectureAlt:
      'An interactive A3S Code run showing task planning, progress tracking, parallel subagents, report artifacts, and a progressive RemoteUI view.',
    capabilitiesEyebrow: 'WHAT YOU GET',
    capabilitiesTitle: 'Five parts of the runtime',
    capabilitiesBody:
      'Tools, models, run data, extension interfaces, and workspaces are configured separately. An application can enable only the parts it needs.',
    surfacesEyebrow: 'USE IT YOUR WAY',
    surfacesTitle: 'Run the CLI or use one of three SDKs',
    surfacesBody:
      'Use the terminal app directly in a repository. Use the Rust crate, Node.js package, or Python package in an IDE, runner, server, or custom interface.',
    boundariesEyebrow: 'WHAT STAYS YOURS',
    boundariesTitle: 'The runtime executes; the app owns accounts and access',
    boundaryItems: [
      'A3S Code Core provides the agent runtime. It is not a hosted service and does not dictate your UI.',
      'The a3s code terminal interface comes from the separate A3S CLI.',
      'Accounts, credentials, deployment, and direct access to application tools remain under your control.',
    ],
    boundaryLink: 'Read the architecture guide',
    boundaryCoreLabel: 'A3S CODE',
    boundaryCoreRole: 'RUNS AGENTS + TOOLS',
    boundaryContract: 'APIs + EVENTS',
    boundaryHostLabel: 'YOUR APP',
    boundaryHostRole: 'OWNS UI + ACCESS',
    stackTitle: 'TUI lifecycle',
    stackHint: 'SELECT A PHASE TO INSPECT IT',
    stackHintMobile: 'TAP A STEP TO EXPAND',
    stackTop: 'PRODUCT',
    stackBottom: 'RECORDS',
    flowTask: 'Audit this repo for release risks offline; generate a report',
    flowReplay: 'REPLAY',
    tuiWorkspace: '~/workspace/a3s',
    tuiMode: 'default',
    tuiTip:
      'Type a message · / for commands · Shift+Tab cycles mode · Ctrl+C twice to exit',
    tuiSlashInput: '/effort',
    tuiSlashEffort: 'adjust model effort',
    tuiSlashModel: 'switch model and provider',
    tuiSlashTheme: 'select the terminal theme',
    tuiMentionInput: 'Review @AGENTS.md',
    tuiFilePicker: '@ file · ↑/↓ · →/← folder · Enter · Esc',
    tuiFileInstructions: 'workspace instructions',
    tuiFileManifest: 'Rust workspace',
    tuiFileWebsite: 'documentation app',
    tuiShellInput: 'cargo test -p a3s-code-core',
    tuiResearchInput: 'Research the latest A3S Code releases and compatibility',
    tuiUser: 'You',
    tuiWorking: 'Working…',
    tuiPlan: 'Plan',
    tuiPlanInspect: 'Read repo constraints and release config',
    tuiPlanDelegate: 'Check code, tests, and docs in parallel',
    tuiPlanPublish: 'Publish report artifact and open RemoteUI',
    tuiSubagents: 'Subagents',
    tuiParallelTask: 'Audit release risks in parallel',
    tuiAgentExploreTask: 'Scan constraints and release workflows',
    tuiAgentTestTask: 'Run the core regression suite',
    tuiAgentReviewTask: 'Review docs and package metadata',
    tuiArtifact: 'Artifact',
    tuiArtifactPath: 'release-risk-report/index.html',
    tuiArtifactWriting: 'publishing…',
    tuiArtifactReady: 'HTML · 18.4 KB',
    tuiOpenView: 'Open view',
    tuiRemoteUi: 'RemoteUI',
    tuiRemotePreparing: 'preparing',
    tuiRemoteStreaming: 'streaming',
    tuiRemoteReady: 'ready',
    tuiReportTitle: 'Release risk report',
    tuiReportSummary: '2 risks · 12 checks',
    tuiAssistant: 'A3S Code',
    tuiResponse:
      'Review complete. Three subagents finished in parallel; the report artifact is ready to open in RemoteUI.',
    tuiContext: 'ctx:12%',
    tutorialStep: 'STEP',
    tutorialCode: 'CODE',
    tutorialLayers: 'ACTIVE LAYER',
    tutorialScroll: 'KEEP SCROLLING',
    ctaEyebrow: 'TRY IT',
    ctaTitle: 'Start with a read-only task',
    ctaBody:
      'Install a3s code and run one inspection in an existing repository. Choose the Rust, Node.js, or Python SDK when you are ready to embed it.',
    ctaPrimary: 'Open the quick start',
    ctaSecondary: 'Open the API reference',
    footer: 'MIT licensed · Built in Rust · Terminal / Rust / Node.js / Python',
  },
};

function localeValue(value: Localized, locale: Locale) {
  return value[locale];
}

function ArrowIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 16 16">
      <path d="M3 8h9M8.5 3.5 13 8l-4.5 4.5" />
    </svg>
  );
}

function AnimatedButtonBorder() {
  return (
    <span aria-hidden="true" className="a3s-button-orbit">
      <span className="a3s-button-orbit-gradient" />
    </span>
  );
}

function GitHubIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24">
      <path d="M12 .8A11.5 11.5 0 0 0 8.36 23.2c.58.1.79-.25.79-.56v-2.2c-3.22.7-3.9-1.36-3.9-1.36-.52-1.34-1.28-1.7-1.28-1.7-1.05-.72.08-.7.08-.7 1.16.08 1.77 1.2 1.77 1.2 1.03 1.78 2.71 1.27 3.37.97.1-.75.4-1.27.73-1.56-2.57-.3-5.28-1.3-5.28-5.7 0-1.27.45-2.3 1.19-3.11-.12-.3-.52-1.48.11-3.07 0 0 .97-.31 3.16 1.19a10.86 10.86 0 0 1 5.76 0c2.2-1.5 3.16-1.19 3.16-1.19.63 1.6.23 2.77.11 3.07.74.81 1.19 1.84 1.19 3.1 0 4.43-2.71 5.4-5.29 5.69.42.36.79 1.07.79 2.16v3.2c0 .31.21.67.8.55A11.5 11.5 0 0 0 12 .8Z" />
    </svg>
  );
}

function InstallSwitcher({
  locale,
  labels,
}: {
  locale: Locale;
  labels: (typeof copy)[Locale];
}) {
  const [activeId, setActiveId] =
    useState<(typeof installCommands)[number]['id']>('unix');
  const [copied, setCopied] = useState(false);
  const active =
    installCommands.find((item) => item.id === activeId) ?? installCommands[0];

  async function copyActiveCommand() {
    await navigator.clipboard.writeText(active.command);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1600);
  }

  return (
    <div className="a3s-install">
      <div className="a3s-install-tabs" role="tablist" aria-label="Install">
        {installCommands.map((item) => (
          <button
            aria-selected={active.id === item.id}
            className={active.id === item.id ? 'is-active' : undefined}
            key={item.id}
            onClick={() => {
              setActiveId(item.id);
              setCopied(false);
            }}
            role="tab"
            type="button"
          >
            {item.label}
          </button>
        ))}
      </div>
      <div className="a3s-command" role="tabpanel">
        <pre>
          <code>{active.command}</code>
        </pre>
        <button
          className="a3s-copy-button"
          onClick={copyActiveCommand}
          type="button"
        >
          <span aria-hidden="true">{copied ? '✓' : '⧉'}</span>
          {copied ? labels.copied : labels.copy}
        </button>
      </div>
      <span className="a3s-install-locale" aria-hidden="true">
        {locale === 'zh' ? 'ZH' : 'EN'}
      </span>
    </div>
  );
}

function RuntimeExecutionFlow({ labels }: { labels: (typeof copy)[Locale] }) {
  const playerRef = useRef<HTMLDivElement>(null);
  const hasStartedRef = useRef(false);
  const [activeIndex, setActiveIndex] = useState(tuiDemoPhases.length - 1);
  const [isPlaying, setIsPlaying] = useState(false);
  const [isVisible, setIsVisible] = useState(false);
  const [typedCount, setTypedCount] = useState(0);
  const active = tuiDemoPhases[activeIndex] ?? tuiDemoPhases[0];
  const composerDemos: Partial<
    Record<
      TuiDemoPhase,
      { mode: TuiComposerMode; symbol: string; text: string }
    >
  > = {
    slash: { mode: 'default', symbol: '❯', text: labels.tuiSlashInput },
    mention: { mode: 'default', symbol: '❯', text: labels.tuiMentionInput },
    shell: { mode: 'shell', symbol: '!', text: labels.tuiShellInput },
    research: {
      mode: 'research',
      symbol: '?',
      text: labels.tuiResearchInput,
    },
    compose: { mode: 'default', symbol: '❯', text: labels.flowTask },
  };
  const composerDemo = composerDemos[active];
  const composerMode = composerDemo?.mode ?? 'default';
  const composerText = composerDemo?.text ?? '';
  const typedComposerText = composerText.slice(0, typedCount);
  const composerSymbol = composerDemo?.symbol ?? '❯';
  const composerStatus =
    composerMode === 'research'
      ? '◇ deep research · --web | --local-only'
      : '◇ high';
  const mentionStart = typedComposerText.lastIndexOf('@');
  const mentionQuery =
    mentionStart >= 0
      ? typedComposerText.slice(mentionStart + 1).toLowerCase()
      : '';
  const inputMenuIsOpen =
    (active === 'slash' && typedComposerText.startsWith('/')) ||
    (active === 'mention' && mentionStart >= 0);
  const slashMenuItems: Array<[string, string]> = [
    ['/effort', labels.tuiSlashEffort],
    ['/model', labels.tuiSlashModel],
    ['/theme', labels.tuiSlashTheme],
  ];
  const fileMenuItems: Array<[string, string]> = [
    ['AGENTS.md', labels.tuiFileInstructions],
    ['Cargo.toml', labels.tuiFileManifest],
    ['website/', labels.tuiFileWebsite],
  ];
  const inputMenuItems: Array<[string, string]> =
    active === 'slash'
      ? slashMenuItems.filter(([command]) =>
          command.startsWith(typedComposerText || '/'),
        )
      : active === 'mention'
        ? fileMenuItems.filter(([path]) =>
            path.toLowerCase().includes(mentionQuery),
          )
        : [];
  const isRunning = isPlaying && isVisible;
  const isWorking =
    activeIndex > tuiDemoIndex.compose && activeIndex < tuiDemoIndex.answer;
  const planItems: Array<{ label: string; status: TuiDemoStatus }> = [
    {
      label: labels.tuiPlanInspect,
      status:
        activeIndex < tuiDemoIndex.plan
          ? 'pending'
          : activeIndex === tuiDemoIndex.plan
            ? 'active'
            : 'done',
    },
    {
      label: labels.tuiPlanDelegate,
      status:
        activeIndex < tuiDemoIndex.delegate
          ? 'pending'
          : activeIndex < tuiDemoIndex.artifact
            ? 'active'
            : 'done',
    },
    {
      label: labels.tuiPlanPublish,
      status:
        activeIndex < tuiDemoIndex.artifact
          ? 'pending'
          : activeIndex < tuiDemoIndex.remoteui
            ? 'active'
            : 'done',
    },
  ];
  const subagents: Array<{
    name: string;
    task: string;
    status: TuiDemoStatus;
    tokens: string;
  }> = [
    {
      name: 'explore',
      task: labels.tuiAgentExploreTask,
      status: activeIndex >= tuiDemoIndex.track ? 'done' : 'active',
      tokens: '0.8k',
    },
    {
      name: 'test',
      task: labels.tuiAgentTestTask,
      status: activeIndex >= tuiDemoIndex.artifact ? 'done' : 'active',
      tokens: '1.5k',
    },
    {
      name: 'review',
      task: labels.tuiAgentReviewTask,
      status: activeIndex >= tuiDemoIndex.track ? 'done' : 'active',
      tokens: '0.9k',
    },
  ];
  const completedSubagentCount = subagents.filter(
    (agent) => agent.status === 'done',
  ).length;
  const runningSubagentCount = subagents.length - completedSubagentCount;
  const visibleSubagents =
    activeIndex === tuiDemoIndex.delegate
      ? subagents
      : activeIndex === tuiDemoIndex.track
        ? subagents.filter((agent) => agent.status === 'active')
        : [];
  const subagentTokens =
    activeIndex < tuiDemoIndex.track
      ? '1.8k'
      : activeIndex < tuiDemoIndex.artifact
        ? '2.7k'
        : '3.2k';
  const artifactIsPublished = activeIndex >= tuiDemoIndex.remoteui;
  const remoteUiIsReady = activeIndex >= tuiDemoIndex.answer;
  const typingInterval = Math.max(
    16,
    Math.floor(1200 / Math.max(composerText.length, 1)),
  );

  useEffect(() => {
    const player = playerRef.current;
    if (!player) return undefined;

    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      setActiveIndex(tuiDemoPhases.length - 1);
      return undefined;
    }

    const observer = new IntersectionObserver(
      ([entry]) => {
        const visible = entry?.isIntersecting ?? false;
        setIsVisible(visible);

        if (visible && !hasStartedRef.current) {
          hasStartedRef.current = true;
          setTypedCount(0);
          setActiveIndex(0);
          setIsPlaying(true);
        }
      },
      { threshold: 0.35 },
    );

    observer.observe(player);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!isRunning) return undefined;

    const delay = tuiDemoDurations[active] ?? 1600;
    const timer = window.setTimeout(() => {
      setTypedCount(0);
      if (activeIndex >= tuiDemoPhases.length - 1) {
        setActiveIndex(0);
      } else {
        setActiveIndex((index) => index + 1);
      }
    }, delay);

    return () => window.clearTimeout(timer);
  }, [active, activeIndex, isRunning]);

  useEffect(() => {
    if (!isRunning || composerText.length === 0) return undefined;
    if (typedCount >= composerText.length) return undefined;

    const timer = window.setTimeout(
      () => setTypedCount((count) => count + 1),
      typingInterval,
    );

    return () => window.clearTimeout(timer);
  }, [active, composerText.length, isRunning, typedCount, typingInterval]);

  function playFlow() {
    if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    setTypedCount(0);
    setActiveIndex(0);
    setIsPlaying(true);
  }

  return (
    <div
      className={[
        'a3s-runtime-inspector',
        'a3s-tui-player',
        isRunning ? 'is-running' : '',
      ]
        .filter(Boolean)
        .join(' ')}
      data-phase={active}
      aria-label={labels.architectureAlt}
      ref={playerRef}
    >
      <header className="a3s-tui-titlebar">
        <span className="a3s-tui-window-dots" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
        <div className="a3s-tui-title">
          <b>a3s code</b>
          <em>{labels.tuiWorkspace}</em>
        </div>
        <button
          className={isRunning ? 'is-playing' : ''}
          onClick={playFlow}
          type="button"
        >
          <i aria-hidden="true" />
          {labels.flowReplay}
        </button>
      </header>

      <section className="a3s-tui-terminal">
        <div className="a3s-tui-welcome" aria-label="A3S Code">
          <pre aria-hidden="true" className="a3s-tui-mascot">
            {tuiMascot}
          </pre>
          <TuiWordmark />
        </div>
        <p className="a3s-tui-meta">
          <span>a3s-code v0.10.9</span>
          <i>·</i>
          <span>openai/gpt-5</span>
          <i>·</i>
          <span>12 skills</span>
          <i>·</i>
          <span>{labels.tuiWorkspace}</span>
        </p>
        <p className="a3s-tui-tip">{labels.tuiTip}</p>

        <div className="a3s-tui-transcript" aria-live="off">
          {activeIndex > tuiDemoIndex.compose ? (
            <article className="a3s-tui-entry a3s-tui-entry--user">
              <span aria-hidden="true">›</span>
              <div>
                <small>{labels.tuiUser}</small>
                <p>{labels.flowTask}</p>
              </div>
            </article>
          ) : null}

          {activeIndex >= tuiDemoIndex.artifact ? (
            <article
              className={[
                'a3s-tui-artifact',
                artifactIsPublished ? 'is-published' : 'is-publishing',
                remoteUiIsReady ? 'is-ready' : '',
              ]
                .filter(Boolean)
                .join(' ')}
            >
              <div className="a3s-tui-artifact-meta">
                <header>
                  <span aria-hidden="true">◇</span>
                  <strong>{labels.tuiArtifact}</strong>
                  <small>
                    {artifactIsPublished
                      ? labels.tuiArtifactReady
                      : labels.tuiArtifactWriting}
                  </small>
                </header>
                <code>{labels.tuiArtifactPath}</code>
                <span
                  className={[
                    'a3s-tui-open-view',
                    activeIndex === tuiDemoIndex.remoteui ? 'is-opening' : '',
                  ]
                    .filter(Boolean)
                    .join(' ')}
                >
                  <i aria-hidden="true">↗</i>
                  {artifactIsPublished
                    ? labels.tuiOpenView
                    : labels.tuiArtifactWriting}
                  {artifactIsPublished ? <b>{labels.tuiRemoteUi}</b> : null}
                </span>
              </div>
              <div
                className={[
                  'a3s-tui-remote-view',
                  !artifactIsPublished
                    ? 'is-preparing'
                    : remoteUiIsReady
                      ? 'is-ready'
                      : 'is-streaming',
                ].join(' ')}
              >
                <header>
                  <span aria-hidden="true">●</span>
                  <b>{labels.tuiRemoteUi}</b>
                  <small>
                    {!artifactIsPublished
                      ? labels.tuiRemotePreparing
                      : remoteUiIsReady
                        ? labels.tuiRemoteReady
                        : labels.tuiRemoteStreaming}
                  </small>
                </header>
                <div>
                  <strong>{labels.tuiReportTitle}</strong>
                  <small>
                    {remoteUiIsReady ? labels.tuiReportSummary : '···'}
                  </small>
                  <span aria-hidden="true">
                    <i />
                    <i />
                    <i />
                  </span>
                </div>
              </div>
            </article>
          ) : null}

          {activeIndex >= tuiDemoIndex.answer ? (
            <article className="a3s-tui-entry a3s-tui-entry--assistant">
              <span aria-hidden="true">•</span>
              <div>
                <strong>{labels.tuiAssistant}</strong>
                <p>{labels.tuiResponse}</p>
              </div>
            </article>
          ) : null}

          {inputMenuIsOpen ? (
            <div
              aria-hidden="true"
              className={`a3s-tui-input-menu is-${active}`}
            >
              {active === 'mention' ? (
                <strong>{labels.tuiFilePicker}</strong>
              ) : null}
              <ul>
                {inputMenuItems.map(([label, detail], index) => (
                  <li className={index === 0 ? 'is-selected' : ''} key={label}>
                    <code>{label}</code>
                    <span>{detail}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
        </div>
      </section>

      <section className="a3s-tui-composer" data-input-mode={composerMode}>
        <div className="a3s-tui-activity" aria-live="polite">
          {isWorking ? (
            <>
              <i aria-hidden="true">✶</i>
              <span>{labels.tuiWorking}</span>
              <small>(00:04 · ↓ 1.2k tokens)</small>
            </>
          ) : null}
        </div>
        {activeIndex >= tuiDemoIndex.plan ? (
          <section className="a3s-tui-plan" aria-label={labels.tuiPlan}>
            <ol>
              {planItems.map((item, index) => (
                <li className={`is-${item.status}`} key={item.label}>
                  <span aria-hidden="true">{index === 0 ? '⎿' : ''}</span>
                  <i aria-hidden="true">{tuiDemoStatusGlyph[item.status]}</i>
                  <p>{item.label}</p>
                </li>
              ))}
            </ol>
          </section>
        ) : null}
        <div className="a3s-tui-effort-rule">
          <span>{composerStatus}</span>
        </div>
        <div className="a3s-tui-input">
          <span aria-hidden="true">{composerSymbol}</span>
          <p>
            {typedComposerText}
            <i aria-hidden="true" />
          </p>
        </div>
        <div className="a3s-tui-input-rule" />
        <footer className="a3s-tui-footer">
          <span className="a3s-tui-mode">
            <i aria-hidden="true">●</i>
            {labels.tuiMode}
          </span>
          <span className="a3s-tui-context">
            {labels.tuiContext}
            <i aria-hidden="true">
              <b />
            </i>
          </span>
          <span className="a3s-tui-identity">
            <b>a3s</b>
            <em>git:(main)</em>
            <em>gpt-5 (128k context)</em>
          </span>
        </footer>
        {visibleSubagents.length > 0 ? (
          <section className="a3s-tui-agents" aria-label={labels.tuiSubagents}>
            <header>
              <span aria-hidden="true">•</span>
              <strong>{labels.tuiParallelTask}</strong>
              <small>
                {runningSubagentCount} running · {completedSubagentCount}/3 done
                · 00:04 · ↓ {subagentTokens} tokens
              </small>
            </header>
            <div>
              {visibleSubagents.map((agent) => (
                <p className={`is-${agent.status}`} key={agent.name}>
                  <i aria-hidden="true">•</i>
                  <b>{agent.name}</b>
                  <span>{agent.task}</span>
                  <small>00:03 · ↓ {agent.tokens}</small>
                </p>
              ))}
            </div>
          </section>
        ) : null}
      </section>
    </div>
  );
}

const runtimeFocusHandler: AnnotationHandler = {
  name: 'focus',
  onlyIfAnnotated: true,
  Line: (props) => (
    <InnerLine
      className="a3s-code-line"
      data-line-number={props.lineNumber}
      merge={props}
    />
  ),
  AnnotatedLine: ({ annotation: _annotation, ...props }) => (
    <InnerLine
      className="a3s-code-line is-focused"
      data-focus="true"
      data-line-number={props.lineNumber}
      merge={props}
    />
  ),
};

function TutorialCode({
  labels,
  step,
}: {
  labels: (typeof copy)[Locale];
  step: RuntimeTutorialStep;
}) {
  const [copied, setCopied] = useState(false);

  async function copyCode() {
    await navigator.clipboard.writeText(step.code);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1400);
  }

  return (
    <div className="a3s-tutorial-code">
      <header>
        <span>
          <i aria-hidden="true" />
          {step.filename}
        </span>
        <span>{step.language.toUpperCase()}</span>
        <button onClick={copyCode} type="button">
          {copied ? labels.copied : labels.copy}
        </button>
      </header>
      <Pre
        code={step.highlighted}
        handlers={[runtimeFocusHandler]}
        key={step.id}
      />
    </div>
  );
}

function RuntimeLayerRail({
  activeIndex,
  labels,
  locale,
  setActiveIndex,
}: {
  activeIndex: number;
  labels: (typeof copy)[Locale];
  locale: Locale;
  setActiveIndex: (index: number) => void;
}) {
  return (
    <div className="a3s-tutorial-layers">
      <header>
        <span>{labels.tutorialLayers}</span>
        <small>{labels.stackHint}</small>
      </header>
      <div>
        {runtimeLayers.map((layer, index) => (
          <button
            aria-current={activeIndex === index ? 'step' : undefined}
            className={[
              activeIndex === index ? 'is-active' : '',
              activeIndex > index ? 'is-complete' : '',
            ]
              .filter(Boolean)
              .join(' ')}
            key={layer.id}
            onClick={() => setActiveIndex(index)}
            type="button"
          >
            <span>{String(index + 1).padStart(2, '0')}</span>
            <span>
              <small>{layer.code.replace(/^L\d+\s*\/\s*/, '')}</small>
              <strong>{localeValue(layer.title, locale)}</strong>
            </span>
            <i aria-hidden="true" />
          </button>
        ))}
      </div>
    </div>
  );
}

function RuntimeTutorialStage({
  locale,
  labels,
}: {
  locale: Locale;
  labels: (typeof copy)[Locale];
}) {
  const [selectedIndex, setSelectedIndex] = useSelectedIndex();
  const activeIndex = Math.min(
    Math.max(selectedIndex, 0),
    runtimeTutorialSteps.length - 1,
  );
  const step = runtimeTutorialSteps[activeIndex];

  return (
    <div className="a3s-tutorial-stage">
      <div className="a3s-tutorial-stage-toolbar">
        <span>{labels.stackTitle}</span>
        <span aria-live="polite">
          {labels.tutorialStep} {String(activeIndex + 1).padStart(2, '0')} /{' '}
          {String(runtimeTutorialSteps.length).padStart(2, '0')}
        </span>
      </div>
      <div className="a3s-tutorial-stage-grid">
        <TutorialCode labels={labels} step={step} />
        <div className="a3s-tutorial-stage-side">
          <RuntimeLayerRail
            activeIndex={activeIndex}
            labels={labels}
            locale={locale}
            setActiveIndex={setSelectedIndex}
          />
          <div className="a3s-tutorial-note">
            <span>{step.layer}</span>
            <p>{localeValue(step.note, locale)}</p>
          </div>
        </div>
      </div>
    </div>
  );
}

function RuntimeTutorialSteps({
  labels,
  locale,
}: {
  labels: (typeof copy)[Locale];
  locale: Locale;
}) {
  const [, setSelectedIndex] = useSelectedIndex();

  return (
    <div className="a3s-tutorial-steps">
      {runtimeTutorialSteps.map((step, index) => (
        <Selectable
          className="a3s-tutorial-step"
          index={index}
          key={step.id}
          selectOn={['scroll']}
        >
          <button
            onClick={() => setSelectedIndex(index)}
            onFocus={() => setSelectedIndex(index)}
            onMouseEnter={() => setSelectedIndex(index)}
            type="button"
          >
            <span className="a3s-tutorial-step-number">
              {String(index + 1).padStart(2, '0')}
            </span>
            <span className="a3s-tutorial-step-layer">{step.layer}</span>
            <h3>{localeValue(step.title, locale)}</h3>
            <p>{localeValue(step.body, locale)}</p>
            <span className="a3s-tutorial-step-tags">
              {step.tags.map((tag) => (
                <i key={tag}>{tag}</i>
              ))}
            </span>
            <span className="a3s-tutorial-step-progress" aria-hidden="true" />
          </button>
          <div className="a3s-tutorial-mobile-preview">
            <div>
              <span>{labels.tutorialLayers}</span>
              <strong>{localeValue(runtimeLayers[index].title, locale)}</strong>
            </div>
            <TutorialCode labels={labels} step={step} />
          </div>
        </Selectable>
      ))}
    </div>
  );
}

function RuntimeTutorial({
  labels,
  locale,
}: {
  labels: (typeof copy)[Locale];
  locale: Locale;
}) {
  return (
    <SelectionProvider
      className="a3s-runtime-tutorial"
      rootMargin="-42% 0px -42% 0px"
    >
      <RuntimeTutorialSteps labels={labels} locale={locale} />
      <aside className="a3s-tutorial-sticky">
        <RuntimeTutorialStage labels={labels} locale={locale} />
      </aside>
    </SelectionProvider>
  );
}

function MarkdownHome({
  locale,
  labels,
}: {
  locale: Locale;
  labels: (typeof copy)[Locale];
}) {
  return (
    <main>
      <h1>
        {labels.titleLead} {labels.titleAccent}
      </h1>
      <p>{labels.subtitle}</p>
      <h2>{labels.whyTitle}</h2>
      <p>{labels.whyBody}</p>
      {governanceFeatures.map((feature) => (
        <section key={feature.index}>
          <h3>{localeValue(feature.title, locale)}</h3>
          <p>{localeValue(feature.body, locale)}</p>
        </section>
      ))}
      <h2>{labels.architectureTitle}</h2>
      <p>{labels.architectureBody}</p>
      <h2>{labels.surfacesTitle}</h2>
      {surfaces.map((surface) => (
        <section key={surface.key}>
          <h3>
            {surface.name}: {surface.packageName}
          </h3>
          <p>{localeValue(surface.description, locale)}</p>
          <pre>
            <code>{surface.command}</code>
          </pre>
        </section>
      ))}
      <h2>{labels.boundariesTitle}</h2>
      <ul>
        {labels.boundaryItems.map((item) => (
          <li key={item}>{item}</li>
        ))}
      </ul>
    </main>
  );
}

export function HomeLayout() {
  const rawLang = useLang();
  const locale: Locale = rawLang === 'zh' ? 'zh' : 'en';
  const labels = copy[locale];
  const version = useVersion();
  const { site } = useSite();
  const defaultVersion = site.multiVersion.default;
  const routePrefix = [
    version && version !== defaultVersion ? version : '',
    locale !== site.lang ? locale : '',
  ]
    .filter(Boolean)
    .join('/');
  const route = (pathname: string) => {
    const normalizedPath = pathname.replace(/^\/+/, '');
    const parts = [routePrefix, normalizedPath].filter(Boolean).join('/');
    return withBase(`/${parts}`);
  };

  if (import.meta.env.SSG_MD) {
    return <MarkdownHome labels={labels} locale={locale} />;
  }

  return (
    <main className="a3s-home">
      <section className="a3s-hero">
        <div className="a3s-hero-copy">
          <div className="a3s-eyebrow">
            <span />
            {labels.eyebrow}
          </div>
          <h1>
            {labels.titleLead}
            <span>{labels.titleAccent}</span>
          </h1>
          <p className="a3s-hero-subtitle">{labels.subtitle}</p>
          <div className="a3s-hero-actions">
            <a
              className="a3s-button a3s-button--primary"
              href={route('/guide/')}
            >
              <AnimatedButtonBorder />
              {labels.docs}
              <ArrowIcon />
            </a>
            <a
              className="a3s-button a3s-button--secondary"
              href="https://github.com/A3S-Lab/Code"
            >
              <GitHubIcon />
              {labels.github}
            </a>
          </div>
          <InstallSwitcher labels={labels} locale={locale} />
        </div>
        <div className="a3s-hero-visual">
          <RuntimeExecutionFlow labels={labels} />
        </div>
      </section>

      <section className="a3s-section a3s-why" id="why-a3s-code">
        <header className="a3s-section-header">
          <div>
            <span className="a3s-section-eyebrow">{labels.whyEyebrow}</span>
            <h2>{labels.whyTitle}</h2>
          </div>
          <p>{labels.whyBody}</p>
        </header>
        <div className="a3s-feature-grid">
          {governanceFeatures.map((feature) => (
            <article className="a3s-feature-card" key={feature.index}>
              <div className="a3s-feature-number">{feature.index}</div>
              <h3>{localeValue(feature.title, locale)}</h3>
              <p>{localeValue(feature.body, locale)}</p>
              <div className="a3s-tag-row">
                {feature.tags.map((tag) => (
                  <span key={tag}>{tag}</span>
                ))}
              </div>
            </article>
          ))}
        </div>
      </section>

      <section className="a3s-section a3s-tutorial-section" id="runtime-stack">
        <header className="a3s-section-header a3s-tutorial-header">
          <div>
            <span className="a3s-section-eyebrow">
              {labels.architectureEyebrow}
            </span>
            <h2>{labels.architectureTitle}</h2>
          </div>
          <div>
            <p>{labels.architectureBody}</p>
            <a href={route('/guide/architecture.html')}>
              {labels.boundaryLink}
              <ArrowIcon />
            </a>
          </div>
        </header>
        <RuntimeTutorial labels={labels} locale={locale} />
      </section>

      <section className="a3s-section a3s-capabilities" id="capabilities">
        <header className="a3s-section-header">
          <div>
            <span className="a3s-section-eyebrow">
              {labels.capabilitiesEyebrow}
            </span>
            <h2>{labels.capabilitiesTitle}</h2>
          </div>
          <p>{labels.capabilitiesBody}</p>
        </header>
        <div className="a3s-bento-grid">
          {capabilityCards.map((card) => (
            <article
              className={`a3s-bento-card ${card.className}`}
              key={card.eyebrow.en}
            >
              <span>{localeValue(card.eyebrow, locale)}</span>
              <h3>{localeValue(card.title, locale)}</h3>
              <p>{localeValue(card.body, locale)}</p>
              <div className="a3s-tag-row">
                {card.tags.map((tag) => (
                  <span key={tag}>{tag}</span>
                ))}
              </div>
            </article>
          ))}
        </div>
      </section>

      <section className="a3s-section a3s-surfaces" id="surfaces">
        <header className="a3s-section-header">
          <div>
            <span className="a3s-section-eyebrow">
              {labels.surfacesEyebrow}
            </span>
            <h2>{labels.surfacesTitle}</h2>
          </div>
          <p>{labels.surfacesBody}</p>
        </header>
        <div className="a3s-surface-grid">
          {surfaces.map((surface) => (
            <article className="a3s-surface-card" key={surface.key}>
              <div className="a3s-surface-card-heading">
                <span>{surface.name}</span>
                <a href={surface.href} aria-label={surface.packageName}>
                  <ArrowIcon />
                </a>
              </div>
              <h3>{surface.packageName}</h3>
              <p>{localeValue(surface.description, locale)}</p>
              <code>{surface.command}</code>
            </article>
          ))}
        </div>
      </section>

      <section className="a3s-section a3s-boundaries" id="boundaries">
        <div className="a3s-boundaries-art" aria-hidden="true">
          <div className="a3s-boundary-core">
            <span>{labels.boundaryCoreLabel}</span>
            <strong>{labels.boundaryCoreRole}</strong>
          </div>
          <div className="a3s-boundary-line">
            <span>{labels.boundaryContract}</span>
          </div>
          <div className="a3s-boundary-host">
            <span>{labels.boundaryHostLabel}</span>
            <strong>{labels.boundaryHostRole}</strong>
          </div>
        </div>
        <div className="a3s-boundaries-copy">
          <span className="a3s-section-eyebrow">
            {labels.boundariesEyebrow}
          </span>
          <h2>{labels.boundariesTitle}</h2>
          <ul>
            {labels.boundaryItems.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
          <a href={route('/guide/architecture.html')}>
            {labels.boundaryLink}
            <ArrowIcon />
          </a>
        </div>
      </section>

      <section className="a3s-cta">
        <div>
          <span className="a3s-section-eyebrow">{labels.ctaEyebrow}</span>
          <h2>{labels.ctaTitle}</h2>
          <p>{labels.ctaBody}</p>
        </div>
        <div className="a3s-cta-actions">
          <a className="a3s-button a3s-button--primary" href={route('/guide/')}>
            <AnimatedButtonBorder />
            {labels.ctaPrimary}
            <ArrowIcon />
          </a>
          <a className="a3s-button a3s-button--secondary" href={route('/api/')}>
            {labels.ctaSecondary}
          </a>
        </div>
      </section>

      <footer className="a3s-home-footer">
        <a href={route('/')}>A3S Code</a>
        <span>{labels.footer}</span>
        <a href="https://github.com/A3S-Lab/Code">GitHub ↗</a>
      </footer>
    </main>
  );
}
