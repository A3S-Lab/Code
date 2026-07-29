import { useEffect, useRef, useState } from 'react';
import type { HomeLabels } from './home-copy';

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

export function RuntimeExecutionFlow({ labels }: { labels: HomeLabels }) {
  const playerRef = useRef<HTMLDivElement>(null);
  const hasStartedRef = useRef(false);
  const playOnceRef = useRef(false);
  const [activeIndex, setActiveIndex] = useState(tuiDemoPhases.length - 1);
  const [isPlaying, setIsPlaying] = useState(false);
  const [isVisible, setIsVisible] = useState(false);
  const [prefersReducedMotion, setPrefersReducedMotion] = useState(false);
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

    const motionPreference = window.matchMedia(
      '(prefers-reduced-motion: reduce)',
    );
    setPrefersReducedMotion(motionPreference.matches);

    const observer = new IntersectionObserver(
      ([entry]) => {
        const visible = entry?.isIntersecting ?? false;
        setIsVisible(visible);

        if (visible && !hasStartedRef.current) {
          hasStartedRef.current = true;
          setTypedCount(0);
          if (motionPreference.matches) {
            setActiveIndex(tuiDemoPhases.length - 1);
            setIsPlaying(false);
          } else {
            setActiveIndex(0);
            setIsPlaying(true);
          }
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
        if (playOnceRef.current) {
          playOnceRef.current = false;
          setIsPlaying(false);
        } else {
          setActiveIndex(0);
        }
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
    playOnceRef.current = prefersReducedMotion;
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
          {prefersReducedMotion && !isRunning
            ? labels.flowPlayOnce
            : labels.flowReplay}
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
          <span>a3s-code v6.6.0</span>
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
