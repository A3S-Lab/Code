import { TuiWelcomeBanner } from './TuiWelcomeBanner';

const codeRows = [
  ['52', 'pub(crate) struct RuntimeTool {'],
  ['53', '    session: OsSession,'],
  ['54', '    client: Client,'],
  ['55', '}'],
  ['…', ''],
  ['117', 'impl Tool for RuntimeTool {'],
  ['118', '    fn name(&self) -> &str {'],
  ['119', '        "runtime"'],
  ['120', '    }'],
];

function IdeTree() {
  return (
    <section className="a3s-real-ide-frame a3s-real-ide-tree">
      <header>▾ a3s</header>
      <div>
        <p>
          <span>▾</span>
          <b>src</b>
        </p>
        <p className="is-selected">
          <span>◇</span>
          <b>runtime_tool.rs</b>
        </p>
        <p>
          <span>◇</span>
          <b>session_llm.rs</b>
        </p>
        <p>
          <span>▸</span>
          <b>tui</b>
        </p>
        <p>
          <span>◇</span>
          <b>main.rs</b>
        </p>
        <p>
          <span>◇</span>
          <b>Cargo.toml</b>
        </p>
      </div>
    </section>
  );
}

function IdeEditor({ stage }: { stage: number }) {
  const title =
    stage === 1
      ? '◇ src › runtime_tool.rs'
      : stage === 2
        ? '⌁ References · src/runtime_tool.rs'
        : '⌁ References · src/runtime_tool.rs · rev 42';

  return (
    <section className="a3s-real-ide-frame a3s-real-ide-editor">
      <header>{title}</header>
      {stage === 1 ? (
        <div className="a3s-real-ide-code">
          {codeRows.map(([line, code]) => (
            <p
              className={line === '117' ? 'is-current' : ''}
              key={`${line}-${code}`}
            >
              <span>{line}</span>
              <code>{code}</code>
            </p>
          ))}
        </div>
      ) : stage === 2 ? (
        <div className="a3s-real-ide-results is-loading">
          <p className="is-selected">› Loading Code Intelligence…</p>
        </div>
      ) : (
        <div className="a3s-real-ide-results">
          <p className="is-selected">› src/runtime_tool.rs:117:15</p>
          <p> src/runtime_tool.rs:188:6</p>
          <p> src/tui/app/view.rs:413:22</p>
          <p> src/runtime_tool/tests.rs:42:16</p>
        </div>
      )}
    </section>
  );
}

export function CapabilityIdeScene({ stage }: { stage: number }) {
  if (stage === 0) {
    return (
      <div data-real-tui-scene="intelligence">
        <TuiWelcomeBanner />
      </div>
    );
  }

  return (
    <div className="a3s-real-ide" data-real-tui-scene="intelligence">
      <div className="a3s-real-ide-main">
        <IdeTree />
        <IdeEditor stage={stage} />
      </div>
      <div className="a3s-real-ide-footer">
        <section className="a3s-real-ide-frame">
          <header>details</header>
          <p>◇ src › runtime_tool.rs · 428 lines · Rust</p>
        </section>
        <section className="a3s-real-ide-frame">
          <header>controls</header>
          <p>
            {stage === 1
              ? ':references'
              : 'Saved version · ↑↓ select · Enter jump · Esc close'}
          </p>
        </section>
      </div>
    </div>
  );
}
