import { type ReactNode, useMemo, useState } from 'react';
import { useLang } from '@rspress/core/runtime';

type Locale = 'zh' | 'en';
type Status = 'idle' | 'running' | 'success' | 'error';

type DemoProps = {
  locale: Locale;
};

const labels = {
  zh: {
    live: '可交互示例',
    local: '只改变本地状态，不会调用 API',
    reset: '重置',
    idle: '等待',
    running: '执行中',
    success: '完成',
    error: '失败',
    details: '详情',
    collapse: '收起',
    empty: '暂无内容',
  },
  en: {
    live: 'INTERACTIVE EXAMPLE',
    local: 'Local state only; no API calls',
    reset: 'Reset',
    idle: 'Idle',
    running: 'Running',
    success: 'Complete',
    error: 'Failed',
    details: 'Details',
    collapse: 'Collapse',
    empty: 'Nothing here',
  },
} as const;

const tuiNames = new Set([
  'ActivityBlock',
  'Checklist',
  'ChoicePrompt',
  'Confirm',
  'TextInput',
  'Tabs',
  'TreePicker',
  'DataTable',
  'DiffView',
  'ToolLogView',
  'Alert',
  'Progress',
  'Toast',
  'SessionStatus',
]);

function Controls({
  children,
  label,
}: {
  children: ReactNode;
  label?: string;
}) {
  return (
    <div className="a3s-demo-controls" aria-label={label}>
      {children}
    </div>
  );
}

function ControlButton({
  active,
  children,
  onClick,
}: {
  active?: boolean;
  children: ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      aria-pressed={active}
      className={active ? 'is-active' : undefined}
      onClick={onClick}
      type="button"
    >
      {children}
    </button>
  );
}

function TuiWindow({
  children,
  footer = 'default · medium    ctx 38%',
}: {
  children: ReactNode;
  footer?: string;
}) {
  return (
    <div className="a3s-demo-tui-window">
      <header>
        <span>A3S CODE</span>
        <span>~/workspace</span>
        <span>80 × 24</span>
      </header>
      <div className="a3s-demo-tui-body">{children}</div>
      <footer>{footer}</footer>
    </div>
  );
}

function WebPanel({ children }: { children: ReactNode }) {
  return (
    <div className="a3s-demo-web-panel">
      <header>
        <span>
          <i />
          A3S Web
        </span>
        <span>LOCAL PREVIEW</span>
      </header>
      <div>{children}</div>
    </div>
  );
}

function StatusControls({
  onChange,
  status,
}: {
  onChange: (status: Status) => void;
  status: Status;
}) {
  return (
    <Controls label="State">
      {(['idle', 'running', 'success', 'error'] as Status[]).map((item) => (
        <ControlButton
          active={status === item}
          key={item}
          onClick={() => onChange(item)}
        >
          {item}
        </ControlButton>
      ))}
    </Controls>
  );
}

function ActivityBlockDemo({ locale }: DemoProps) {
  const copy = labels[locale];
  const [status, setStatus] = useState<Status>('running');
  const [open, setOpen] = useState(true);
  const statusIcon = {
    idle: '·',
    running: '◌',
    success: '✓',
    error: '×',
  }[status];

  return (
    <>
      <StatusControls onChange={setStatus} status={status} />
      <TuiWindow>
        <button
          className={`a3s-tui-activity-block is-${status}`}
          onClick={() => setOpen((value) => !value)}
          type="button"
        >
          <span>{statusIcon}</span>
          <span>
            <strong>
              {locale === 'zh' ? '检查认证调用路径' : 'Inspect auth call paths'}
            </strong>
            <small>
              {status === 'running'
                ? locale === 'zh'
                  ? '正在读取 4 个文件'
                  : 'Reading 4 files'
                : copy[status]}
            </small>
          </span>
          <i>{open ? '−' : '+'}</i>
        </button>
        {open && (
          <div className="a3s-tui-activity-detail">
            <p>
              <span>✓</span> read <code>src/auth/session.rs</code>
            </p>
            <p>
              <span>✓</span> grep <code>refresh_token</code>
            </p>
            <p className={status === 'running' ? 'is-running' : undefined}>
              <span>{status === 'running' ? '◌' : '·'}</span>{' '}
              {locale === 'zh' ? '整理调用关系' : 'Map the call graph'}
            </p>
          </div>
        )}
      </TuiWindow>
    </>
  );
}

type CheckState = 'todo' | 'doing' | 'done';

function ChecklistDemo({ locale }: DemoProps) {
  const initial: CheckState[] = ['done', 'doing', 'todo'];
  const [items, setItems] = useState<CheckState[]>(initial);
  const text =
    locale === 'zh'
      ? ['定位失效分支', '更新会话恢复逻辑', '补充回归测试']
      : [
          'Locate the expiry branch',
          'Update session recovery',
          'Add a regression test',
        ];

  function cycle(index: number) {
    const next: Record<CheckState, CheckState> = {
      todo: 'doing',
      doing: 'done',
      done: 'todo',
    };
    setItems((current) =>
      current.map((item, itemIndex) =>
        itemIndex === index ? next[item] : item,
      ),
    );
  }

  return (
    <>
      <Controls>
        <button onClick={() => setItems(initial)} type="button">
          {labels[locale].reset}
        </button>
        <span>
          {items.filter((item) => item === 'done').length} / {items.length}
        </span>
      </Controls>
      <TuiWindow>
        <section className="a3s-tui-checklist-demo">
          <header>
            <strong>{locale === 'zh' ? '任务清单' : 'Checklist'}</strong>
            <span>{items.filter((item) => item === 'done').length}/3</span>
          </header>
          {items.map((state, index) => (
            <button
              className={`is-${state}`}
              key={text[index]}
              onClick={() => cycle(index)}
              type="button"
            >
              <span>
                {state === 'done' ? '✓' : state === 'doing' ? '◌' : '·'}
              </span>
              <span>{text[index]}</span>
              <small>{state}</small>
            </button>
          ))}
        </section>
      </TuiWindow>
    </>
  );
}

function ChoicePromptDemo({ locale }: DemoProps) {
  const [selected, setSelected] = useState(1);
  const options =
    locale === 'zh'
      ? ['只运行相关测试', '运行完整测试套件', '先不运行']
      : ['Run related tests', 'Run the full suite', 'Skip for now'];

  return (
    <>
      <Controls>
        <span>selected: {selected + 1}</span>
        <button onClick={() => setSelected(0)} type="button">
          {labels[locale].reset}
        </button>
      </Controls>
      <TuiWindow footer="↑↓ move    Enter select    Esc cancel">
        <section className="a3s-tui-choice">
          <span>QUESTION</span>
          <h4>
            {locale === 'zh'
              ? '修改完成后要运行哪些测试？'
              : 'Which tests should run after the change?'}
          </h4>
          {options.map((option, index) => (
            <button
              className={selected === index ? 'is-active' : undefined}
              key={option}
              onClick={() => setSelected(index)}
              type="button"
            >
              <span>{selected === index ? '›' : ' '}</span>
              {option}
            </button>
          ))}
        </section>
      </TuiWindow>
    </>
  );
}

function ConfirmDemo({ locale }: DemoProps) {
  const [decision, setDecision] = useState<'allow' | 'deny' | null>(null);

  return (
    <>
      <Controls>
        <span>decision: {decision ?? 'pending'}</span>
        <button onClick={() => setDecision(null)} type="button">
          {labels[locale].reset}
        </button>
      </Controls>
      <TuiWindow footer="←→ choose    Enter confirm    Esc deny">
        <section className="a3s-tui-confirm">
          <span className="a3s-demo-risk">CONFIRM</span>
          <h4>
            {locale === 'zh' ? '允许运行本地命令？' : 'Allow a local command?'}
          </h4>
          <code>cargo test auth::session</code>
          <p>
            {locale === 'zh'
              ? '命令只在当前 Workspace 内运行。'
              : 'The command runs only in the current workspace.'}
          </p>
          {decision ? (
            <output className={`is-${decision}`}>
              {decision === 'allow'
                ? locale === 'zh'
                  ? '已允许这一次操作'
                  : 'Allowed for this operation'
                : locale === 'zh'
                  ? '已拒绝，不会执行'
                  : 'Denied; nothing will run'}
            </output>
          ) : (
            <footer>
              <button onClick={() => setDecision('deny')} type="button">
                {locale === 'zh' ? '拒绝' : 'Deny'}
              </button>
              <button
                className="is-primary"
                onClick={() => setDecision('allow')}
                type="button"
              >
                {locale === 'zh' ? '允许一次' : 'Allow once'}
              </button>
            </footer>
          )}
        </section>
      </TuiWindow>
    </>
  );
}

function TextInputDemo({ locale }: DemoProps) {
  const initial =
    locale === 'zh'
      ? '检查登录流程并补上测试'
      : 'Inspect the login flow and add tests';
  const [value, setValue] = useState(initial);

  return (
    <>
      <Controls>
        <span>length: {value.length}</span>
        <button onClick={() => setValue('')} type="button">
          clear
        </button>
      </Controls>
      <TuiWindow footer="Enter submit    Alt+Enter newline">
        <section className="a3s-tui-input-demo">
          <label htmlFor="a3s-tui-text-input">
            {locale === 'zh' ? '给 Agent 一条指令' : 'Send an instruction'}
          </label>
          <div>
            <span>›</span>
            <input
              id="a3s-tui-text-input"
              onChange={(event) => setValue(event.target.value)}
              spellCheck="false"
              value={value}
            />
            <small>{value.length}/240</small>
          </div>
          <p>
            {value.trim()
              ? locale === 'zh'
                ? 'Enter 发送'
                : 'Press Enter to send'
              : locale === 'zh'
                ? '请输入内容'
                : 'Type an instruction'}
          </p>
        </section>
      </TuiWindow>
    </>
  );
}

function TabsDemo({ locale }: DemoProps) {
  const tabs = ['Transcript', 'Changes', 'Artifacts'];
  const [active, setActive] = useState(tabs[0]);

  return (
    <>
      <Controls>
        <span>active: {active}</span>
      </Controls>
      <TuiWindow>
        <section className="a3s-tui-tabs-demo">
          <nav>
            {tabs.map((tab) => (
              <button
                aria-selected={active === tab}
                key={tab}
                onClick={() => setActive(tab)}
                role="tab"
                type="button"
              >
                {tab}
              </button>
            ))}
          </nav>
          <div>
            <span>{active.toUpperCase()}</span>
            <p>
              {active === 'Transcript'
                ? locale === 'zh'
                  ? '显示用户消息、推理与工具活动。'
                  : 'User messages, reasoning, and tool activity.'
                : active === 'Changes'
                  ? locale === 'zh'
                    ? '2 个文件已修改，+18 −6。'
                    : '2 files changed, +18 −6.'
                  : locale === 'zh'
                    ? '当前 Run 生成了 3 个 Artifact。'
                    : 'The current run produced 3 artifacts.'}
            </p>
          </div>
        </section>
      </TuiWindow>
    </>
  );
}

function TreePickerDemo({ locale }: DemoProps) {
  const [expanded, setExpanded] = useState(true);
  const [selected, setSelected] = useState(['src/auth/session.rs']);
  const files = ['src/auth/session.rs', 'src/auth/token.rs', 'Cargo.toml'];

  function toggle(file: string) {
    setSelected((current) =>
      current.includes(file)
        ? current.filter((item) => item !== file)
        : [...current, file],
    );
  }

  return (
    <>
      <Controls>
        <span>{selected.length} selected</span>
        <button onClick={() => setSelected([])} type="button">
          clear
        </button>
      </Controls>
      <TuiWindow footer="Space toggle    Enter accept">
        <section className="a3s-tui-tree-demo">
          <header>
            {locale === 'zh' ? '选择上下文文件' : 'Pick context files'}
          </header>
          <button onClick={() => setExpanded((value) => !value)} type="button">
            <span>{expanded ? '▾' : '▸'}</span> src
          </button>
          {expanded &&
            files.slice(0, 2).map((file) => (
              <button key={file} onClick={() => toggle(file)} type="button">
                <span>│</span>
                <i>{selected.includes(file) ? '■' : '□'}</i>
                {file.replace('src/', '')}
              </button>
            ))}
          <button onClick={() => toggle(files[2])} type="button">
            <span> </span>
            <i>{selected.includes(files[2]) ? '■' : '□'}</i>
            Cargo.toml
          </button>
        </section>
      </TuiWindow>
    </>
  );
}

type SortKey = 'name' | 'duration' | 'status';

function DataTableDemo({ locale }: DemoProps) {
  const [sort, setSort] = useState<SortKey>('duration');
  const [selected, setSelected] = useState('read');
  const rows = useMemo(
    () => [
      { name: 'read', duration: 42, status: 'done' },
      { name: 'grep', duration: 118, status: 'done' },
      { name: 'bash', duration: 934, status: 'running' },
    ],
    [],
  );
  const sorted = [...rows].sort((a, b) =>
    sort === 'duration'
      ? b.duration - a.duration
      : a[sort].localeCompare(b[sort]),
  );

  return (
    <>
      <Controls>
        {(['name', 'duration', 'status'] as SortKey[]).map((key) => (
          <ControlButton
            active={sort === key}
            key={key}
            onClick={() => setSort(key)}
          >
            {key}
          </ControlButton>
        ))}
      </Controls>
      <TuiWindow>
        <table className="a3s-tui-table-demo">
          <caption>{locale === 'zh' ? '工具调用' : 'Tool calls'}</caption>
          <thead>
            <tr>
              <th>TOOL</th>
              <th>STATUS</th>
              <th>DURATION</th>
            </tr>
          </thead>
          <tbody>
            {sorted.map((row) => (
              <tr
                className={selected === row.name ? 'is-active' : undefined}
                key={row.name}
                onClick={() => setSelected(row.name)}
              >
                <td>{row.name}</td>
                <td>{row.status}</td>
                <td>{row.duration} ms</td>
              </tr>
            ))}
          </tbody>
        </table>
      </TuiWindow>
    </>
  );
}

function DiffViewDemo({ locale: _locale }: DemoProps) {
  const [view, setView] = useState<'unified' | 'split'>('unified');
  const [whitespace, setWhitespace] = useState(false);

  return (
    <>
      <Controls>
        <ControlButton
          active={view === 'unified'}
          onClick={() => setView('unified')}
        >
          unified
        </ControlButton>
        <ControlButton
          active={view === 'split'}
          onClick={() => setView('split')}
        >
          split
        </ControlButton>
        <ControlButton
          active={whitespace}
          onClick={() => setWhitespace((value) => !value)}
        >
          whitespace
        </ControlButton>
      </Controls>
      <TuiWindow>
        <section className={`a3s-tui-diff-demo is-${view}`}>
          <header>
            <strong>src/auth/session.rs</strong>
            <span>+4 −1</span>
          </header>
          <pre>
            <span>
              <i>42</i> match refresh_token(state).await {'{'}
            </span>
            <span className="is-removed">
              <i>43</i>- Err(_) =&gt; AuthError::Expired,
            </span>
            <span className="is-added">
              <i>43</i>+ Err(AuthError::Expired) =&gt; {'{'}
            </span>
            <span className="is-added">
              <i>44</i>+ {'  '}session.require_login().await?;
            </span>
            <span className="is-added">
              <i>45</i>+ {'  '}session.retry_refresh().await
              {whitespace ? '··' : ''}
            </span>
            <span className="is-added">
              <i>46</i>+ {'}'}
            </span>
          </pre>
        </section>
      </TuiWindow>
    </>
  );
}

function ToolLogViewDemo({ locale }: DemoProps) {
  const [filter, setFilter] = useState<'all' | 'active'>('all');
  const [open, setOpen] = useState('bash');
  const entries = [
    { id: 'read', state: 'done', text: 'read src/auth/session.rs' },
    { id: 'grep', state: 'done', text: 'grep refresh_token' },
    { id: 'bash', state: 'active', text: 'cargo test auth::session' },
  ];

  return (
    <>
      <Controls>
        <ControlButton
          active={filter === 'all'}
          onClick={() => setFilter('all')}
        >
          all
        </ControlButton>
        <ControlButton
          active={filter === 'active'}
          onClick={() => setFilter('active')}
        >
          active
        </ControlButton>
      </Controls>
      <TuiWindow>
        <section className="a3s-tui-log-demo">
          <header>TOOL LOG</header>
          {entries
            .filter((entry) => filter === 'all' || entry.state === 'active')
            .map((entry) => (
              <button
                className={`is-${entry.state}`}
                key={entry.id}
                onClick={() => setOpen(open === entry.id ? '' : entry.id)}
                type="button"
              >
                <span>{entry.state === 'done' ? '✓' : '◌'}</span>
                <strong>{entry.text}</strong>
                <i>{open === entry.id ? '−' : '+'}</i>
                {open === entry.id && (
                  <small>
                    {entry.id === 'bash'
                      ? locale === 'zh'
                        ? '4 项测试通过，耗时 0.93s'
                        : '4 tests passed in 0.93s'
                      : 'exit 0'}
                  </small>
                )}
              </button>
            ))}
        </section>
      </TuiWindow>
    </>
  );
}

function AlertDemo({ locale }: DemoProps) {
  const [kind, setKind] = useState<'info' | 'success' | 'warning' | 'error'>(
    'warning',
  );
  const copy = {
    info:
      locale === 'zh'
        ? 'Session 已连接到当前 Workspace。'
        : 'Session connected to the current workspace.',
    success:
      locale === 'zh'
        ? '验证完成，4 项测试通过。'
        : 'Verified: 4 tests passed.',
    warning:
      locale === 'zh'
        ? '上下文已达到 80%，下一轮将自动压缩。'
        : 'Context reached 80%; the next turn will compact it.',
    error:
      locale === 'zh'
        ? '命令被当前权限规则拒绝。'
        : 'The current permission policy denied the command.',
  };

  return (
    <>
      <Controls>
        {(Object.keys(copy) as (keyof typeof copy)[]).map((item) => (
          <ControlButton
            active={kind === item}
            key={item}
            onClick={() => setKind(item)}
          >
            {item}
          </ControlButton>
        ))}
      </Controls>
      <TuiWindow>
        <div className={`a3s-tui-alert is-${kind}`}>
          <span>
            {kind === 'success'
              ? '✓'
              : kind === 'error'
                ? '×'
                : kind === 'warning'
                  ? '!'
                  : 'i'}
          </span>
          <div>
            <strong>{kind.toUpperCase()}</strong>
            <p>{copy[kind]}</p>
          </div>
        </div>
      </TuiWindow>
    </>
  );
}

function ProgressDemo({ locale }: DemoProps) {
  const [value, setValue] = useState(64);
  const [running, setRunning] = useState(true);

  return (
    <>
      <Controls>
        <button onClick={() => setRunning((value) => !value)} type="button">
          {running ? 'pause' : 'resume'}
        </button>
        <input
          aria-label="Progress"
          max="100"
          min="0"
          onChange={(event) => setValue(Number(event.target.value))}
          type="range"
          value={value}
        />
        <span>{value}%</span>
      </Controls>
      <TuiWindow>
        <section className="a3s-tui-progress-demo">
          <header>
            <strong>{locale === 'zh' ? '运行测试' : 'Run tests'}</strong>
            <span>{running ? 'RUNNING' : 'PAUSED'}</span>
          </header>
          <div>
            <i style={{ width: `${value}%` }} />
          </div>
          <p>
            <span>{'■'.repeat(Math.round(value / 10))}</span>
            <span>{'□'.repeat(10 - Math.round(value / 10))}</span>
            <small>{value} / 100</small>
          </p>
        </section>
      </TuiWindow>
    </>
  );
}

function ToastDemo({ locale }: DemoProps) {
  const [visible, setVisible] = useState(true);
  const [kind, setKind] = useState<'saved' | 'copied'>('saved');

  return (
    <>
      <Controls>
        <ControlButton
          active={kind === 'saved'}
          onClick={() => {
            setKind('saved');
            setVisible(true);
          }}
        >
          saved
        </ControlButton>
        <ControlButton
          active={kind === 'copied'}
          onClick={() => {
            setKind('copied');
            setVisible(true);
          }}
        >
          copied
        </ControlButton>
        <button onClick={() => setVisible((value) => !value)} type="button">
          {visible ? 'dismiss' : 'show'}
        </button>
      </Controls>
      <TuiWindow>
        <div className="a3s-tui-toast-stage">
          <p>
            {locale === 'zh'
              ? '终端内容保持可见。'
              : 'Terminal content stays visible.'}
          </p>
          {visible && (
            <output className="a3s-tui-toast">
              <span>✓</span>
              {kind === 'saved'
                ? locale === 'zh'
                  ? '会话已保存'
                  : 'Session saved'
                : locale === 'zh'
                  ? '路径已复制'
                  : 'Path copied'}
              <button onClick={() => setVisible(false)} type="button">
                ×
              </button>
            </output>
          )}
        </div>
      </TuiWindow>
    </>
  );
}

function SessionStatusDemo({ locale }: DemoProps) {
  const [mode, setMode] = useState<'code' | 'plan'>('code');
  const [context, setContext] = useState(38);

  return (
    <>
      <Controls>
        <ControlButton active={mode === 'code'} onClick={() => setMode('code')}>
          code
        </ControlButton>
        <ControlButton active={mode === 'plan'} onClick={() => setMode('plan')}>
          plan
        </ControlButton>
        <input
          aria-label="Context usage"
          max="100"
          min="0"
          onChange={(event) => setContext(Number(event.target.value))}
          type="range"
          value={context}
        />
      </Controls>
      <TuiWindow footer="">
        <div className="a3s-tui-status-stage">
          <p>
            {locale === 'zh'
              ? '当前 Session 正在运行。'
              : 'The current Session is active.'}
          </p>
          <footer>
            <span>{mode.toUpperCase()}</span>
            <span>default · medium</span>
            <span className={context > 79 ? 'is-warning' : undefined}>
              ctx {context}%
            </span>
            <span>Ctrl+T transcript · /help</span>
          </footer>
        </div>
      </TuiWindow>
    </>
  );
}

function TaskComposerDemo({ locale }: DemoProps) {
  const initial =
    locale === 'zh'
      ? '检查登录流程，修复 refresh token 失效后的恢复逻辑。'
      : 'Inspect the login flow and repair recovery after refresh-token expiry.';
  const [value, setValue] = useState(initial);
  const [mode, setMode] = useState<'Code' | 'Plan'>('Code');
  const [submitted, setSubmitted] = useState(false);

  return (
    <>
      <Controls>
        <ControlButton active={mode === 'Code'} onClick={() => setMode('Code')}>
          Code
        </ControlButton>
        <ControlButton active={mode === 'Plan'} onClick={() => setMode('Plan')}>
          Plan
        </ControlButton>
        <span>{value.length}/1200</span>
      </Controls>
      <WebPanel>
        <section className="a3s-web-composer-demo">
          <textarea
            aria-label="Task"
            onChange={(event) => {
              setValue(event.target.value);
              setSubmitted(false);
            }}
            value={value}
          />
          <footer>
            <div>
              <button type="button">+ Context</button>
              <button type="button">{mode}</button>
              <button type="button">Medium</button>
            </div>
            <button
              className="is-send"
              disabled={!value.trim()}
              onClick={() => setSubmitted(true)}
              type="button"
            >
              ↑
            </button>
          </footer>
          {submitted && (
            <output>
              {locale === 'zh' ? '任务已加入队列' : 'Task added to the queue'}
            </output>
          )}
        </section>
      </WebPanel>
    </>
  );
}

function TaskLibraryDemo({ locale }: DemoProps) {
  const tasks =
    locale === 'zh'
      ? ['修复登录会话续期', '整理发布说明', '检查 API 兼容性']
      : [
          'Repair login session renewal',
          'Prepare release notes',
          'Check API compatibility',
        ];
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(tasks[0]);
  const filtered = tasks.filter((task) =>
    task.toLowerCase().includes(query.toLowerCase()),
  );

  return (
    <>
      <Controls>
        <input
          aria-label="Search tasks"
          onChange={(event) => setQuery(event.target.value)}
          placeholder={locale === 'zh' ? '筛选任务' : 'Filter tasks'}
          value={query}
        />
        <span>{filtered.length} items</span>
      </Controls>
      <WebPanel>
        <aside className="a3s-web-library-demo">
          <header>
            <strong>{locale === 'zh' ? '任务' : 'Tasks'}</strong>
            <button type="button">+</button>
          </header>
          <button className="is-new" type="button">
            + {locale === 'zh' ? '新任务' : 'New task'}
          </button>
          <small>RECENT</small>
          {filtered.length ? (
            filtered.map((task, index) => (
              <button
                className={active === task ? 'is-active' : undefined}
                key={task}
                onClick={() => setActive(task)}
                type="button"
              >
                <i>{index === 0 ? '●' : '○'}</i>
                <span>{task}</span>
              </button>
            ))
          ) : (
            <p>{labels[locale].empty}</p>
          )}
        </aside>
      </WebPanel>
    </>
  );
}

const streamEvents = [
  ['reasoning', 'Inspect authentication call paths'],
  ['read', 'Read src/auth/session.rs'],
  ['search', 'Search refresh_token'],
  ['test', 'Run auth::session tests'],
] as const;

function ExecutionStreamDemo({ locale }: DemoProps) {
  const [count, setCount] = useState(2);

  return (
    <>
      <Controls>
        <button
          disabled={count >= streamEvents.length}
          onClick={() => setCount((value) => Math.min(value + 1, 4))}
          type="button"
        >
          next event
        </button>
        <button onClick={() => setCount(1)} type="button">
          {labels[locale].reset}
        </button>
        <span>{count}/4</span>
      </Controls>
      <WebPanel>
        <section className="a3s-web-stream-demo">
          <article>
            {locale === 'zh'
              ? '修复 refresh token 失效后的恢复逻辑。'
              : 'Repair recovery after refresh-token expiry.'}
          </article>
          {streamEvents.slice(0, count).map(([kind, text], index) => (
            <div
              className={index === count - 1 ? 'is-current' : 'is-complete'}
              key={kind}
            >
              <span>{index === count - 1 ? '◌' : '✓'}</span>
              <strong>
                {locale === 'zh'
                  ? {
                      reasoning: '检查认证调用路径',
                      read: '读取 src/auth/session.rs',
                      search: '搜索 refresh_token',
                      test: '运行 auth::session 测试',
                    }[kind]
                  : text}
              </strong>
              <small>{kind}</small>
            </div>
          ))}
        </section>
      </WebPanel>
    </>
  );
}

function ReasoningDisclosureDemo({ locale }: DemoProps) {
  const [open, setOpen] = useState(true);
  const [live, setLive] = useState(true);

  return (
    <>
      <Controls>
        <ControlButton active={open} onClick={() => setOpen((value) => !value)}>
          expanded
        </ControlButton>
        <ControlButton active={live} onClick={() => setLive((value) => !value)}>
          live
        </ControlButton>
      </Controls>
      <WebPanel>
        <section className="a3s-web-reasoning-demo">
          <button onClick={() => setOpen((value) => !value)} type="button">
            <span>◇</span>
            <span>
              <strong>
                {locale === 'zh'
                  ? '检查认证调用路径'
                  : 'Inspect authentication call paths'}
              </strong>
              <small>{live ? 'LIVE · 12s' : 'COMPLETE · 18s'}</small>
            </span>
            <i>{open ? '⌃' : '⌄'}</i>
          </button>
          {open && (
            <p>
              {locale === 'zh'
                ? '先定位 refresh token 的读取与续期分支，再确认失效状态怎样回到登录流程。'
                : 'Locate refresh-token reads and renewal branches, then trace how expiry returns to login.'}
            </p>
          )}
        </section>
      </WebPanel>
    </>
  );
}

function ToolCallTimelineDemo({ locale }: DemoProps) {
  const [status, setStatus] = useState<Status>('running');
  const [open, setOpen] = useState(true);

  return (
    <>
      <StatusControls onChange={setStatus} status={status} />
      <WebPanel>
        <section className={`a3s-web-tool-demo is-${status}`}>
          <button onClick={() => setOpen((value) => !value)} type="button">
            <span>
              {status === 'success'
                ? '✓'
                : status === 'error'
                  ? '×'
                  : status === 'running'
                    ? '◌'
                    : '·'}
            </span>
            <span>
              <strong>cargo test auth::session</strong>
              <small>bash · {labels[locale][status]}</small>
            </span>
            <i>{open ? '−' : '+'}</i>
          </button>
          {open && (
            <pre>
              {status === 'error'
                ? 'test auth::session ... FAILED\nexit code: 101'
                : status === 'success'
                  ? 'running 4 tests\n....\ntest result: ok. 4 passed'
                  : '$ cargo test auth::session\ncompiling a3s-code-core ...'}
            </pre>
          )}
        </section>
      </WebPanel>
    </>
  );
}

function PermissionDecisionDemo({ locale }: DemoProps) {
  const [decision, setDecision] = useState<'allow' | 'deny' | null>(null);

  return (
    <>
      <Controls>
        <span>decision: {decision ?? 'pending'}</span>
        <button onClick={() => setDecision(null)} type="button">
          {labels[locale].reset}
        </button>
      </Controls>
      <WebPanel>
        <section className="a3s-web-permission-demo">
          <header>
            <span>♢</span>
            <span>
              <strong>
                {locale === 'zh' ? '需要你的确认' : 'Confirmation required'}
              </strong>
              <small>
                {locale === 'zh' ? '只影响当前操作' : 'This operation only'}
              </small>
            </span>
          </header>
          <dl>
            <div>
              <dt>{locale === 'zh' ? '即将执行' : 'Operation'}</dt>
              <dd>cargo test auth::session</dd>
            </div>
            <div>
              <dt>{locale === 'zh' ? '影响范围' : 'Scope'}</dt>
              <dd>
                {locale === 'zh' ? '当前 Workspace' : 'Current workspace'}
              </dd>
            </div>
          </dl>
          {decision ? (
            <output className={`is-${decision}`}>
              {decision === 'allow'
                ? locale === 'zh'
                  ? '已允许一次'
                  : 'Allowed once'
                : locale === 'zh'
                  ? '已拒绝'
                  : 'Denied'}
            </output>
          ) : (
            <footer>
              <button onClick={() => setDecision('deny')} type="button">
                {locale === 'zh' ? '拒绝' : 'Deny'}
              </button>
              <button
                className="is-primary"
                onClick={() => setDecision('allow')}
                type="button"
              >
                {locale === 'zh' ? '允许一次' : 'Allow once'}
              </button>
            </footer>
          )}
        </section>
      </WebPanel>
    </>
  );
}

function RecoveryNoticeDemo({ locale }: DemoProps) {
  const [state, setState] = useState<'ready' | 'retrying' | 'dismissed'>(
    'ready',
  );

  return (
    <>
      <Controls>
        <button onClick={() => setState('ready')} type="button">
          {labels[locale].reset}
        </button>
        <span>{state}</span>
      </Controls>
      <WebPanel>
        <div className="a3s-web-recovery-stage">
          {state === 'dismissed' ? (
            <button onClick={() => setState('ready')} type="button">
              {locale === 'zh' ? '重新显示提示' : 'Show notice'}
            </button>
          ) : (
            <section className="a3s-web-recovery-demo">
              <span>!</span>
              <div>
                <strong>
                  {state === 'retrying'
                    ? locale === 'zh'
                      ? '正在恢复 Session…'
                      : 'Restoring Session…'
                    : locale === 'zh'
                      ? '上一次执行意外中断'
                      : 'The previous run was interrupted'}
                </strong>
                <p>
                  {locale === 'zh'
                    ? '可以从最近一次 Checkpoint 继续。'
                    : 'Continue from the latest checkpoint.'}
                </p>
              </div>
              <footer>
                <button onClick={() => setState('dismissed')} type="button">
                  {locale === 'zh' ? '忽略' : 'Dismiss'}
                </button>
                <button
                  disabled={state === 'retrying'}
                  onClick={() => setState('retrying')}
                  type="button"
                >
                  {locale === 'zh' ? '继续任务' : 'Resume'}
                </button>
              </footer>
            </section>
          )}
        </div>
      </WebPanel>
    </>
  );
}

function PlanListDemo({ locale }: DemoProps) {
  const [states, setStates] = useState<CheckState[]>(['done', 'doing', 'todo']);
  const items =
    locale === 'zh'
      ? ['定位失效分支', '更新恢复逻辑', '补充回归测试']
      : ['Locate expiry branch', 'Update recovery', 'Add regression test'];

  function advance(index: number) {
    const next: Record<CheckState, CheckState> = {
      todo: 'doing',
      doing: 'done',
      done: 'todo',
    };
    setStates((current) =>
      current.map((state, itemIndex) =>
        itemIndex === index ? next[state] : state,
      ),
    );
  }

  return (
    <>
      <Controls>
        <span>
          {states.filter((state) => state === 'done').length}/3 complete
        </span>
      </Controls>
      <WebPanel>
        <section className="a3s-web-plan-demo">
          <header>
            <strong>{locale === 'zh' ? '计划' : 'Plan'}</strong>
            <span>{states.filter((state) => state === 'done').length} / 3</span>
          </header>
          {items.map((item, index) => (
            <button
              className={`is-${states[index]}`}
              key={item}
              onClick={() => advance(index)}
              type="button"
            >
              <span>
                {states[index] === 'done'
                  ? '✓'
                  : states[index] === 'doing'
                    ? '◌'
                    : '·'}
              </span>
              <strong>{item}</strong>
              <small>{states[index]}</small>
            </button>
          ))}
        </section>
      </WebPanel>
    </>
  );
}

function SubagentListDemo({ locale }: DemoProps) {
  const [active, setActive] = useState('explorer');
  const agents = [
    { id: 'explorer', name: 'explorer', status: 'running', progress: 68 },
    { id: 'reviewer', name: 'reviewer', status: 'waiting', progress: 0 },
    { id: 'verifier', name: 'verifier', status: 'done', progress: 100 },
  ];

  return (
    <>
      <Controls>
        <span>selected: {active}</span>
      </Controls>
      <WebPanel>
        <section className="a3s-web-agent-demo">
          <header>
            <strong>{locale === 'zh' ? '子任务' : 'Subagents'}</strong>
            <span>3</span>
          </header>
          {agents.map((agent) => (
            <button
              className={active === agent.id ? 'is-active' : undefined}
              key={agent.id}
              onClick={() => setActive(agent.id)}
              type="button"
            >
              <span>{agent.name.slice(0, 1).toUpperCase()}</span>
              <span>
                <strong>{agent.name}</strong>
                <small>{agent.status}</small>
              </span>
              <i>
                <b style={{ width: `${agent.progress}%` }} />
              </i>
            </button>
          ))}
        </section>
      </WebPanel>
    </>
  );
}

function DeliverySummaryDemo({ locale }: DemoProps) {
  const [open, setOpen] = useState(true);

  return (
    <>
      <Controls>
        <ControlButton active={open} onClick={() => setOpen((value) => !value)}>
          details
        </ControlButton>
      </Controls>
      <WebPanel>
        <section className="a3s-web-delivery-demo">
          <span>✓</span>
          <small>{locale === 'zh' ? '交付摘要' : 'DELIVERY SUMMARY'}</small>
          <h4>{locale === 'zh' ? '任务已完成' : 'Task complete'}</h4>
          <p>
            {locale === 'zh'
              ? 'refresh token 失效后会重新要求登录。'
              : 'Expired refresh tokens now require a new sign-in.'}
          </p>
          <div>
            <span>✓ {locale === 'zh' ? '验证通过' : 'Verified'}</span>
            <span>2 {locale === 'zh' ? '个文件' : 'files'}</span>
            <span>4 {locale === 'zh' ? '项测试' : 'tests'}</span>
          </div>
          <button onClick={() => setOpen((value) => !value)} type="button">
            {open ? labels[locale].collapse : labels[locale].details}
          </button>
          {open && (
            <ul>
              <li>
                src/auth/session.rs <span>+12 −4</span>
              </li>
              <li>
                tests/auth_session.rs <span>+6 −2</span>
              </li>
            </ul>
          )}
        </section>
      </WebPanel>
    </>
  );
}

function ArtifactEntriesDemo({ locale }: DemoProps) {
  const artifacts = [
    { name: 'test-output.txt', type: 'TEXT', size: '4.2 KB' },
    { name: 'auth-flow.svg', type: 'IMAGE', size: '18 KB' },
    { name: 'run-trace.json', type: 'JSON', size: '31 KB' },
  ];
  const [selected, setSelected] = useState(artifacts[0]);

  return (
    <>
      <Controls>
        <span>{selected.name}</span>
      </Controls>
      <WebPanel>
        <section className="a3s-web-artifact-demo">
          <aside>
            <header>{locale === 'zh' ? '产物' : 'Artifacts'}</header>
            {artifacts.map((artifact) => (
              <button
                className={
                  selected.name === artifact.name ? 'is-active' : undefined
                }
                key={artifact.name}
                onClick={() => setSelected(artifact)}
                type="button"
              >
                <span>{artifact.type.slice(0, 2)}</span>
                <span>
                  <strong>{artifact.name}</strong>
                  <small>{artifact.size}</small>
                </span>
              </button>
            ))}
          </aside>
          <div>
            <small>{selected.type}</small>
            <strong>{selected.name}</strong>
            <code>artifact://run-01/{selected.name}</code>
          </div>
        </section>
      </WebPanel>
    </>
  );
}

function ChangesInspectorDemo({ locale }: DemoProps) {
  const [file, setFile] = useState('session.rs');
  const [view, setView] = useState<'unified' | 'split'>('unified');

  return (
    <>
      <Controls>
        <ControlButton
          active={view === 'unified'}
          onClick={() => setView('unified')}
        >
          unified
        </ControlButton>
        <ControlButton
          active={view === 'split'}
          onClick={() => setView('split')}
        >
          split
        </ControlButton>
      </Controls>
      <WebPanel>
        <section className={`a3s-web-changes-demo is-${view}`}>
          <aside>
            <header>
              <strong>{locale === 'zh' ? '改动' : 'Changes'}</strong>
              <span>+18 −6</span>
            </header>
            {['session.rs', 'auth_session.rs'].map((name, index) => (
              <button
                className={file === name ? 'is-active' : undefined}
                key={name}
                onClick={() => setFile(name)}
                type="button"
              >
                <span>RS</span>
                <strong>{name}</strong>
                <small>{index ? '+6 −2' : '+12 −4'}</small>
              </button>
            ))}
          </aside>
          <pre>
            <span>@@ -42,2 +42,5 @@</span>
            <i>- return Err(AuthError::Expired)</i>
            <b>+ session.require_login().await?;</b>
            <b>+ session.retry_refresh().await</b>
          </pre>
        </section>
      </WebPanel>
    </>
  );
}

function WorkspaceEditorDemo({ locale }: DemoProps) {
  const [active, setActive] = useState('session.rs');
  const [dirty, setDirty] = useState(false);
  const [line, setLine] = useState('session.require_login().await?;');

  return (
    <>
      <Controls>
        <span>{dirty ? 'modified' : 'saved'}</span>
        <button onClick={() => setDirty(false)} type="button">
          save
        </button>
      </Controls>
      <WebPanel>
        <section className="a3s-web-editor-demo">
          <nav>
            {['session.rs', 'auth_session.rs'].map((file) => (
              <button
                className={active === file ? 'is-active' : undefined}
                key={file}
                onClick={() => setActive(file)}
                type="button"
              >
                {file}
                {dirty && active === file ? ' •' : ''}
              </button>
            ))}
          </nav>
          <div>
            <span>43</span>
            <textarea
              aria-label={locale === 'zh' ? '代码编辑器' : 'Code editor'}
              onChange={(event) => {
                setLine(event.target.value);
                setDirty(true);
              }}
              spellCheck="false"
              value={line}
            />
          </div>
          <footer>Rust · Ln 43, Col {line.length + 1} · UTF-8</footer>
        </section>
      </WebPanel>
    </>
  );
}

function MemoryGraphDemo({ locale }: DemoProps) {
  const [mode, setMode] = useState<'graph' | 'timeline'>('graph');
  const [active, setActive] = useState('session');

  return (
    <>
      <Controls>
        <ControlButton
          active={mode === 'graph'}
          onClick={() => setMode('graph')}
        >
          graph
        </ControlButton>
        <ControlButton
          active={mode === 'timeline'}
          onClick={() => setMode('timeline')}
        >
          timeline
        </ControlButton>
      </Controls>
      <WebPanel>
        <section className={`a3s-web-memory-demo is-${mode}`}>
          {mode === 'graph' ? (
            <>
              <svg aria-label="Memory graph" viewBox="0 0 520 230">
                <path d="M95 116 240 62M95 116l151 78M240 62l178 48M246 194l172-84" />
                {[
                  ['auth', 95, 116],
                  ['session', 240, 62],
                  ['tests', 246, 194],
                  ['refresh', 418, 110],
                ].map(([id, x, y]) => (
                  <g
                    className={active === id ? 'is-active' : undefined}
                    key={id}
                    onClick={() => setActive(String(id))}
                    role="button"
                    tabIndex={0}
                    transform={`translate(${x} ${y})`}
                  >
                    <circle r="28" />
                    <text textAnchor="middle" y="4">
                      {id}
                    </text>
                  </g>
                ))}
              </svg>
              <p>
                <strong>{active}</strong>
                {locale === 'zh'
                  ? ' · 点击节点查看关联记忆'
                  : ' · select a node to inspect relations'}
              </p>
            </>
          ) : (
            <ol>
              <li>
                <span>09:14</span> Session created
              </li>
              <li>
                <span>09:16</span> Procedure recalled
              </li>
              <li>
                <span>09:21</span> Verification saved
              </li>
            </ol>
          )}
        </section>
      </WebPanel>
    </>
  );
}

const demos: Record<string, (props: DemoProps) => ReactNode> = {
  ActivityBlock: ActivityBlockDemo,
  Alert: AlertDemo,
  ArtifactEntries: ArtifactEntriesDemo,
  ChangesInspector: ChangesInspectorDemo,
  Checklist: ChecklistDemo,
  ChoicePrompt: ChoicePromptDemo,
  Confirm: ConfirmDemo,
  DataTable: DataTableDemo,
  DeliverySummary: DeliverySummaryDemo,
  DiffView: DiffViewDemo,
  ExecutionStream: ExecutionStreamDemo,
  MemoryGraph: MemoryGraphDemo,
  PermissionDecision: PermissionDecisionDemo,
  Progress: ProgressDemo,
  ReasoningDisclosure: ReasoningDisclosureDemo,
  RecoveryNotice: RecoveryNoticeDemo,
  SessionStatus: SessionStatusDemo,
  TaskComposer: TaskComposerDemo,
  TaskLibrary: TaskLibraryDemo,
  TaskRuntimePlanList: PlanListDemo,
  TaskRuntimeSubagentList: SubagentListDemo,
  Tabs: TabsDemo,
  TextInput: TextInputDemo,
  Toast: ToastDemo,
  ToolCallTimeline: ToolCallTimelineDemo,
  ToolLogView: ToolLogViewDemo,
  TreePicker: TreePickerDemo,
  WorkspaceEditor: WorkspaceEditorDemo,
};

export default function A3SComponentDemo({ name }: { name: string }) {
  const locale: Locale = useLang() === 'zh' ? 'zh' : 'en';
  const Demo = demos[name];
  const surface = tuiNames.has(name) ? 'TUI' : 'WEB';

  if (!Demo) {
    return (
      <p className="a3s-component-demo-missing">
        Unknown component: <code>{name}</code>
      </p>
    );
  }

  return (
    <section
      className={`a3s-component-demo is-${surface.toLowerCase()}`}
      data-component={name}
    >
      <header className="a3s-component-demo-header">
        <span>
          <i aria-hidden="true" />
          {labels[locale].live}
        </span>
        <strong>{name}</strong>
        <small>{surface}</small>
      </header>
      <div className="a3s-component-demo-stage">
        <Demo locale={locale} />
      </div>
      <footer>
        <span>STATE / LOCAL</span>
        <span>{labels[locale].local}</span>
      </footer>
    </section>
  );
}
