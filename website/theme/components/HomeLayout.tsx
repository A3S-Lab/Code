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
      zh: '治理每一次副作用',
      en: 'Govern every side effect',
    },
    body: {
      zh: '参数校验、能力声明、权限、人工确认、Hook、预算、安全提供者与取消，共享同一条工具调用路径。',
      en: 'Validation, capabilities, permissions, confirmation, hooks, budgets, security providers, and cancellation share one invocation path.',
    },
    tags: ['policy', 'HITL', 'sandbox'],
  },
  {
    index: '02',
    title: {
      zh: '让上下文保持有界',
      en: 'Keep context bounded',
    },
    body: {
      zh: '读取、搜索、命令输出、Git 与网页证据使用范围或游标；大型结果进入带预览、大小与哈希的有界 Artifact。',
      en: 'Reads, searches, command output, Git, and web evidence use ranges or cursors. Large results become bounded artifacts with previews, sizes, and hashes.',
    },
    tags: ['cursor', 'artifact', 'hash'],
  },
  {
    index: '03',
    title: {
      zh: '拥有自己的 UI',
      en: 'Own the UI',
    },
    body: {
      zh: 'Core 发出 AgentEvent；SDK 流与持久化 Run 使用无损 EventEnvelopeV1。宿主可以选择呈现方式，而不必分叉 Agent Loop。',
      en: 'Core emits AgentEvent while SDK streams and persisted runs use lossless EventEnvelopeV1. Hosts choose presentation without forking the agent loop.',
    },
    tags: ['AgentEvent', 'EventEnvelopeV1'],
  },
  {
    index: '04',
    title: {
      zh: '从证据恢复',
      en: 'Resume from evidence',
    },
    body: {
      zh: 'SessionSnapshotV1 可以把会话、Run、Artifact、Trace、验证报告与子任务记录原子提交为一个 Generation。',
      en: 'SessionSnapshotV1 can atomically commit sessions, runs, artifacts, traces, verification reports, and child-task records as one generation.',
    },
    tags: ['snapshot', 'replay', 'verification'],
  },
];

const capabilityCards = [
  {
    className: 'a3s-bento-card--wide a3s-bento-card--policy',
    eyebrow: { zh: '调用内核', en: 'INVOCATION KERNEL' },
    title: {
      zh: '工具可用，不等于工具越权',
      en: 'Available never means ungoverned',
    },
    body: {
      zh: '文件、搜索、Shell、Git、Web、Batch、QuickJS、结构化生成与委派，都在 Workspace 能力和宿主策略允许时才暴露。',
      en: 'Files, search, shell, Git, web, batch, QuickJS, structured generation, and delegation are exposed only when workspace capability and host policy allow.',
    },
    tags: ['files', 'shell', 'git', 'web', 'program', 'task'],
  },
  {
    className: 'a3s-bento-card--models',
    eyebrow: { zh: '模型适配', en: 'MODEL ADAPTERS' },
    title: {
      zh: '统一生命周期，不锁定提供商',
      en: 'One lifecycle, no provider lock-in',
    },
    body: {
      zh: 'Anthropic、智谱、OpenAI-compatible API，或宿主注入的 LlmClient。',
      en: 'Anthropic, Zhipu, OpenAI-compatible APIs, or a host-injected LlmClient.',
    },
    tags: ['streaming', 'tools', 'structured output'],
  },
  {
    className: 'a3s-bento-card--state',
    eyebrow: { zh: '持久状态', en: 'DURABLE STATE' },
    title: {
      zh: 'Run、Trace、Artifact 与 Snapshot',
      en: 'Runs, traces, artifacts, and snapshots',
    },
    body: {
      zh: '原子快照、事件回放、验证证据、Checkpoint，以及可选的 State Graph / Flow 投影。',
      en: 'Atomic snapshots, event replay, verification evidence, checkpoints, and optional State Graph / Flow projection.',
    },
    tags: ['atomic', 'replayable', 'auditable'],
  },
  {
    className: 'a3s-bento-card--extend',
    eyebrow: { zh: '扩展边界', en: 'EXTENSION BOUNDARIES' },
    title: {
      zh: '用显式契约扩展运行时',
      en: 'Extend through explicit contracts',
    },
    body: {
      zh: 'MCP、Skills、ContextProvider、MemoryStore、SessionStore、自定义工具与 Workspace 服务保持可替换。',
      en: 'MCP, Skills, ContextProvider, MemoryStore, SessionStore, custom tools, and workspace services stay replaceable.',
    },
    tags: ['MCP', 'Skills', 'traits'],
  },
  {
    className: 'a3s-bento-card--wide a3s-bento-card--workspace',
    eyebrow: { zh: '工作区', en: 'WORKSPACE' },
    title: {
      zh: '代码智能与工具遵守同一个 Workspace',
      en: 'Code intelligence and tools share one workspace boundary',
    },
    body: {
      zh: '符号、定义、引用、实现、诊断与修订信息由宿主选择的 Workspace 提供；不具备本地能力的后端不会向模型声明本地 Bash 或 Git。',
      en: 'Symbols, definitions, references, implementations, diagnostics, and revisions come from the host-selected workspace. A backend without local capability never advertises local Bash or Git.',
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
      zh: '现成的交互式编码产品，渲染推理、工具、审批、任务进度与 Diff。',
      en: 'The ready interactive coding product for reasoning, tools, approvals, task progress, and diffs.',
    },
    command: 'brew install A3S-Lab/tap/a3s',
  },
  {
    key: 'rust',
    name: 'Rust',
    packageName: 'a3s-code-core',
    href: 'https://crates.io/crates/a3s-code-core',
    description: {
      zh: '完整的异步运行时 API 与公共扩展 Trait。',
      en: 'The complete async runtime API and public extension traits.',
    },
    command: 'cargo add a3s-code-core',
  },
  {
    key: 'node',
    name: 'Node.js',
    packageName: '@a3s-lab/code',
    href: 'https://www.npmjs.com/package/@a3s-lab/code',
    description: {
      zh: '基于 N-API 的原生绑定，覆盖生命周期、事件流、工具、Store、编排与 MCP。',
      en: 'Native N-API bindings for lifecycle, streams, tools, stores, orchestration, and MCP.',
    },
    command: 'npm install @a3s-lab/code',
  },
  {
    key: 'python',
    name: 'Python',
    packageName: 'a3s-code',
    href: 'https://pypi.org/project/a3s-code/',
    description: {
      zh: '基于 PyO3 的原生包，提供同步与异步应用 API。',
      en: 'A native PyO3 package with synchronous and asynchronous application APIs.',
    },
    command: 'python -m pip install a3s-code',
  },
];

const runtimeLayers = [
  {
    id: 'surfaces',
    code: 'L01 / SURFACES',
    title: { zh: '产品入口', en: 'Product surfaces' },
    body: {
      zh: 'Terminal、Rust、Node.js 与 Python 进入同一套执行语义；呈现方式可以不同，Runtime Contract 保持一致。',
      en: 'Terminal, Rust, Node.js, and Python enter the same execution semantics. Presentation varies while the runtime contract stays consistent.',
    },
    tags: ['a3s code', 'Rust', 'Node.js', 'Python'],
  },
  {
    id: 'session',
    code: 'L02 / AGENT API',
    title: { zh: 'Agent 与 Session', en: 'Agent and session' },
    body: {
      zh: 'Agent 持有解析后的配置与共享能力；AgentSession 把它们绑定到一个 Workspace 和一段对话生命周期。',
      en: 'Agent owns resolved configuration and shared capabilities. AgentSession binds them to one workspace and conversation lifecycle.',
    },
    tags: ['Agent', 'AgentSession', 'lifecycle'],
  },
  {
    id: 'context',
    code: 'L03 / INTELLIGENCE',
    title: { zh: '上下文、记忆与模型', en: 'Context, memory, and models' },
    body: {
      zh: 'ContextAssembler 排序并预算输入；Memory 保留可复用事实；模型适配器统一流、工具调用、结构化输出与取消。',
      en: 'ContextAssembler ranks and budgets inputs, memory retains reusable facts, and model adapters normalize streaming, tool calls, structured output, and cancellation.',
    },
    tags: ['ContextAssembler', 'Memory', 'LlmClient'],
  },
  {
    id: 'governance',
    code: 'L04 / GOVERNANCE',
    title: { zh: '治理内核', en: 'Governance kernel' },
    body: {
      zh: '每个副作用依次经过参数校验、能力检查、权限、人工确认、预算、安全提供者、沙箱与取消边界。',
      en: 'Every side effect crosses argument validation, capability checks, permissions, human confirmation, budgets, security providers, sandboxing, and cancellation.',
    },
    tags: ['validate', 'permission', 'confirm', 'budget'],
  },
  {
    id: 'tools',
    code: 'L05 / WORKSPACE',
    title: { zh: 'Workspace 与工具', en: 'Workspace and tools' },
    body: {
      zh: '文件、搜索、Shell、Git、Web、代码智能、MCP、Skills 与委派只在 Workspace 能力和策略共同允许时注册。',
      en: 'Files, search, shell, Git, web, code intelligence, MCP, Skills, and delegation register only when workspace capability and policy both allow.',
    },
    tags: ['files', 'git', 'web', 'MCP', 'Skills'],
  },
  {
    id: 'evidence',
    code: 'L06 / DURABILITY',
    title: { zh: '事件与持久证据', en: 'Events and durable evidence' },
    body: {
      zh: 'AgentEvent 与 EventEnvelopeV1 向产品公开生命周期；Run、Trace、Artifact、验证报告和 SessionSnapshotV1 支持审计与恢复。',
      en: 'AgentEvent and EventEnvelopeV1 expose lifecycle to products. Runs, traces, artifacts, verification reports, and SessionSnapshotV1 support audit and recovery.',
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
    eyebrow: 'OPEN SOURCE · ASYNC RUST RUNTIME',
    titleLead: '构建可治理的',
    titleAccent: '编码 Agent',
    subtitle:
      'A3S Code 把 Agent Loop、Workspace 工具、模型适配、策略决策、版本化事件与持久化证据放在显式契约之后。',
    docs: '开始使用',
    github: '查看 GitHub',
    copy: '复制',
    copied: '已复制',
    turn: '一次受治理的 Turn',
    proposal: '模型提出工具调用',
    governed: '统一治理边界',
    result: '结果成为事件与证据',
    context: '上下文 + 记忆',
    model: '模型适配器',
    guard: '校验 → 权限 → 确认 → 预算 → 沙箱',
    evidence: 'Run · Trace · Artifact · Snapshot',
    surfacesLabel: '一个运行时，四种产品表面',
    whyEyebrow: 'WHY A3S CODE',
    whyTitle: '可组合，也可问责。',
    whyBody:
      '运行时把执行语义放在一个可观察、可替换、可持久化的边界里；UI、身份、凭据与部署策略仍由宿主拥有。',
    architectureEyebrow: 'VISIBLE BY DESIGN',
    architectureTitle: '责任链不是黑盒。',
    architectureBody:
      'AgentSession 绑定 Workspace 与会话；模型只提出调用。每个副作用经过同一条治理路径，再以版本化事件和耐久证据向宿主公开。',
    architectureAlt:
      'A3S Code 的受治理 Agent 运行时架构：模型、策略、工具、事件与持久化快照之间的显式流转。',
    capabilitiesEyebrow: 'RUNTIME CAPABILITIES',
    capabilitiesTitle: '能力丰富，授权明确。',
    capabilitiesBody:
      'Core 默认保持可嵌入。云存储、服务端和遥测是可选能力；自动压缩、目标、委派、沙箱、持久化与图投影都需要宿主显式配置。',
    surfacesEyebrow: 'CHOOSE YOUR SURFACE',
    surfacesTitle: '同一套执行语义，进入你的技术栈。',
    surfacesBody:
      '直接运行终端产品，或通过 Rust、Node.js、Python 把同一个 Runtime 嵌入自己的 IDE、Runner、服务与产品界面。',
    boundariesEyebrow: 'EXPLICIT BOUNDARIES',
    boundariesTitle: 'Core 管执行，宿主管信任。',
    boundaryItems: [
      'Core 是可嵌入运行时，不是托管 Agent 服务，也不是终端组件库。',
      '独立的 A3S CLI 负责交互式 TUI、账户适配与呈现策略。',
      '身份、凭据、部署和直接宿主工具的信任决策始终属于宿主。',
    ],
    boundaryLink: '阅读架构与边界',
    ctaTitle: '从一个可观察的 Turn 开始。',
    ctaBody:
      '运行 a3s code，或把 a3s-code-core 嵌入你的产品。工具、策略、事件和证据从第一天起就是显式的。',
    ctaPrimary: '阅读快速开始',
    ctaSecondary: '查看 API 契约',
    footer:
      'MIT licensed. Built in Rust. Designed for governed agent products.',
  },
  en: {
    eyebrow: 'OPEN SOURCE · ASYNC RUST RUNTIME',
    titleLead: 'Build governed',
    titleAccent: 'coding agents',
    subtitle:
      'A3S Code keeps the agent loop, workspace tools, model adapters, policy decisions, versioned events, and durable evidence behind explicit contracts.',
    docs: 'Get started',
    github: 'View on GitHub',
    copy: 'Copy',
    copied: 'Copied',
    turn: 'One governed turn',
    proposal: 'The model proposes a tool call',
    governed: 'One governance boundary',
    result: 'Results become events and evidence',
    context: 'context + memory',
    model: 'model adapter',
    guard: 'validation → permission → confirmation → budget → sandbox',
    evidence: 'Run · Trace · Artifact · Snapshot',
    surfacesLabel: 'One runtime, four product surfaces',
    whyEyebrow: 'WHY A3S CODE',
    whyTitle: 'Composable and accountable.',
    whyBody:
      'The runtime puts execution semantics behind one observable, replaceable, durable boundary. The host still owns UI, identity, credentials, and deployment policy.',
    architectureEyebrow: 'VISIBLE BY DESIGN',
    architectureTitle: 'The chain of responsibility is not a black box.',
    architectureBody:
      'AgentSession binds a workspace and conversation; the model only proposes calls. Every side effect crosses the same governance path, then returns as versioned events and durable evidence.',
    architectureAlt:
      'A3S Code governed agent runtime architecture showing the explicit flow between model, policy, tools, events, and durable snapshots.',
    capabilitiesEyebrow: 'RUNTIME CAPABILITIES',
    capabilitiesTitle: 'Rich capability. Explicit authority.',
    capabilitiesBody:
      'Core stays embeddable by default. Cloud storage, serving, and telemetry are opt-in; compaction, goals, delegation, sandboxing, persistence, and graph projection require host configuration.',
    surfacesEyebrow: 'CHOOSE YOUR SURFACE',
    surfacesTitle: 'One execution model, in your stack.',
    surfacesBody:
      'Run the terminal product or embed the same runtime in an IDE, runner, service, or product UI through Rust, Node.js, and Python.',
    boundariesEyebrow: 'EXPLICIT BOUNDARIES',
    boundariesTitle: 'Core owns execution. The host owns trust.',
    boundaryItems: [
      'Core is an embeddable runtime, not a hosted agent service or terminal widget library.',
      'The separate A3S CLI owns the interactive TUI, account adapters, and presentation policy.',
      'Identity, credentials, deployment, and trust decisions for direct host tools remain host-owned.',
    ],
    boundaryLink: 'Read architecture and boundaries',
    ctaTitle: 'Start with one observable turn.',
    ctaBody:
      'Run a3s code or embed a3s-code-core. Tools, policy, events, and evidence are explicit from day one.',
    ctaPrimary: 'Read the quick start',
    ctaSecondary: 'Explore the API contract',
    footer:
      'MIT licensed. Built in Rust. Designed for governed agent products.',
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
          <strong>evidence</strong>
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

function LayeredRuntime({ locale }: { locale: Locale }) {
  const [activeId, setActiveId] = useState('governance');
  const activeLayer =
    runtimeLayers.find((layer) => layer.id === activeId) ?? runtimeLayers[3];

  return (
    <div
      className="a3s-layered-runtime"
      onMouseLeave={() => setActiveId('governance')}
    >
      <div className="a3s-stack-toolbar">
        <span>RUNTIME STACK / EXPLODED VIEW</span>
        <span>
          <i /> {locale === 'zh' ? '移入查看职责' : 'HOVER TO INSPECT'}
        </span>
      </div>
      <div className="a3s-stack-stage">
        <div className="a3s-stack-axis" aria-hidden="true">
          <span>PRODUCT</span>
          <i />
          <span>EVIDENCE</span>
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
          <LayeredRuntime locale={locale} />
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
            <span>CORE</span>
            <strong>EXECUTION</strong>
          </div>
          <div className="a3s-boundary-line">
            <span>explicit contract</span>
          </div>
          <div className="a3s-boundary-host">
            <span>HOST</span>
            <strong>TRUST</strong>
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
          <span className="a3s-section-eyebrow">START BUILDING</span>
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
            href={route('/guide/api-contract.html')}
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
