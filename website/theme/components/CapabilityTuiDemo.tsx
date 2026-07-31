import { A3sCodeTui } from './A3sCodeTui';
import { CapabilityIdeScene } from './CapabilityIdeScene';
import {
  localized,
  sectionCopy,
  type CapabilityStory,
  type Locale,
} from './capability-stories';

type ToolTone = 'active' | 'success' | 'warning' | 'muted';

type ToolBranch = {
  text: string;
  tone?: 'success' | 'warning';
};

function UserMessage({ children }: { children: string }) {
  return (
    <article className="a3s-real-user-message">
      <span aria-hidden="true">›</span>
      <p>{children}</p>
    </article>
  );
}

function ToolLine({
  action,
  branches = [],
  detail,
  tone,
}: {
  action: string;
  branches?: ToolBranch[];
  detail?: string;
  tone: ToolTone;
}) {
  return (
    <article className={`a3s-real-tool-line is-${tone}`}>
      <header>
        <i aria-hidden="true">•</i>
        <strong>{action}</strong>
        {detail ? <code>{detail}</code> : null}
      </header>
      {branches.length > 0 ? (
        <div>
          {branches.map((branch, index) => (
            <p
              className={branch.tone ? `is-${branch.tone}` : ''}
              key={branch.text}
            >
              <span aria-hidden="true">{index === 0 ? '└' : ' '}</span>
              <code>{branch.text}</code>
            </p>
          ))}
        </div>
      ) : null}
    </article>
  );
}

function ApprovalPrompt({
  label,
  selected,
}: {
  label: string;
  selected: number;
}) {
  const options = [
    ['↵', 'Allow once'],
    ['◎', 'Allow exact capability for this session'],
    ['⌘', 'Add exact capability rule to project'],
    ['⊘', 'Deny and tell the agent why'],
  ];

  return (
    <section className="a3s-real-approval" aria-label="Permission required">
      <header>
        <span aria-hidden="true">◆</span>
        <strong>Permission required</strong>
      </header>
      <p className="a3s-real-approval-detail">
        <span>Run</span>
        <code>{label}</code>
      </p>
      <ol>
        {options.map(([glyph, option], index) => (
          <li className={index === selected ? 'is-selected' : ''} key={option}>
            <span aria-hidden="true">{index === selected ? '❯' : ''}</span>
            <b>{index + 1}</b>
            <i aria-hidden="true">{glyph}</i>
            <p>{option}</p>
          </li>
        ))}
      </ol>
      <footer>Enter select · ↑↓ move · Esc deny</footer>
    </section>
  );
}

function HitlScene({ locale, stage }: { locale: Locale; stage: number }) {
  const prompt = localized(
    {
      zh: '测试通过后，将 main 分支推送到 origin',
      en: 'Push main to origin after the tests pass',
    },
    locale,
  );

  return (
    <div className="a3s-real-transcript" data-real-tui-scene="hitl">
      <UserMessage>{prompt}</UserMessage>
      {stage === 0 ? (
        <ToolLine
          action="Preparing"
          detail="git push origin main"
          tone="muted"
        />
      ) : stage < 3 ? (
        <ToolLine
          action="Awaiting approval for"
          detail="git push origin main"
          tone="warning"
        />
      ) : (
        <ToolLine
          action="Ran"
          branches={[
            { text: 'To github.com:A3S-Lab/Code.git' },
            { text: 'main -> main', tone: 'success' },
          ]}
          detail="git push origin main"
          tone="success"
        />
      )}
    </div>
  );
}

const progressiveCalls = [
  {
    request: `curl … -d '{"action":"list"}'`,
    result: 'knowledge · runtime · assets · workflows',
  },
  {
    request: `curl … -d '{"action":"search","query":"knowledge deploy"}'`,
    result: 'knowledge.deploy · score 0.94',
  },
  {
    request: `curl … -d '{"action":"describe","operation":"deploy"}'`,
    result: 'assetId:string · release:string · shaped:boolean',
  },
  {
    request: `curl … -d '{"action":"execute","shaped":true,…}'`,
    result: 'requestId req_01J… · view returned',
  },
];

function OpenView() {
  return (
    <p className="a3s-real-open-view">
      <i aria-hidden="true">•</i>
      <span aria-hidden="true">↗</span>
      <strong>Open view</strong>
      <small>· click to open</small>
    </p>
  );
}

function ProgressiveScene({
  locale,
  stage,
}: {
  locale: Locale;
  stage: number;
}) {
  const prompt = localized(
    {
      zh: '部署发布知识包，并打开运行视图',
      en: 'Deploy the release knowledge package and open its run view',
    },
    locale,
  );

  return (
    <div className="a3s-real-transcript" data-real-tui-scene="progressive">
      <UserMessage>{prompt}</UserMessage>
      <div className="a3s-real-tool-stack">
        {progressiveCalls.slice(0, stage + 1).map((call, index) => {
          const complete =
            index < stage || stage === progressiveCalls.length - 1;
          return (
            <ToolLine
              action={complete ? 'Ran' : 'Running'}
              branches={complete ? [{ text: call.result }] : []}
              detail={call.request}
              key={call.request}
              tone={complete ? 'success' : 'active'}
            />
          );
        })}
      </div>
      {stage === 3 ? <OpenView /> : null}
    </div>
  );
}

function RuntimeScene({ locale, stage }: { locale: Locale; stage: number }) {
  const prompt = localized(
    {
      zh: '并行检查 core、Node 和 Python 发布包',
      en: 'Check the core, Node, and Python releases in parallel',
    },
    locale,
  );

  const runningBranches: ToolBranch[] = [
    { text: 'worker release-checker -> 57989959-0b1d-41da-974c-31ad8101df37' },
    { text: '3 parallel subtasks submitted (batch batch-01J…)' },
  ];
  if (stage >= 2) {
    runningBranches.push({ text: '⏳ 1/3 done · 2 running · 0 queued' });
  }

  return (
    <div className="a3s-real-transcript" data-real-tui-scene="runtime">
      <UserMessage>{prompt}</UserMessage>
      {stage === 0 ? (
        <ToolLine
          action="Awaiting approval for"
          detail="3 tasks via release-checker: check core; check Node +1 more"
          tone="warning"
        />
      ) : stage < 3 ? (
        <ToolLine
          action="Running Runtime"
          branches={runningBranches}
          detail="3 tasks via release-checker: check core; check Node +1 more"
          tone="active"
        />
      ) : (
        <ToolLine
          action="Used Runtime"
          branches={[
            { text: '✓ task 1 · inv-core · completed', tone: 'success' },
            { text: '✓ task 2 · inv-node · completed', tone: 'success' },
            { text: '✓ task 3 · inv-python · completed', tone: 'success' },
          ]}
          detail="3/3 tasks via 57989959-0b1d-41da-974c-31ad8101df37"
          tone="success"
        />
      )}
    </div>
  );
}

function RuntimeTracker({ locale, stage }: { locale: Locale; stage: number }) {
  if (stage < 1 || stage > 2) return null;

  const rows = [
    ['runtime', 'check a3s-code-core release', stage === 2 ? 'done' : 'active'],
    ['runtime', 'check @a3s-lab/code release', 'active'],
    ['runtime', 'check a3s-code release', 'active'],
  ] as const;
  const running = rows.filter(([, , status]) => status === 'active').length;

  return (
    <section className="a3s-tui-agents a3s-real-runtime-tracker">
      <header>
        <span aria-hidden="true">•</span>
        <strong>
          {localized(
            {
              zh: '并行检查 core、Node 和 Python 发布包',
              en: 'Check the core, Node, and Python releases in parallel',
            },
            locale,
          )}
        </strong>
        <small>
          {running} running · {3 - running}/3 done · 00:04
        </small>
      </header>
      <div>
        {rows.map(([name, task, status]) => (
          <p className={`is-${status}`} key={task}>
            <i aria-hidden="true">•</i>
            <b>{name}</b>
            <span>{task}</span>
            <small>00:03 · ↓ 0.4k</small>
          </p>
        ))}
      </div>
    </section>
  );
}

const ctxHits = [
  [
    '1.',
    'A3S · 2026-07-28 · Preserve shaped view responses',
    'Execute responses keep the .view object so the host can surface Open view.',
  ],
  [
    '2.',
    'Codex · 2026-07-24 · Runtime view fallback',
    'Resolve relative ViewLink URLs against the authenticated OS origin.',
  ],
  [
    '3.',
    'Claude · 2026-07-19 · Loop report contract',
    'Keep the report artifact and its source requestId together.',
  ],
];

function CtxResults() {
  return (
    <section className="a3s-real-ctx-results">
      {ctxHits.map(([index, title, snippet]) => (
        <div key={title}>
          <p>
            <b>{index}</b>
            <strong>{title}</strong>
          </p>
          <small>{snippet}</small>
        </div>
      ))}
      <footer>
        ⧉ /ctx &lt;n&gt; attaches to next message · /ctx save &lt;n&gt; keeps as
        memory
      </footer>
    </section>
  );
}

function CtxScene({ stage }: { stage: number }) {
  return (
    <div className="a3s-real-transcript a3s-real-ctx" data-real-tui-scene="ctx">
      {stage >= 1 ? <CtxResults /> : null}
      {stage >= 2 ? (
        <p className="a3s-real-notice is-success">
          <i aria-hidden="true">✔</i>
          context staged — it will be attached to your next message (one-shot)
        </p>
      ) : null}
      {stage >= 3 ? (
        <p className="a3s-real-notice is-success">
          <i aria-hidden="true">✔</i>
          saved to memory: Runtime view fallback · shows in /memory (source=ctx)
        </p>
      ) : null}
    </div>
  );
}

function Scene({
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
      return <CapabilityIdeScene stage={stage} />;
    case 'ctx':
      return <CtxScene stage={stage} />;
  }
}

function composerText(story: CapabilityStory, stage: number, locale: Locale) {
  if (story.key === 'intelligence') return stage === 0 ? '/ide' : '';
  if (story.key === 'ctx') {
    if (stage === 0) return localized(story.prompt, locale);
    if (stage === 1) return '/ctx 2';
    if (stage === 2) return '/ctx save 2';
  }
  return '';
}

function isWorking(story: CapabilityStory, stage: number) {
  if (story.key === 'hitl') return stage === 0;
  if (story.key === 'progressive') return stage < 3;
  if (story.key === 'runtime') return stage === 1 || stage === 2;
  return false;
}

export function CapabilityTuiDemo({
  isPlaying,
  isVisible,
  locale,
  onPlayback,
  reducedMotion,
  stage,
  story,
}: {
  isPlaying: boolean;
  isVisible: boolean;
  locale: Locale;
  onPlayback: () => void;
  reducedMotion: boolean;
  stage: number;
  story: CapabilityStory;
}) {
  const labels = sectionCopy[locale];
  const working = isPlaying && isVisible && isWorking(story, stage);
  const fullscreen = story.key === 'intelligence' && stage > 0;
  const approval =
    story.key === 'hitl' && stage > 0 && stage < 3 ? (
      <ApprovalPrompt
        label="Git(git push origin main)"
        selected={stage === 1 ? 0 : 1}
      />
    ) : story.key === 'runtime' && stage === 0 ? (
      <ApprovalPrompt
        label="Runtime(3 tasks via release-checker: check core; check Node +1 more)"
        selected={0}
      />
    ) : undefined;

  return (
    <A3sCodeTui
      activity={
        working ? (
          <>
            <i aria-hidden="true">✶</i>
            <span>Working…</span>
            <small>(00:04 · ↓ 1.2k tokens)</small>
          </>
        ) : undefined
      }
      afterFooter={
        story.key === 'runtime' ? (
          <RuntimeTracker locale={locale} stage={stage} />
        ) : undefined
      }
      ariaLabel={`${story.eyebrow}: ${localized(story.title, locale)}`}
      beforeInput={approval}
      className={`a3s-capability-player is-${story.key}`}
      composerHidden={fullscreen}
      composerStatus="◇ high"
      composerText={composerText(story, stage, locale)}
      contextLabel={labels.context}
      isPlaying={isPlaying && isVisible}
      modeGlyph="⏵"
      modeLabel="default mode"
      onPlayback={onPlayback}
      phase={`${story.key}-${stage + 1}`}
      playbackLabel={
        isPlaying ? labels.pause : stage === 3 ? labels.replay : labels.play
      }
      showCursor={!reducedMotion}
      surface={`capability-${story.key}`}
      workspace={labels.workspace}
    >
      <Scene locale={locale} stage={stage} story={story} />
    </A3sCodeTui>
  );
}
