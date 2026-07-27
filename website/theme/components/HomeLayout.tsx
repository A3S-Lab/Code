import { useState } from 'react';
import type { CSSProperties } from 'react';
import { useLang, useSite, useVersion, withBase } from '@rspress/core/runtime';

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

const installCommands = [
  {
    id: 'terminal',
    label: 'Terminal',
    command: 'brew install A3S-Lab/tap/a3s\ncd /path/to/project\na3s code',
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
      zh: '每次工具调用都先过检查',
      en: 'Check every tool call',
    },
    body: {
      zh: '修改文件、运行命令、操作 Git 和访问外部服务都会走同一套流程：校验参数、检查能力和权限，必要时先询问用户，再执行。',
      en: 'File changes, shell commands, Git operations, and external requests use one path: validate the arguments, check capabilities and permissions, ask the user when needed, then run.',
    },
    tags: ['policy', 'HITL', 'sandbox'],
  },
  {
    index: '02',
    title: {
      zh: '大结果不会塞满上下文',
      en: 'Keep large results out of the prompt',
    },
    body: {
      zh: '文件读取、搜索、命令输出、Git 结果和网页内容都支持范围或游标。内容过大时会保存为 Artifact，模型只接收预览、大小和哈希。',
      en: 'File reads, searches, command output, Git results, and web pages support ranges or cursors. Oversized results become artifacts, while the model gets a preview, size, and hash.',
    },
    tags: ['cursor', 'artifact', 'hash'],
  },
  {
    index: '03',
    title: {
      zh: '界面由你的产品来做',
      en: 'Build the UI you want',
    },
    body: {
      zh: 'Core 在执行过程中持续发出 AgentEvent，SDK 和持久化 Run 使用 EventEnvelopeV1。终端、IDE 或网页都能复用同一个 Agent Loop。',
      en: 'Core emits AgentEvent throughout a run. SDK streams and persisted runs use EventEnvelopeV1, so a terminal, IDE, or web UI can render the same loop.',
    },
    tags: ['AgentEvent', 'EventEnvelopeV1'],
  },
  {
    index: '04',
    title: {
      zh: '任务中断后可以接着跑',
      en: 'Resume without guessing',
    },
    body: {
      zh: 'SessionSnapshotV1 会一起保存会话、Run、Artifact、Trace、验证结果和子任务记录，恢复时不用让模型猜之前做过什么。',
      en: 'SessionSnapshotV1 saves the session, runs, artifacts, traces, verification results, and child-task records together, so interrupted work can continue from saved state.',
    },
    tags: ['snapshot', 'replay', 'verification'],
  },
];

const capabilityCards = [
  {
    className: 'a3s-bento-card--wide a3s-bento-card--policy',
    eyebrow: { zh: '工具调用', en: 'TOOL CALLS' },
    title: {
      zh: '哪些工具能用，由代码和配置决定',
      en: 'Code and config decide which tools are available',
    },
    body: {
      zh: '文件、搜索、Shell、Git、Web、Batch、QuickJS、结构化输出和子任务，只有在 Workspace 支持且权限允许时才会开放给模型。',
      en: 'Files, search, shell, Git, web, batch, QuickJS, structured output, and child tasks are exposed only when the workspace supports them and policy allows them.',
    },
    tags: ['files', 'shell', 'git', 'web', 'program', 'task'],
  },
  {
    className: 'a3s-bento-card--models',
    eyebrow: { zh: '模型', en: 'MODELS' },
    title: {
      zh: '换模型，不用重写 Agent Loop',
      en: 'Switch models without rewriting the loop',
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
      zh: '保存运行记录，也能恢复现场',
      en: 'Save the run and pick up where it stopped',
    },
    body: {
      zh: '一次任务可以保存 Snapshot、事件、Trace、Artifact、验证结果和 Checkpoint；需要时再接 State Graph 或 Flow。',
      en: 'Save snapshots, events, traces, artifacts, verification results, and checkpoints. Add State Graph or Flow only when you need them.',
    },
    tags: ['atomic', 'replayable', 'auditable'],
  },
  {
    className: 'a3s-bento-card--extend',
    eyebrow: { zh: '扩展', en: 'EXTENSIONS' },
    title: {
      zh: '接入自己的工具和存储',
      en: 'Bring your own tools and storage',
    },
    body: {
      zh: 'MCP、Skills、ContextProvider、MemoryStore、SessionStore、Workspace 服务和自定义工具都可以替换或扩展。',
      en: 'Replace or extend MCP, Skills, ContextProvider, MemoryStore, SessionStore, workspace services, and custom tools.',
    },
    tags: ['MCP', 'Skills', 'traits'],
  },
  {
    className: 'a3s-bento-card--wide a3s-bento-card--workspace',
    eyebrow: { zh: '工作区', en: 'WORKSPACE' },
    title: {
      zh: '模型只会看到当前 Workspace 能做的事',
      en: 'The model sees only what the workspace can do',
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
    href: 'https://github.com/A3S-Lab/CLI',
    description: {
      zh: '开箱即用的终端界面，可以查看推理、工具调用、确认提示、任务进度和 Diff。',
      en: 'A ready-to-run terminal UI for reasoning, tool calls, approval prompts, task progress, and diffs.',
    },
    command: 'brew install A3S-Lab/tap/a3s',
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

const copy = {
  zh: {
    eyebrow: 'OPEN SOURCE · RUST AGENT RUNTIME',
    titleLead: '把编码 Agent',
    titleAccent: '接进你的产品',
    subtitle:
      'A3S Code 是一个用 Rust 写的 Agent 运行时。它负责 Agent Loop、工具调用、权限确认、事件流和任务恢复，并提供 Rust、Node.js、Python API。',
    docs: '查看文档',
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
    whyTitle: 'Agent 真正动手之前，先把规则定清楚。',
    whyBody:
      '模型可以读写文件、运行命令和操作 Git，但每次调用仍会先检查参数和权限。你的应用决定给它哪些工具，也能拿到完整的执行记录。',
    architectureEyebrow: 'HOW IT RUNS',
    architectureTitle: '一层一层看清 Agent 在做什么。',
    architectureBody:
      '从产品入口到 AgentSession、模型、权限检查、Workspace 工具和运行记录，每一层只负责一件事。把鼠标移到分层图上，可以查看它们各自的作用。',
    architectureAlt:
      'A3S Code 运行时分层图，展示接入方式、AgentSession、上下文、权限检查、工具与运行记录。',
    capabilitiesEyebrow: 'WHAT YOU GET',
    capabilitiesTitle: '需要什么，就打开什么。',
    capabilitiesBody:
      'Core 默认只提供可嵌入的基础运行时。云存储、服务端、遥测、自动压缩、子任务、沙箱和持久化，都由你的应用按需启用。',
    surfacesEyebrow: 'USE IT YOUR WAY',
    surfacesTitle: '想直接用，或接进自己的应用，都可以。',
    surfacesBody:
      '终端版可以立即运行；Rust crate、Node.js 和 Python 包提供同一套 Runtime，适合 IDE、Runner、服务端和自己的界面。',
    boundariesEyebrow: 'WHAT STAYS YOURS',
    boundariesTitle: '执行交给 Runtime，账号和权限留在你的应用里。',
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
    stackTitle: 'A3S CODE / 分层图',
    stackHint: '移入查看这一层',
    stackTop: '产品',
    stackBottom: '记录',
    ctaEyebrow: 'TRY IT',
    ctaTitle: '先在一个项目里跑起来。',
    ctaBody:
      '安装 a3s code 直接体验，或者选择 Rust、Node.js、Python 包接入自己的产品。',
    ctaPrimary: '查看快速开始',
    ctaSecondary: '打开 Playground',
    footer: 'MIT 开源 · Rust 编写 · 支持 Terminal / Rust / Node.js / Python',
  },
  en: {
    eyebrow: 'OPEN SOURCE · RUST AGENT RUNTIME',
    titleLead: 'Add a coding agent',
    titleAccent: 'to your product',
    subtitle:
      'A3S Code is a Rust agent runtime. It handles the agent loop, tool calls, approval, event streaming, and recovery, with APIs for Rust, Node.js, and Python.',
    docs: 'Read the docs',
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
    whyTitle: 'Set the rules before the agent changes anything.',
    whyBody:
      'The model can edit files, run commands, and operate Git, but every call is checked first. Your application decides which tools it gets and receives the full execution stream.',
    architectureEyebrow: 'HOW IT RUNS',
    architectureTitle: 'See what each part of the runtime does.',
    architectureBody:
      'The stack runs from product entry points through AgentSession, models, permission checks, workspace tools, and run records. Hover a layer to inspect its job.',
    architectureAlt:
      'A3S Code runtime layers showing entry points, AgentSession, context, permission checks, tools, and run records.',
    capabilitiesEyebrow: 'WHAT YOU GET',
    capabilitiesTitle: 'Turn on only what your product needs.',
    capabilitiesBody:
      'Core starts as an embeddable runtime. Cloud storage, serving, telemetry, compaction, child tasks, sandboxing, and persistence are enabled by your application when needed.',
    surfacesEyebrow: 'USE IT YOUR WAY',
    surfacesTitle: 'Run it in a terminal or embed it in your app.',
    surfacesBody:
      'The terminal app is ready to use. The Rust crate, Node.js package, and Python package bring the same runtime to an IDE, runner, server, or custom UI.',
    boundariesEyebrow: 'WHAT STAYS YOURS',
    boundariesTitle: 'The runtime executes. Your app controls access.',
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
    stackTitle: 'A3S CODE / RUNTIME LAYERS',
    stackHint: 'HOVER A LAYER',
    stackTop: 'PRODUCT',
    stackBottom: 'RECORDS',
    ctaEyebrow: 'TRY IT',
    ctaTitle: 'Try it in a real repository.',
    ctaBody:
      'Install a3s code to start immediately, or choose the Rust, Node.js, or Python package for your own product.',
    ctaPrimary: 'Open the quick start',
    ctaSecondary: 'Open the playground',
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
    useState<(typeof installCommands)[number]['id']>('terminal');
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

function RuntimeDiagram({ labels }: { labels: (typeof copy)[Locale] }) {
  return (
    <div className="a3s-runtime-visual" aria-label={labels.turn}>
      <div className="a3s-runtime-grid" aria-hidden="true" />
      <div className="a3s-runtime-heading">
        <span className="a3s-runtime-status" />
        {labels.turn}
        <span>EVENT 0001</span>
      </div>
      <div className="a3s-runtime-surfaces">
        <span>Terminal</span>
        <span>Rust</span>
        <span>Node.js</span>
        <span>Python</span>
      </div>
      <div className="a3s-runtime-connector" />
      <div className="a3s-runtime-session">
        <span>AgentSession</span>
        <small>{labels.context}</small>
      </div>
      <div className="a3s-runtime-arrow">
        <span>{labels.proposal}</span>
      </div>
      <div className="a3s-runtime-model">{labels.model}</div>
      <div className="a3s-runtime-guard">
        <span>{labels.governed}</span>
        <code>{labels.guard}</code>
      </div>
      <div className="a3s-runtime-arrow a3s-runtime-arrow--result">
        <span>{labels.result}</span>
      </div>
      <div className="a3s-runtime-events">
        <div>
          <strong>AgentEvent</strong>
          <small>EventEnvelopeV1</small>
        </div>
        <div>
          <strong>{labels.record}</strong>
          <small>{labels.evidence}</small>
        </div>
      </div>
      <span className="a3s-runtime-corner a3s-runtime-corner--top" />
      <span className="a3s-runtime-corner a3s-runtime-corner--bottom" />
    </div>
  );
}

type RuntimeLayerStyle = CSSProperties & {
  '--layer-index': number;
};

function LayeredRuntime({
  locale,
  labels,
}: {
  locale: Locale;
  labels: (typeof copy)[Locale];
}) {
  const [activeId, setActiveId] = useState('governance');
  const activeLayer =
    runtimeLayers.find((layer) => layer.id === activeId) ?? runtimeLayers[3];

  return (
    <div
      className="a3s-layered-runtime"
      onMouseLeave={() => setActiveId('governance')}
    >
      <div className="a3s-stack-toolbar">
        <span>{labels.stackTitle}</span>
        <span>
          <i /> {labels.stackHint}
        </span>
      </div>
      <div className="a3s-stack-stage">
        <div className="a3s-stack-axis" aria-hidden="true">
          <span>{labels.stackTop}</span>
          <i />
          <span>{labels.stackBottom}</span>
        </div>
        {runtimeLayers.map((layer, index) => (
          <button
            aria-label={`${localeValue(layer.title, locale)}: ${localeValue(
              layer.body,
              locale,
            )}`}
            className={`a3s-stack-layer ${
              activeLayer.id === layer.id ? 'is-active' : ''
            }`}
            key={layer.id}
            onFocus={() => setActiveId(layer.id)}
            onMouseEnter={() => setActiveId(layer.id)}
            style={{ '--layer-index': index } as RuntimeLayerStyle}
            type="button"
          >
            <span className="a3s-stack-layer-face">
              <small>{layer.code}</small>
              <strong>{localeValue(layer.title, locale)}</strong>
              <i />
            </span>
          </button>
        ))}
        {runtimeLayers.map((layer, index) => (
          <div
            aria-hidden="true"
            className={`a3s-stack-label ${
              index % 2 === 0
                ? 'a3s-stack-label--left'
                : 'a3s-stack-label--right'
            } ${activeLayer.id === layer.id ? 'is-active' : ''}`}
            key={layer.code}
            onMouseEnter={() => setActiveId(layer.id)}
            style={{ '--layer-index': index } as RuntimeLayerStyle}
          >
            <span>{layer.code}</span>
            <i />
          </div>
        ))}
      </div>
      <div className="a3s-stack-detail" aria-live="polite">
        <div className="a3s-stack-detail-index">
          {String(runtimeLayers.indexOf(activeLayer) + 1).padStart(2, '0')}
        </div>
        <div>
          <span>{activeLayer.code}</span>
          <h3>{localeValue(activeLayer.title, locale)}</h3>
          <p>{localeValue(activeLayer.body, locale)}</p>
        </div>
        <div className="a3s-stack-detail-tags">
          {activeLayer.tags.map((tag) => (
            <span key={tag}>{tag}</span>
          ))}
        </div>
      </div>
    </div>
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
          <RuntimeDiagram labels={labels} />
        </div>
      </section>

      <section className="a3s-surface-strip" aria-label={labels.surfacesLabel}>
        <div className="a3s-surface-strip-label">
          <span>{labels.surfacesLabel}</span>
        </div>
        {surfaces.map((surface) => (
          <a href={surface.href} key={surface.key}>
            <span>{surface.name}</span>
            <strong>{surface.packageName}</strong>
            <ArrowIcon />
          </a>
        ))}
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

      <section className="a3s-section a3s-architecture" id="runtime-stack">
        <div className="a3s-architecture-copy">
          <span className="a3s-section-eyebrow">
            {labels.architectureEyebrow}
          </span>
          <h2>{labels.architectureTitle}</h2>
          <p>{labels.architectureBody}</p>
          <a href={route('/guide/architecture.html')}>
            {labels.boundaryLink}
            <ArrowIcon />
          </a>
        </div>
        <div className="a3s-architecture-frame">
          <LayeredRuntime labels={labels} locale={locale} />
        </div>
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
            {labels.ctaPrimary}
            <ArrowIcon />
          </a>
          <a
            className="a3s-button a3s-button--secondary"
            href={route('/guide/playground/')}
          >
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
