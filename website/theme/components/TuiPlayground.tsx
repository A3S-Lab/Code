import { type CSSProperties, useState } from 'react';
import { useLang } from '@rspress/core/runtime';

type Locale = 'zh' | 'en';
type SceneId = 'session' | 'diff' | 'workspace' | 'components';
type ThemeId = 'dark' | 'light' | 'catppuccin' | 'tokyo-night';

type Theme = {
  id: ThemeId;
  name: string;
  palette: {
    bg: string;
    panel: string;
    panelStrong: string;
    text: string;
    muted: string;
    line: string;
    blue: string;
    green: string;
    amber: string;
    red: string;
    purple: string;
  };
};

const themes: Theme[] = [
  {
    id: 'dark',
    name: 'Dark',
    palette: {
      bg: '#090b10',
      panel: '#0f1218',
      panelStrong: '#171b24',
      text: '#e8edf4',
      muted: '#7f8999',
      line: '#29303c',
      blue: '#6ca3ff',
      green: '#46d39a',
      amber: '#efb354',
      red: '#ef767a',
      purple: '#b493ff',
    },
  },
  {
    id: 'light',
    name: 'Light',
    palette: {
      bg: '#f4f5f7',
      panel: '#ffffff',
      panelStrong: '#e9ebef',
      text: '#1d222b',
      muted: '#687180',
      line: '#d5d9df',
      blue: '#2465d8',
      green: '#16845f',
      amber: '#a8650c',
      red: '#c33f48',
      purple: '#6f50bf',
    },
  },
  {
    id: 'catppuccin',
    name: 'Catppuccin',
    palette: {
      bg: '#1e1e2e',
      panel: '#181825',
      panelStrong: '#313244',
      text: '#cdd6f4',
      muted: '#9399b2',
      line: '#45475a',
      blue: '#89b4fa',
      green: '#a6e3a1',
      amber: '#f9e2af',
      red: '#f38ba8',
      purple: '#cba6f7',
    },
  },
  {
    id: 'tokyo-night',
    name: 'Tokyo Night',
    palette: {
      bg: '#16161e',
      panel: '#1a1b26',
      panelStrong: '#24283b',
      text: '#c0caf5',
      muted: '#787c99',
      line: '#3b4261',
      blue: '#7aa2f7',
      green: '#9ece6a',
      amber: '#e0af68',
      red: '#f7768e',
      purple: '#bb9af7',
    },
  },
];

const copy = {
  zh: {
    aria: 'A3S TUI 组件预览',
    themes: '主题',
    scenes: {
      session: '执行',
      diff: '代码改动',
      workspace: 'Workspace',
      components: '组件',
    },
    demo: '文档预览',
    noBackend: '这里演示组件状态，不会运行命令或连接模型。',
    taskTitle: '修复登录会话续期',
    userPrompt:
      '检查登录流程，修复 refresh token 失效后没有重新登录的问题，并补上测试。',
    thinking: '正在检查 session 与 token 的调用路径',
    readDone: '已读取 src/auth/session.rs',
    searchDone: '找到 4 处 refresh_token 引用',
    testRunning: '正在运行 auth::session 测试',
    checklist: '任务清单',
    items: ['定位失效分支', '更新会话恢复逻辑', '补充回归测试'],
    changed: '2 个文件已修改',
    diffSummary: '+18  -6',
    workspaceTitle: '项目文件',
    previewTitle: 'session.rs',
    componentsTitle: '同一框架中的组件',
    componentsHint: '选择一项查看终端组件如何组合。',
    status: 'default · medium',
    keys: 'Ctrl+T transcript   /help commands   Esc close',
  },
  en: {
    aria: 'A3S TUI component preview',
    themes: 'Themes',
    scenes: {
      session: 'Run',
      diff: 'Changes',
      workspace: 'Workspace',
      components: 'Components',
    },
    demo: 'Docs preview',
    noBackend:
      'This demonstrates component states; it does not run commands or connect to a model.',
    taskTitle: 'Repair login session renewal',
    userPrompt:
      'Inspect the login flow, fix re-authentication after a refresh token expires, and add tests.',
    thinking: 'Inspecting session and token call paths',
    readDone: 'Read src/auth/session.rs',
    searchDone: 'Found 4 refresh_token references',
    testRunning: 'Running auth::session tests',
    checklist: 'Checklist',
    items: [
      'Locate the expiry branch',
      'Update session recovery',
      'Add a regression test',
    ],
    changed: '2 files changed',
    diffSummary: '+18  -6',
    workspaceTitle: 'Project files',
    previewTitle: 'session.rs',
    componentsTitle: 'Components in the same framework',
    componentsHint:
      'Select an item to see how terminal components fit together.',
    status: 'default · medium',
    keys: 'Ctrl+T transcript   /help commands   Esc close',
  },
};

const componentSets = [
  {
    name: 'Navigation',
    components: 'MenuPanel · Tabs · Tree · TreePicker · Breadcrumb',
    preview: 'menu',
  },
  {
    name: 'Agent activity',
    components:
      'ActivityBlock · Checklist · TaskQueue · SubagentTracker · Timeline',
    preview: 'activity',
  },
  {
    name: 'Tools & output',
    components:
      'ToolLogView · ToolStatusLine · DiffView · OutputBlock · LogView',
    preview: 'tools',
  },
  {
    name: 'Input & feedback',
    components: 'PromptLine · Textarea · ChoicePrompt · Confirm · Toast',
    preview: 'input',
  },
];

function terminalStyle(theme: Theme): CSSProperties {
  return {
    '--tui-bg': theme.palette.bg,
    '--tui-panel': theme.palette.panel,
    '--tui-panel-strong': theme.palette.panelStrong,
    '--tui-text': theme.palette.text,
    '--tui-muted': theme.palette.muted,
    '--tui-line': theme.palette.line,
    '--tui-blue': theme.palette.blue,
    '--tui-green': theme.palette.green,
    '--tui-amber': theme.palette.amber,
    '--tui-red': theme.palette.red,
    '--tui-purple': theme.palette.purple,
  } as CSSProperties;
}

export default function TuiPlayground() {
  const locale: Locale = useLang() === 'zh' ? 'zh' : 'en';
  const labels = copy[locale];
  const [themeId, setThemeId] = useState<ThemeId>('dark');
  const [scene, setScene] = useState<SceneId>('session');
  const [checked, setChecked] = useState([true, true, false]);
  const [componentIndex, setComponentIndex] = useState(1);
  const theme = themes.find((item) => item.id === themeId) ?? themes[0];

  const toggleCheck = (index: number) => {
    setChecked((current) =>
      current.map((value, itemIndex) => (itemIndex === index ? !value : value)),
    );
  };

  return (
    <section
      className="a3s-playground a3s-tui-playground"
      aria-label={labels.aria}
    >
      <header className="a3s-playground-toolbar">
        <div>
          <span className="a3s-playground-live" aria-hidden="true" />
          <strong>A3S TUI</strong>
          <small>{labels.demo}</small>
        </div>
        <div className="a3s-playground-theme" aria-label={labels.themes}>
          {themes.map((item) => (
            <button
              aria-label={`${labels.themes}: ${item.name}`}
              aria-pressed={theme.id === item.id}
              key={item.id}
              onClick={() => setThemeId(item.id)}
              title={item.name}
              type="button"
            >
              <span
                style={{
                  background: `linear-gradient(135deg, ${item.palette.bg} 50%, ${item.palette.blue} 50%)`,
                }}
              />
            </button>
          ))}
        </div>
      </header>

      <div
        className="a3s-playground-tabs"
        role="tablist"
        aria-label={labels.aria}
      >
        {(Object.keys(labels.scenes) as SceneId[]).map((id) => (
          <button
            aria-selected={scene === id}
            key={id}
            onClick={() => setScene(id)}
            role="tab"
            type="button"
          >
            {labels.scenes[id]}
          </button>
        ))}
      </div>

      <div className="a3s-tui-frame" style={terminalStyle(theme)}>
        <div className="a3s-tui-titlebar">
          <span>A3S CODE</span>
          <span>~/workspaces/a3s-code</span>
          <span>80 × 24</span>
        </div>

        <div className="a3s-tui-screen">
          <aside className="a3s-tui-sidebar">
            <strong>CODE</strong>
            <button className="is-active" type="button">
              <span>›</span> {locale === 'zh' ? '当前任务' : 'Active task'}
            </button>
            <button type="button">
              <span> </span> {locale === 'zh' ? '会话' : 'Sessions'}{' '}
              <small>12</small>
            </button>
            <button type="button">
              <span> </span> {locale === 'zh' ? '任务' : 'Tasks'}{' '}
              <small>3</small>
            </button>
            <button type="button">
              <span> </span> {locale === 'zh' ? '记忆' : 'Memory'}
            </button>
            <div />
            <button type="button">
              <span> </span> {locale === 'zh' ? '设置' : 'Settings'}
            </button>
          </aside>

          <main className="a3s-tui-main">
            {scene === 'session' && (
              <div className="a3s-tui-session">
                <header>
                  <div>
                    <span className="a3s-tui-dot" />
                    <strong>{labels.taskTitle}</strong>
                  </div>
                  <small>00:42</small>
                </header>
                <div className="a3s-tui-user">
                  <span>YOU</span>
                  <p>{labels.userPrompt}</p>
                </div>
                <div className="a3s-tui-activity">
                  <p className="is-running">
                    <span>◆</span> {labels.thinking}
                  </p>
                  <p>
                    <span>✓</span> {labels.readDone}
                  </p>
                  <p>
                    <span>✓</span> {labels.searchDone}
                  </p>
                  <p className="is-running">
                    <span>◌</span> {labels.testRunning}
                  </p>
                </div>
                <section className="a3s-tui-checklist">
                  <strong>{labels.checklist}</strong>
                  {labels.items.map((item, index) => (
                    <button
                      key={item}
                      onClick={() => toggleCheck(index)}
                      type="button"
                    >
                      <span>{checked[index] ? '✓' : '·'}</span>
                      <span
                        className={checked[index] ? 'is-checked' : undefined}
                      >
                        {item}
                      </span>
                    </button>
                  ))}
                </section>
              </div>
            )}

            {scene === 'diff' && (
              <div className="a3s-tui-diff">
                <header>
                  <strong>DIFF VIEW</strong>
                  <span>{labels.changed}</span>
                  <small>{labels.diffSummary}</small>
                </header>
                <div className="a3s-tui-file-heading">
                  <span>▾</span>
                  <strong>src/auth/session.rs</strong>
                  <small>+12 -4</small>
                </div>
                <pre aria-label="session.rs diff">
                  <span className="line neutral">
                    <i>42</i> {'  '}match refresh_token(state).await {'{'}
                  </span>
                  <span className="line removed">
                    <i>43</i>- {'    '}Err(_) =&gt; return
                    Err(AuthError::Expired),
                  </span>
                  <span className="line added">
                    <i>43</i>+ {'    '}Err(AuthError::Expired) =&gt; {'{'}
                  </span>
                  <span className="line added">
                    <i>44</i>+ {'      '}session.require_login().await?;
                  </span>
                  <span className="line added">
                    <i>45</i>+ {'      '}session.retry_refresh().await
                  </span>
                  <span className="line added">
                    <i>46</i>+ {'    }\n'}
                  </span>
                  <span className="line neutral">
                    <i>47</i> {'    '}result =&gt; result,
                  </span>
                  <span className="line neutral">
                    <i>48</i> {'  }'}
                  </span>
                </pre>
                <div className="a3s-tui-file-heading">
                  <span>›</span>
                  <strong>tests/auth_session.rs</strong>
                  <small>+6 -2</small>
                </div>
              </div>
            )}

            {scene === 'workspace' && (
              <div className="a3s-tui-workspace">
                <section>
                  <header>{labels.workspaceTitle}</header>
                  <p>
                    <span>▾</span> a3s-code
                  </p>
                  <p>
                    <span>│ ▾</span> src
                  </p>
                  <p className="is-active">
                    <span>│ │</span> session.rs
                  </p>
                  <p>
                    <span>│ │</span> permissions.rs
                  </p>
                  <p>
                    <span>│ └</span> tools.rs
                  </p>
                  <p>
                    <span>├ ▸</span> tests
                  </p>
                  <p>
                    <span>└</span> Cargo.toml
                  </p>
                </section>
                <section>
                  <header>
                    {labels.previewTitle}
                    <small>Rust</small>
                  </header>
                  <pre>
                    <span>
                      <i>1</i>
                      <b>pub async fn</b> refresh(
                    </span>
                    <span>
                      <i>2</i>
                      {'  '}session: &amp;<em>mut Session</em>,
                    </span>
                    <span>
                      <i>3</i>) -&gt; Result&lt;Token&gt; {'{'}
                    </span>
                    <span className="is-highlighted">
                      <i>4</i>
                      {'  '}session.ensure_active().await?;
                    </span>
                    <span>
                      <i>5</i>
                      {'  '}session.token().await
                    </span>
                    <span>
                      <i>6</i>
                      {'\u007d'}
                    </span>
                  </pre>
                </section>
              </div>
            )}

            {scene === 'components' && (
              <div className="a3s-tui-components">
                <header>
                  <strong>{labels.componentsTitle}</strong>
                  <small>{labels.componentsHint}</small>
                </header>
                <div>
                  <nav>
                    {componentSets.map((set, index) => (
                      <button
                        className={
                          componentIndex === index ? 'is-active' : undefined
                        }
                        key={set.name}
                        onClick={() => setComponentIndex(index)}
                        type="button"
                      >
                        <span>{componentIndex === index ? '›' : ' '}</span>
                        {set.name}
                      </button>
                    ))}
                  </nav>
                  <section>
                    <strong>{componentSets[componentIndex].name}</strong>
                    <p>{componentSets[componentIndex].components}</p>
                    <div
                      className={`a3s-tui-mini ${componentSets[componentIndex].preview}`}
                    >
                      <span>READY</span>
                      <span>■■■■■■□□ 75%</span>
                      <span>✓ workspace checked</span>
                      <span>› Continue</span>
                    </div>
                  </section>
                </div>
              </div>
            )}
          </main>
        </div>

        <footer className="a3s-tui-status">
          <span>PLAN</span>
          <span>{labels.status}</span>
          <span>ctx 38%</span>
          <span>{labels.keys}</span>
        </footer>
      </div>

      <p className="a3s-playground-note">{labels.noBackend}</p>
    </section>
  );
}
