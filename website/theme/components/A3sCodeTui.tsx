import { forwardRef, type ReactNode } from 'react';

export type A3sCodeTuiInputMode = 'default' | 'shell' | 'research';

type A3sCodeTuiProps = {
  activity?: ReactNode;
  afterFooter?: ReactNode;
  ariaLabel: string;
  beforeInput?: ReactNode;
  branch?: string;
  children: ReactNode;
  className?: string;
  composerMode?: A3sCodeTuiInputMode;
  composerStatus: string;
  composerSymbol?: string;
  composerText: string;
  contextLabel: string;
  identity?: string;
  isPlaying: boolean;
  modeLabel: string;
  model?: string;
  onPlayback: () => void;
  phase?: string;
  playbackLabel: string;
  showCursor?: boolean;
  surface: string;
  workspace: string;
};

function classNames(...values: Array<string | false | undefined>) {
  return values.filter(Boolean).join(' ');
}

export const A3sCodeTui = forwardRef<HTMLDivElement, A3sCodeTuiProps>(
  function A3sCodeTui(
    {
      activity,
      afterFooter,
      ariaLabel,
      beforeInput,
      branch = 'git:(main)',
      children,
      className,
      composerMode = 'default',
      composerStatus,
      composerSymbol = '❯',
      composerText,
      contextLabel,
      identity = 'a3s',
      isPlaying,
      modeLabel,
      model = 'gpt-5 (128k context)',
      onPlayback,
      phase,
      playbackLabel,
      showCursor = true,
      surface,
      workspace,
    },
    ref,
  ) {
    return (
      <div
        aria-label={ariaLabel}
        className={classNames(
          'a3s-runtime-inspector',
          'a3s-tui-player',
          isPlaying && 'is-running',
          className,
        )}
        data-a3s-code-tui={surface}
        data-phase={phase}
        ref={ref}
      >
        <header className="a3s-tui-titlebar" data-tui-region="titlebar">
          <span className="a3s-tui-window-dots" aria-hidden="true">
            <i />
            <i />
            <i />
          </span>
          <div className="a3s-tui-title">
            <b>a3s code</b>
            <em>{workspace}</em>
          </div>
          <button
            aria-pressed={isPlaying}
            className={isPlaying ? 'is-playing' : ''}
            onClick={onPlayback}
            type="button"
          >
            <i aria-hidden="true" />
            {playbackLabel}
          </button>
        </header>

        <section className="a3s-tui-terminal" data-tui-region="terminal">
          {children}
        </section>

        <section
          className="a3s-tui-composer"
          data-input-mode={composerMode}
          data-tui-region="composer"
        >
          <div className="a3s-tui-activity" aria-live="polite">
            {activity}
          </div>
          {beforeInput}
          <div className="a3s-tui-effort-rule">
            <span>{composerStatus}</span>
          </div>
          <div className="a3s-tui-input">
            <span aria-hidden="true">{composerSymbol}</span>
            <p>
              {composerText}
              {showCursor ? <i aria-hidden="true" /> : null}
            </p>
          </div>
          <div className="a3s-tui-input-rule" />
          <footer className="a3s-tui-footer" data-tui-region="footer">
            <span className="a3s-tui-mode">
              <i aria-hidden="true">●</i>
              {modeLabel}
            </span>
            <span className="a3s-tui-context">
              {contextLabel}
              <i aria-hidden="true">
                <b />
              </i>
            </span>
            <span className="a3s-tui-identity">
              <b>{identity}</b>
              <em>{branch}</em>
              <em>{model}</em>
            </span>
          </footer>
          {afterFooter}
        </section>
      </div>
    );
  },
);
