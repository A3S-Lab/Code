import { useState } from 'react';
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
import { CanvasGridEffect } from './CanvasGridEffect';
import {
  CapabilityShowcase,
  CapabilityStoriesMarkdown,
} from './CapabilityShowcase';
import { InstallSwitcher } from './InstallSwitcher';
import { PremiumInteractions } from './PremiumInteractions';
import runtimeTutorialData from '../generated/runtime-tutorial.json';
import { copy } from './home-copy';
import { RuntimeExecutionFlow } from './TuiRuntimeDemo';

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

const governanceFeatures: Feature[] = [
  {
    index: '01',
    title: {
      zh: '统一检查文件、Shell、Git 与外部请求',
      en: 'Check files, shell, Git, and external requests',
    },
    body: {
      zh: '模型或前置钩子提交工具参数后，Runtime 会再次校验 Schema，再检查 Workspace 能力和权限规则。需要用户确认的调用会先暂停。',
      en: 'After the model or a pre-hook supplies tool arguments, the runtime validates the schema again, then checks workspace capability and permission policy. Calls that need approval pause.',
    },
    tags: ['hooks', 'policy', 'HITL', 'sandbox'],
  },
  {
    index: '02',
    title: {
      zh: '确定性投影大输出，保留完整证据',
      en: 'Project large output with verifiable evidence',
    },
    body: {
      zh: '固定策略可以保留头尾、折叠重复行并采样 JSON。模型收到投影内容；应用同时得到字节、哈希和损失证据，Artifact 保留完整原文。',
      en: 'A pinned policy can retain head and tail, fold repeated lines, and sample JSON. The model receives projected content; the app gets byte, hash, and loss evidence while artifacts keep the original.',
    },
    tags: ['transform', 'evidence', 'artifact'],
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
    eyebrow: { zh: '工作区检索', en: 'WORKSPACE RETRIEVAL' },
    title: {
      zh: '异步构建，会话关闭时完整释放',
      en: 'Build asynchronously; release with the session',
    },
    body: {
      zh: '增量 BM25、可选宿主 Embedding、内存向量分区和 Hybrid RRF 共用一个有界文本目录；不需要向量数据库，非文本文件不会进入切块或向量化。',
      en: 'Incremental BM25, optional host embeddings, in-memory vector partitions, and hybrid RRF share one bounded text catalog. No vector database is required, and non-text files never enter chunking or embeddings.',
    },
    tags: ['BM25', 'semantic', 'RRF', 'CPU rerank'],
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
  {
    key: 'go',
    name: 'Go',
    packageName: 'sdk/go/v7',
    href: 'https://pkg.go.dev/github.com/A3S-Lab/Code/sdk/go/v7',
    description: {
      zh: '纯 Go API 通过长驻桥接进程提供会话、事件流、工具、验证和 MCP，无需 CGO。',
      en: 'A pure-Go API for sessions, event streams, tools, verification, and MCP through a long-lived bridge, without CGO.',
    },
    command: 'go get github.com/A3S-Lab/Code/sdk/go/v7',
  },
];

const runtimeLayers = [
  {
    id: 'surfaces',
    code: 'L01 / SURFACES',
    title: { zh: '接入方式', en: 'Ways to use it' },
    body: {
      zh: '同一套 Runtime 可以直接跑在终端里，也可以通过 Rust、Node.js、Python 或 Go 接进你的应用。接口不同，执行流程一致。',
      en: 'Run the same runtime in a terminal or embed it through Rust, Node.js, Python, or Go. The APIs differ; the execution flow stays the same.',
    },
    tags: ['a3s code', 'Rust', 'Node.js', 'Python', 'Go'],
  },
  {
    id: 'session',
    code: 'L02 / AGENT API',
    title: { zh: 'Agent 与 Session', en: 'Agent and session' },
    body: {
      zh: 'Agent 读取配置并准备共享能力与优先级调度器；AgentSession 把它们连接到一个项目目录和一段对话。',
      en: 'Agent loads configuration, shared capabilities, and the priority scheduler. AgentSession connects them to one project workspace and one conversation.',
    },
    tags: ['Agent', 'AgentSession', 'priority'],
  },
  {
    id: 'context',
    code: 'L03 / INTELLIGENCE',
    title: { zh: '上下文、记忆与模型', en: 'Context, memory, and models' },
    body: {
      zh: 'ContextAssembler 控制输入；会话目录异步提供 BM25 与可选语义检索；Memory 保存可复用信息；模型适配器负责流式输出、工具调用和取消。',
      en: 'ContextAssembler bounds input; the session catalog serves BM25 and optional semantic retrieval asynchronously; memory keeps reusable information; model adapters handle streaming, tools, and cancellation.',
    },
    tags: ['ContextAssembler', 'BM25 / RRF', 'Memory', 'LlmClient'],
  },
  {
    id: 'governance',
    code: 'L04 / GOVERNANCE',
    title: { zh: '权限与执行检查', en: 'Permission and execution checks' },
    body: {
      zh: '工具真正执行前，Runtime 会运行门控钩子、重新校验改写参数，并检查能力和权限；再按配置进行用户确认、预算、沙箱或取消。',
      en: 'Before a tool runs, the runtime executes gating hooks, revalidates rewritten arguments, and checks capabilities and permissions before approval, budget, sandbox, or cancellation.',
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
      zh: 'AgentEvent 把执行过程交给界面；Run、Trace、Artifact、不可变 Git 补丁和 SessionSnapshotV1 用来排查、审计、合并与恢复。',
      en: 'AgentEvent feeds the execution stream to your UI. Runs, traces, artifacts, immutable Git patches, and SessionSnapshotV1 support debugging, audit, merge, and recovery.',
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
      <CapabilityStoriesMarkdown locale={locale} />
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
      <PremiumInteractions />
      <div className="a3s-global-grid" aria-hidden="true">
        <CanvasGridEffect
          cellSize={54}
          className="a3s-global-grid-canvas"
          intensity={0.68}
          interactionScope="page"
        />
      </div>
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
          <InstallSwitcher labels={labels} />
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

      <CapabilityShowcase
        guideHref={route('/guide/tui.html')}
        locale={locale}
      />

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
