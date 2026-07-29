import { useEffect, useRef, useState } from 'react';

type Locale = 'zh' | 'en';

type Localized = {
  zh: string;
  en: string;
};

type CapabilityKey =
  'hitl' | 'progressive' | 'runtime' | 'intelligence' | 'ctx';

type CapabilityStory = {
  key: CapabilityKey;
  index: string;
  eyebrow: string;
  title: Localized;
  body: Localized;
  availability: Localized;
  tags: string[];
  stages: Localized[];
};

const sectionCopy = {
  zh: {
    eyebrow: 'A3S CODE / DISTINCTIVE CAPABILITIES',
    title: '五个关键瞬间，看懂一次任务如何安全地变聪明',
    body: '从执行前确认到代码语义，从按需发现平台能力到找回过去会话：选择一个场景，查看真实的交互顺序与边界。',
    guide: '查看完整 TUI 指南',
    select: '选择能力演示',
    live: 'LIVE CAPABILITY',
    play: '播放',
    pause: '暂停',
    replay: '重播',
    step: '阶段',
  },
  en: {
    eyebrow: 'A3S CODE / DISTINCTIVE CAPABILITIES',
    title: 'Five decisive moments that make a coding run safer and smarter',
    body: 'From approval before execution to code semantics, on-demand platform discovery, and past-session recall: select a capability to see its real interaction order and boundaries.',
    guide: 'Read the complete TUI guide',
    select: 'Select a capability demo',
    live: 'LIVE CAPABILITY',
    play: 'Play',
    pause: 'Pause',
    replay: 'Replay',
    step: 'Stage',
  },
};

export const capabilityStories: CapabilityStory[] = [
  {
    key: 'hitl',
    index: '01',
    eyebrow: 'HITL',
    title: {
      zh: '越过执行边界前，由你决定',
      en: 'You decide before a call crosses the boundary',
    },
    body: {
      zh: '需要确认的调用会在执行前暂停，并展示规范化参数。你可以只允许一次、保留精确的会话或项目授权，或者拒绝并说明原因。',
      en: 'Calls that need confirmation pause before execution and expose canonical arguments. Allow once, retain an exact session or project grant, or deny with a reason.',
    },
    availability: {
      zh: 'Default 模式 · 风险感知',
      en: 'Default mode · risk-aware',
    },
    tags: ['canonical args', 'exact grants', 'fail closed'],
    stages: [
      { zh: '识别边界', en: 'Classify boundary' },
      { zh: '暂停并展示', en: 'Pause and explain' },
      { zh: '选择授权范围', en: 'Choose grant scope' },
      { zh: '记录决定', en: 'Record the decision' },
    ],
  },
  {
    key: 'progressive',
    index: '02',
    eyebrow: 'PROGRESSIVE API',
    title: {
      zh: '先发现，再只加载需要的 Schema',
      en: 'Discover first, then load only the schema you need',
    },
    body: {
      zh: '登录 A3S OS 后，通过一个按权限过滤的入口执行 list → search → describe → execute。模型无需把整个平台手册塞进上下文。',
      en: 'After A3S OS login, one permission-filtered endpoint runs list → search → describe → execute. The model never needs the whole platform manual in context.',
    },
    availability: {
      zh: '登录后可用 · 权限过滤',
      en: 'Available after login · permission-filtered',
    },
    tags: ['list', 'search', 'describe', 'execute'],
    stages: [
      { zh: '列出模块', en: 'List modules' },
      { zh: '搜索操作', en: 'Search operations' },
      { zh: '读取单个 Schema', en: 'Describe one schema' },
      { zh: '执行并返回视图', en: 'Execute with a view' },
    ],
  },
  {
    key: 'runtime',
    index: '03',
    eyebrow: 'RUNTIME TOOL',
    title: {
      zh: '把独立输入交给远程 Worker 并行处理',
      en: 'Send independent inputs to a remote worker in parallel',
    },
    body: {
      zh: '登录后注册的 runtime 工具按 UUID 或名称解析 tool-kind Worker，提交 Function as a Service 批任务，流式呈现进度并聚合结果。',
      en: 'The login-gated runtime tool resolves a tool-kind worker by UUID or name, submits a Function-as-a-Service batch, streams progress, and aggregates results.',
    },
    availability: {
      zh: '登录后注册 · 批量执行',
      en: 'Registered after login · batch execution',
    },
    tags: ['worker resolve', 'batch', 'streaming'],
    stages: [
      { zh: '解析 Worker', en: 'Resolve worker' },
      { zh: '提交三个输入', en: 'Submit three inputs' },
      { zh: '流式追踪进度', en: 'Stream progress' },
      { zh: '聚合结果', en: 'Aggregate results' },
    ],
  },
  {
    key: 'intelligence',
    index: '04',
    eyebrow: 'CODE INTELLIGENCE',
    title: {
      zh: 'Agent、TUI 与 Web 共享同一份代码语义',
      en: 'Agent, TUI, and Web share one semantic code runtime',
    },
    body: {
      zh: '基于已保存文件提供符号、定义、声明、引用、实现与诊断。Agent 工具、/ide 和 Monaco 使用同一运行时，脏缓冲区不会伪装成已发布语义。',
      en: 'Saved files provide symbols, definitions, declarations, references, implementations, and diagnostics. Agent tools, /ide, and Monaco share the runtime; dirty buffers never masquerade as published semantics.',
    },
    availability: {
      zh: 'Rust · TypeScript / JavaScript',
      en: 'Rust · TypeScript / JavaScript',
    },
    tags: ['saved files', 'navigation', 'diagnostics'],
    stages: [
      { zh: '启动语言服务', en: 'Start language service' },
      { zh: '定位符号', en: 'Resolve symbol' },
      { zh: '查找引用', en: 'Find references' },
      { zh: '合并诊断', en: 'Collect diagnostics' },
    ],
  },
  {
    key: 'ctx',
    index: '05',
    eyebrow: 'CTX RECALL',
    title: {
      zh: '从过去会话找回决定，而不是重新猜测',
      en: 'Recover decisions from past sessions instead of guessing again',
    },
    body: {
      zh: '本地 ctx 可用时，/ctx 会搜索跨工具、跨会话的索引。命中窗口可以一次性附加到下一条消息，也可以携带来源保存为长期记忆。',
      en: 'When the local ctx index is available, /ctx searches across tools and sessions. A hit can be attached once to the next turn or saved to long-term memory with provenance.',
    },
    availability: {
      zh: '本地索引 · 跨会话 · 可追溯',
      en: 'Local index · cross-session · traceable',
    },
    tags: ['one-shot attach', 'source=ctx', 'memory backlink'],
    stages: [
      { zh: '搜索历史', en: 'Search history' },
      { zh: '选择命中', en: 'Select a hit' },
      { zh: '一次性附加', en: 'Attach once' },
      { zh: '保存并保留来源', en: 'Save with provenance' },
    ],
  },
];

function value(localized: Localized, locale: Locale) {
  return localized[locale];
}

function CapabilityIcon({ story }: { story: CapabilityKey }) {
  const paths: Record<CapabilityKey, React.ReactNode> = {
    hitl: (
      <>
        <path d="M12 2.8 19 6v5.2c0 4.3-2.8 8.1-7 10-4.2-1.9-7-5.7-7-10V6l7-3.2Z" />
        <path d="M9.2 12.1 11 14l4-4.3" />
      </>
    ),
    progressive: (
      <>
        <path d="M4 6h5M4 12h10M4 18h16" />
        <path d="m7 3 3 3-3 3M12 9l3 3-3 3m5 0 3 3-3 3" />
      </>
    ),
    runtime: (
      <>
        <rect x="3" y="4" width="18" height="5" rx="1.5" />
        <rect x="3" y="15" width="5" height="5" rx="1.2" />
        <rect x="10" y="15" width="5" height="5" rx="1.2" />
        <rect x="17" y="15" width="4" height="5" rx="1.2" />
        <path d="M12 9v3M5.5 12h13M5.5 12v3m6.5-3v3m6.5-3v3" />
      </>
    ),
    intelligence: (
      <>
        <path d="m9 6-6 6 6 6m6-12 6 6-6 6M14 3l-4 18" />
      </>
    ),
    ctx: (
      <>
        <path d="M4 5.5h11a4 4 0 0 1 4 4v1.5" />
        <path d="m16 8 3 3 3-3M20 18.5H9a4 4 0 0 1-4-4V13" />
        <path d="m8 16-3-3-3 3" />
      </>
    ),
  };

  return (
    <svg aria-hidden="true" viewBox="0 0 24 24">
      {paths[story]}
    </svg>
  );
}

function StageRail({
  locale,
  stage,
  story,
}: {
  locale: Locale;
  stage: number;
  story: CapabilityStory;
}) {
  return (
    <ol className="a3s-capability-stage-rail">
      {story.stages.map((item, index) => (
        <li
          className={
            index < stage ? 'is-complete' : index === stage ? 'is-active' : ''
          }
          key={item.en}
        >
          <i aria-hidden="true">{index < stage ? '✓' : index + 1}</i>
          <span>{value(item, locale)}</span>
        </li>
      ))}
    </ol>
  );
}

function HitlScene({ locale, stage }: { locale: Locale; stage: number }) {
  const options =
    locale === 'zh'
      ? [
          '仅允许一次',
          '本会话允许精确能力',
          '向项目加入精确规则',
          '拒绝并说明原因',
        ]
      : [
          'Allow once',
          'Allow exact capability for this session',
          'Add exact capability rule to project',
          'Deny and tell the agent why',
        ];

  return (
    <div className="a3s-capability-scene a3s-capability-scene--hitl">
      <div className="a3s-capability-call">
        <span>bash</span>
        <code>git push origin main</code>
        <em className={stage > 0 ? 'is-warning' : ''}>
          {stage === 0 ? 'classifying…' : 'external side effect · ASK'}
        </em>
      </div>
      <div
        className={`a3s-capability-approval ${stage > 0 ? 'is-visible' : ''}`}
      >
        <header>
          <span>HUMAN APPROVAL REQUIRED</span>
          <small>canonical arguments</small>
        </header>
        <p>
          <small>Run</small>
          <code>git push origin main</code>
        </p>
        <ol>
          {options.map((option, index) => (
            <li
              className={stage >= 2 && index === 0 ? 'is-selected' : ''}
              key={option}
            >
              <b>{index + 1}</b>
              <span>{option}</span>
            </li>
          ))}
        </ol>
        <footer className={stage >= 3 ? 'is-resolved' : ''}>
          <i aria-hidden="true" />
          {stage >= 3
            ? locale === 'zh'
              ? '允许一次 · 决定已写入事件流'
              : 'Allowed once · decision recorded in the event stream'
            : locale === 'zh'
              ? '工具尚未执行'
              : 'Tool has not executed'}
        </footer>
      </div>
    </div>
  );
}

function ProgressiveScene({
  locale,
  stage,
}: {
  locale: Locale;
  stage: number;
}) {
  const rows = [
    ['list', '18 modules · knowledge · runtime · assets'],
    ['search', 'knowledge.deploy'],
    ['describe', '{ assetId: string, shaped?: boolean }'],
    ['execute', 'viewUrl → Open view'],
  ];

  return (
    <div className="a3s-capability-scene a3s-capability-scene--progressive">
      <div className="a3s-progressive-query">
        <span>{locale === 'zh' ? '意图' : 'intent'}</span>
        <p>
          {locale === 'zh'
            ? '部署发布知识包，并打开运行视图'
            : 'Deploy the release knowledge package and open its run view'}
        </p>
      </div>
      <div className="a3s-progressive-endpoint">
        <code>POST /api/v1/kernel/capabilities</code>
        <span>signed in · permission filtered</span>
      </div>
      <div className="a3s-progressive-steps">
        {rows.map(([action, result], index) => (
          <div
            className={
              index < stage ? 'is-complete' : index === stage ? 'is-active' : ''
            }
            key={action}
          >
            <b>{action}</b>
            <i aria-hidden="true" />
            <code>{index <= stage ? result : '••••••••'}</code>
          </div>
        ))}
      </div>
      <div className={`a3s-progressive-view ${stage >= 3 ? 'is-ready' : ''}`}>
        <span>↗</span>
        <div>
          <small>RemoteUI</small>
          <strong>
            {stage >= 3
              ? 'Knowledge deployment · ready'
              : 'waiting for shaped response'}
          </strong>
        </div>
      </div>
    </div>
  );
}

function RuntimeScene({ locale, stage }: { locale: Locale; stage: number }) {
  const jobs = [
    ['core', 'a3s-code-core'],
    ['node', '@a3s-lab/code'],
    ['python', 'a3s-code'],
  ];

  return (
    <div className="a3s-capability-scene a3s-capability-scene--runtime">
      <div className="a3s-runtime-tool-call">
        <span>runtime</span>
        <code>{'{ worker: "release-checker", inputs: 3 }'}</code>
      </div>
      <div className="a3s-runtime-worker">
        <div>
          <small>TOOL-KIND WORKER</small>
          <strong>release-checker</strong>
          <code>
            {stage > 0 ? 'resolved · worker_01HV…' : 'resolving by name…'}
          </code>
        </div>
        <i aria-hidden="true" className={stage > 0 ? 'is-ready' : ''} />
      </div>
      <div className="a3s-runtime-batch">
        {jobs.map(([key, name], index) => {
          const complete = stage >= 3 || (stage >= 2 && index < 2);
          const running = stage >= 1 && !complete;
          return (
            <div
              className={complete ? 'is-complete' : running ? 'is-running' : ''}
              key={key}
            >
              <span>{String(index + 1).padStart(2, '0')}</span>
              <p>
                <strong>{name}</strong>
                <small>
                  {complete ? 'passed' : running ? 'running' : 'queued'}
                </small>
              </p>
              <i aria-hidden="true">
                <b />
              </i>
            </div>
          );
        })}
      </div>
      <footer className={stage >= 3 ? 'is-ready' : ''}>
        <span>{stage >= 3 ? '3 / 3' : `${Math.max(stage - 1, 0)} / 3`}</span>
        <p>
          {stage >= 3
            ? locale === 'zh'
              ? '聚合完成 · 3 个独立结果'
              : 'Aggregate ready · 3 independent results'
            : locale === 'zh'
              ? '正在流式接收批任务进度'
              : 'Streaming batch progress'}
        </p>
      </footer>
    </div>
  );
}

function IntelligenceScene({
  locale,
  stage,
}: {
  locale: Locale;
  stage: number;
}) {
  return (
    <div className="a3s-capability-scene a3s-capability-scene--intelligence">
      <header>
        <span>
          <i /> src/runtime_tool.rs
        </span>
        <small>
          {stage > 0 ? 'rust-analyzer · ready' : 'starting language service…'}
        </small>
      </header>
      <div className="a3s-intelligence-workbench">
        <div
          className="a3s-intelligence-code"
          aria-label="Saved Rust source preview"
        >
          <p>
            <span>52</span>
            <code>
              <b>pub(crate) struct</b> <mark>RuntimeTool</mark> {'{'}
            </code>
          </p>
          <p>
            <span>53</span>
            <code> session: OsSession,</code>
          </p>
          <p>
            <span>54</span>
            <code> client: Client,</code>
          </p>
          <p>
            <span>55</span>
            <code>{'}'}</code>
          </p>
          <p>
            <span>···</span>
            <code />
          </p>
          <p className={stage >= 1 ? 'is-symbol' : ''}>
            <span>117</span>
            <code>
              <b>impl Tool for</b> <mark>RuntimeTool</mark> {'{'}
            </code>
          </p>
          <p>
            <span>118</span>
            <code>
              {' '}
              <b>fn</b> name(&amp;self) → &amp;str
            </code>
          </p>
        </div>
        <aside>
          <div className="a3s-intelligence-command">
            <span>:</span>
            <code>references</code>
          </div>
          <strong>
            {locale === 'zh'
              ? '引用 · 已保存版本'
              : 'References · saved version'}
          </strong>
          <ul>
            <li className={stage >= 2 ? 'is-visible' : ''}>
              <b>117:15</b>
              <span>impl Tool for RuntimeTool</span>
            </li>
            <li className={stage >= 2 ? 'is-visible' : ''}>
              <b>188:6</b>
              <span>impl RuntimeTool</span>
            </li>
            <li className={stage >= 2 ? 'is-visible' : ''}>
              <b>413:22</b>
              <span>RuntimeTool::new(session)</span>
            </li>
          </ul>
        </aside>
      </div>
      <footer>
        <span>
          <i className={stage >= 1 ? 'is-ready' : ''} /> saved file semantics
        </span>
        <span className={stage >= 3 ? 'is-ready' : ''}>
          {stage >= 3 ? '0 errors · 2 warnings' : 'diagnostics pending'}
        </span>
        <code>code_symbols · code_navigation · code_diagnostics</code>
      </footer>
    </div>
  );
}

function CtxScene({ locale, stage }: { locale: Locale; stage: number }) {
  const hits = [
    ['A3S', '2026-07-28', 'Preserve shaped view responses'],
    ['Codex', '2026-07-24', 'Runtime view fallback'],
    ['Claude', '2026-07-19', 'Loop report contract'],
  ];

  return (
    <div className="a3s-capability-scene a3s-capability-scene--ctx">
      <div className="a3s-ctx-command">
        <span>❯</span>
        <code>/ctx RemoteUI view link</code>
      </div>
      <div className="a3s-ctx-results">
        <header>
          <span>
            ⌕ {locale === 'zh' ? '跨会话搜索结果' : 'cross-session results'}
          </span>
          <small>local index · limit 8</small>
        </header>
        {hits.map(([provider, date, title], index) => (
          <div
            className={stage >= 1 && index === 1 ? 'is-selected' : ''}
            key={title}
          >
            <b>{index + 1}.</b>
            <span>{provider}</span>
            <small>{date}</small>
            <p>{title}</p>
          </div>
        ))}
      </div>
      <div className="a3s-ctx-actions">
        <div className={stage >= 2 ? 'is-ready' : ''}>
          <code>/ctx 2</code>
          <span>{stage >= 2 ? '✓' : '○'}</span>
          <p>
            {locale === 'zh'
              ? '下一条消息一次性附加'
              : 'one-shot attach to next turn'}
          </p>
        </div>
        <div className={stage >= 3 ? 'is-ready' : ''}>
          <code>/ctx save 2</code>
          <span>{stage >= 3 ? '✓' : '○'}</span>
          <p>
            {locale === 'zh'
              ? '保存到记忆并保留回链'
              : 'save to memory with backlink'}
          </p>
        </div>
      </div>
      <footer>
        <span>source=ctx</span>
        <span>ctx_event_id</span>
        <span>ctx_session_id</span>
      </footer>
    </div>
  );
}

function CapabilityScene({
  locale,
  stage,
  story,
}: {
  locale: Locale;
  stage: number;
  story: CapabilityStory;
}) {
  switch (story.key) {
    case 'hitl':
      return <HitlScene locale={locale} stage={stage} />;
    case 'progressive':
      return <ProgressiveScene locale={locale} stage={stage} />;
    case 'runtime':
      return <RuntimeScene locale={locale} stage={stage} />;
    case 'intelligence':
      return <IntelligenceScene locale={locale} stage={stage} />;
    case 'ctx':
      return <CtxScene locale={locale} stage={stage} />;
  }
}

export function CapabilityStoriesMarkdown({ locale }: { locale: Locale }) {
  const labels = sectionCopy[locale];

  return (
    <section>
      <h2>{labels.title}</h2>
      <p>{labels.body}</p>
      {capabilityStories.map((story) => (
        <section key={story.key}>
          <h3>{value(story.title, locale)}</h3>
          <p>{value(story.body, locale)}</p>
          <p>{value(story.availability, locale)}</p>
        </section>
      ))}
    </section>
  );
}

export function CapabilityShowcase({
  guideHref,
  locale,
}: {
  guideHref: string;
  locale: Locale;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const selectorRef = useRef<HTMLElement>(null);
  const [activeIndex, setActiveIndex] = useState(0);
  const [stage, setStage] = useState(0);
  const [isPlaying, setIsPlaying] = useState(false);
  const [isVisible, setIsVisible] = useState(false);
  const [reducedMotion, setReducedMotion] = useState(false);
  const story = capabilityStories[activeIndex] ?? capabilityStories[0];
  const labels = sectionCopy[locale];

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return undefined;

    const preference = window.matchMedia('(prefers-reduced-motion: reduce)');
    const applyPreference = () => {
      setReducedMotion(preference.matches);
      if (preference.matches) {
        setStage(3);
        setIsPlaying(false);
      }
    };
    applyPreference();

    const observer = new IntersectionObserver(
      ([entry]) => {
        const visible = entry?.isIntersecting ?? false;
        setIsVisible(visible);
        if (visible && !preference.matches) setIsPlaying(true);
      },
      { threshold: 0.32 },
    );
    observer.observe(host);
    preference.addEventListener('change', applyPreference);

    return () => {
      observer.disconnect();
      preference.removeEventListener('change', applyPreference);
    };
  }, []);

  useEffect(() => {
    if (!isPlaying || !isVisible || reducedMotion) return undefined;

    const timer = window.setTimeout(
      () => {
        if (stage < 3) {
          setStage((current) => current + 1);
          return;
        }
        setStage(0);
        setActiveIndex((current) => (current + 1) % capabilityStories.length);
      },
      stage === 3 ? 3000 : 1450,
    );

    return () => window.clearTimeout(timer);
  }, [isPlaying, isVisible, reducedMotion, stage]);

  useEffect(() => {
    const selector = selectorRef.current;
    const button = selector?.children.item(activeIndex);
    if (!(button instanceof HTMLElement) || !selector) return;
    if (selector.scrollWidth <= selector.clientWidth) return;

    const centeredLeft =
      button.offsetLeft - (selector.clientWidth - button.clientWidth) / 2;
    selector.scrollTo({
      behavior: reducedMotion ? 'auto' : 'smooth',
      left: Math.max(0, centeredLeft),
    });
  }, [activeIndex, reducedMotion]);

  function selectStory(index: number) {
    setActiveIndex(index);
    setStage(reducedMotion ? 3 : 0);
    if (!reducedMotion) setIsPlaying(true);
  }

  function togglePlayback() {
    if (isPlaying) {
      setIsPlaying(false);
      return;
    }
    if (stage === 3) setStage(0);
    setIsPlaying(true);
  }

  return (
    <section
      className="a3s-section a3s-capability-stories"
      id="capability-stories"
    >
      <header className="a3s-section-header a3s-capability-stories-header">
        <div>
          <span className="a3s-section-eyebrow">{labels.eyebrow}</span>
          <h2>{labels.title}</h2>
        </div>
        <div>
          <p>{labels.body}</p>
          <a href={guideHref}>
            {labels.guide}
            <span aria-hidden="true">→</span>
          </a>
        </div>
      </header>

      <div
        className={`a3s-capability-console is-${story.key}`}
        data-stage={stage}
        ref={hostRef}
      >
        <nav
          aria-label={labels.select}
          className="a3s-capability-selector"
          ref={selectorRef}
        >
          {capabilityStories.map((item, index) => (
            <button
              aria-current={index === activeIndex ? 'true' : undefined}
              className={index === activeIndex ? 'is-active' : ''}
              key={item.key}
              onClick={() => selectStory(index)}
              type="button"
            >
              <span>{item.index}</span>
              <i>
                <CapabilityIcon story={item.key} />
              </i>
              <p>
                <small>{item.eyebrow}</small>
                <strong>{value(item.title, locale)}</strong>
              </p>
              <em aria-hidden="true">
                <b />
              </em>
            </button>
          ))}
        </nav>

        <div className="a3s-capability-player">
          <header className="a3s-capability-player-bar">
            <span>
              <i />
              <i />
              <i />
            </span>
            <p>
              <b>{labels.live}</b>
              <small>
                {story.index} / {story.eyebrow}
              </small>
            </p>
            <button onClick={togglePlayback} type="button">
              <i
                className={isPlaying ? 'is-pause' : 'is-play'}
                aria-hidden="true"
              />
              {isPlaying
                ? labels.pause
                : stage === 3
                  ? labels.replay
                  : labels.play}
            </button>
          </header>

          <div className="a3s-capability-player-intro">
            <div>
              <span>{story.eyebrow}</span>
              <h3>{value(story.title, locale)}</h3>
              <p>{value(story.body, locale)}</p>
            </div>
            <small>
              <i />
              {value(story.availability, locale)}
            </small>
          </div>

          <div className="a3s-capability-player-stage" key={story.key}>
            <StageRail locale={locale} stage={stage} story={story} />
            <div className="a3s-capability-scene-wrap">
              <CapabilityScene locale={locale} stage={stage} story={story} />
            </div>
          </div>

          <footer className="a3s-capability-player-footer">
            <span>
              {labels.step} {String(stage + 1).padStart(2, '0')} / 04
            </span>
            <div>
              {story.tags.map((tag) => (
                <code key={tag}>{tag}</code>
              ))}
            </div>
          </footer>
        </div>
      </div>
    </section>
  );
}
