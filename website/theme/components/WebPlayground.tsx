import { useState } from 'react';
import { useLang } from '@rspress/core/runtime';

type Locale = 'zh' | 'en';
type ViewId = 'compose' | 'run' | 'permission' | 'result';
type Decision = 'allowed' | 'denied' | null;

const copy = {
  zh: {
    aria: 'A3S Web 任务流程预览',
    demo: '文档预览',
    noBackend:
      '这里演示 A3S Web 的组件与状态切换，不会发送请求、修改文件或连接模型。',
    tabs: {
      compose: '准备',
      run: '执行',
      permission: '确认',
      result: '结果',
    },
    taskLibrary: '任务',
    newTask: '新任务',
    sessions: ['修复登录会话续期', '整理发布说明', '检查 API 兼容性'],
    startTitle: '今天要处理什么？',
    startHint: '描述任务，也可以加入文件、Skill 和 Workspace。',
    prompt:
      '检查登录流程，修复 refresh token 失效后没有重新登录的问题，并补上测试。',
    attach: '添加上下文',
    mode: 'Code',
    effort: 'Medium',
    send: '运行演示',
    runningTitle: '修复登录会话续期',
    runningHint: 'A3S Code 正在处理当前 Workspace',
    instruction:
      '检查登录流程，修复 refresh token 失效后没有重新登录的问题，并补上测试。',
    reasoningTitle: '检查认证调用路径',
    reasoning:
      '先定位 refresh token 的读取与续期分支，再检查失效状态如何回到登录流程。',
    toolRead: '读取 src/auth/session.rs',
    toolSearch: '搜索 refresh_token',
    toolTest: '运行 auth::session 测试',
    plan: '计划',
    planItems: ['定位失效分支', '更新会话恢复逻辑', '补充回归测试'],
    nextPermission: '查看确认组件',
    permissionTitle: '需要你的确认',
    permissionHint: '只影响当前这一次操作',
    operation: '即将执行',
    operationValue: '运行 cargo test auth::session',
    reason: '为什么需要',
    reasonValue: '该命令会启动本地进程，当前权限规则要求先确认。',
    scope: '影响范围',
    scopeValue: '当前任务的 Workspace',
    risk: '需要注意',
    riskValue: '测试可能创建临时构建文件，不会访问 Workspace 之外的路径。',
    deny: '拒绝',
    allow: '允许一次',
    denied: '已拒绝，本次操作不会执行。',
    reset: '重新演示',
    resultTitle: '任务已完成',
    resultHint: '登录会话现在会在 refresh token 失效后要求重新登录。',
    verified: '验证通过',
    filesChanged: '2 个文件已修改',
    tests: '4 项测试通过',
    openChanges: '查看改动',
    delivery: '交付摘要',
    changedFiles: ['src/auth/session.rs', 'tests/auth_session.rs'],
  },
  en: {
    aria: 'A3S Web task-flow preview',
    demo: 'Docs preview',
    noBackend:
      'This demonstrates A3S Web components and state changes; it does not send requests, edit files, or connect to a model.',
    tabs: {
      compose: 'Prepare',
      run: 'Run',
      permission: 'Confirm',
      result: 'Result',
    },
    taskLibrary: 'Tasks',
    newTask: 'New task',
    sessions: [
      'Repair login session renewal',
      'Prepare release notes',
      'Check API compatibility',
    ],
    startTitle: 'What would you like to work on?',
    startHint:
      'Describe the task and optionally add files, Skills, and a Workspace.',
    prompt:
      'Inspect the login flow, fix re-authentication after a refresh token expires, and add tests.',
    attach: 'Add context',
    mode: 'Code',
    effort: 'Medium',
    send: 'Run demo',
    runningTitle: 'Repair login session renewal',
    runningHint: 'A3S Code is working in the current Workspace',
    instruction:
      'Inspect the login flow, fix re-authentication after a refresh token expires, and add tests.',
    reasoningTitle: 'Inspect authentication call paths',
    reasoning:
      'First locate refresh-token reads and renewal branches, then trace how an expired state returns to login.',
    toolRead: 'Read src/auth/session.rs',
    toolSearch: 'Search refresh_token',
    toolTest: 'Run auth::session tests',
    plan: 'Plan',
    planItems: [
      'Locate the expiry branch',
      'Update session recovery',
      'Add a regression test',
    ],
    nextPermission: 'Open confirmation',
    permissionTitle: 'Your confirmation is required',
    permissionHint: 'This applies only to this operation',
    operation: 'About to run',
    operationValue: 'cargo test auth::session',
    reason: 'Why this is needed',
    reasonValue:
      'This command starts a local process, so the current permission rules require confirmation.',
    scope: 'Scope',
    scopeValue: 'The current task Workspace',
    risk: 'What to know',
    riskValue:
      'The test may create temporary build files, but it cannot access paths outside the Workspace.',
    deny: 'Deny',
    allow: 'Allow once',
    denied: 'Denied. This operation will not run.',
    reset: 'Start over',
    resultTitle: 'Task complete',
    resultHint:
      'The login session now asks the user to sign in again after the refresh token expires.',
    verified: 'Verified',
    filesChanged: '2 files changed',
    tests: '4 tests passed',
    openChanges: 'Review changes',
    delivery: 'Delivery summary',
    changedFiles: ['src/auth/session.rs', 'tests/auth_session.rs'],
  },
};

function BrandMark() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24">
      <path d="m12 3 8 4.6v8.8L12 21l-8-4.6V7.6L12 3Z" />
      <path d="m8 14 4-7 4 7M9.4 11.7h5.2" />
    </svg>
  );
}

export default function WebPlayground() {
  const locale: Locale = useLang() === 'zh' ? 'zh' : 'en';
  const labels = copy[locale];
  const [view, setView] = useState<ViewId>('compose');
  const [decision, setDecision] = useState<Decision>(null);
  const [prompt, setPrompt] = useState(labels.prompt);

  const restart = () => {
    setDecision(null);
    setView('compose');
    setPrompt(labels.prompt);
  };

  const allow = () => {
    setDecision('allowed');
    setView('result');
  };

  return (
    <section
      className="a3s-playground a3s-web-playground"
      aria-label={labels.aria}
    >
      <header className="a3s-playground-toolbar">
        <div>
          <span className="a3s-playground-live" aria-hidden="true" />
          <strong>A3S Web</strong>
          <small>{labels.demo}</small>
        </div>
        <nav className="a3s-web-steps" aria-label={labels.aria}>
          {(Object.keys(labels.tabs) as ViewId[]).map((id, index) => (
            <button
              aria-current={view === id ? 'step' : undefined}
              className={view === id ? 'is-active' : undefined}
              key={id}
              onClick={() => setView(id)}
              type="button"
            >
              <span>{index + 1}</span>
              {labels.tabs[id]}
            </button>
          ))}
        </nav>
      </header>

      <div className="a3s-web-frame">
        <aside className="a3s-web-activity" aria-label="A3S Web">
          <div className="a3s-web-brand">
            <BrandMark />
          </div>
          <button
            className="is-active"
            aria-label={labels.taskLibrary}
            type="button"
          >
            <span>⌁</span>
          </button>
          <button aria-label="Workspace" type="button">
            <span>▱</span>
          </button>
          <button aria-label="Memory" type="button">
            <span>◇</span>
          </button>
          <button aria-label="Settings" type="button">
            <span>⚙</span>
          </button>
        </aside>

        <aside className="a3s-web-library">
          <header>
            <strong>{labels.taskLibrary}</strong>
            <button onClick={restart} type="button">
              +
            </button>
          </header>
          <button className="a3s-web-new-task" onClick={restart} type="button">
            <span>+</span>
            {labels.newTask}
          </button>
          <small>{locale === 'zh' ? '最近' : 'RECENT'}</small>
          {labels.sessions.map((session, index) => (
            <button
              className={
                index === 0 && view !== 'compose' ? 'is-active' : undefined
              }
              key={session}
              type="button"
            >
              <span>{index === 0 && view !== 'compose' ? '●' : '○'}</span>
              <span>{session}</span>
            </button>
          ))}
        </aside>

        <main className="a3s-web-main">
          {view === 'compose' && (
            <div className="a3s-web-compose">
              <div className="a3s-web-welcome">
                <span className="a3s-web-welcome-mark">
                  <BrandMark />
                </span>
                <h3>{labels.startTitle}</h3>
                <p>{labels.startHint}</p>
              </div>
              <div className="a3s-web-composer">
                <textarea
                  aria-label={labels.startTitle}
                  onChange={(event) => setPrompt(event.target.value)}
                  rows={4}
                  value={prompt}
                />
                <footer>
                  <div>
                    <button type="button">+ {labels.attach}</button>
                    <button type="button">{labels.mode}⌄</button>
                    <button type="button">{labels.effort}⌄</button>
                  </div>
                  <button
                    aria-label={labels.send}
                    className="a3s-web-send"
                    disabled={!prompt.trim()}
                    onClick={() => setView('run')}
                    type="button"
                  >
                    ↑
                  </button>
                </footer>
              </div>
              <button
                className="a3s-web-demo-action"
                disabled={!prompt.trim()}
                onClick={() => setView('run')}
                type="button"
              >
                {labels.send}
              </button>
            </div>
          )}

          {view === 'run' && (
            <div className="a3s-web-task">
              <header className="a3s-web-task-header">
                <div>
                  <span className="a3s-web-task-icon">⌁</span>
                  <span>
                    <strong>{labels.runningTitle}</strong>
                    <small>{labels.runningHint}</small>
                  </span>
                </div>
                <span className="a3s-web-running">
                  <i /> {locale === 'zh' ? '执行中' : 'Running'}
                </span>
              </header>
              <div className="a3s-web-task-body">
                <section className="a3s-web-stream">
                  <article className="a3s-web-instruction">
                    {labels.instruction}
                  </article>
                  <article className="a3s-web-reasoning">
                    <header>
                      <span>◇</span>
                      <strong>{labels.reasoningTitle}</strong>
                      <small>12s</small>
                    </header>
                    <p>{labels.reasoning}</p>
                  </article>
                  <div className="a3s-web-tool-call is-complete">
                    <span>✓</span>
                    <strong>{labels.toolRead}</strong>
                    <small>session.rs</small>
                  </div>
                  <div className="a3s-web-tool-call is-complete">
                    <span>✓</span>
                    <strong>{labels.toolSearch}</strong>
                    <small>4 {locale === 'zh' ? '处结果' : 'matches'}</small>
                  </div>
                  <div className="a3s-web-tool-call is-running">
                    <span>◌</span>
                    <strong>{labels.toolTest}</strong>
                    <small>cargo test</small>
                  </div>
                  <button
                    className="a3s-web-demo-action"
                    onClick={() => {
                      setDecision(null);
                      setView('permission');
                    }}
                    type="button"
                  >
                    {labels.nextPermission}
                  </button>
                </section>
                <aside className="a3s-web-plan">
                  <header>
                    <strong>{labels.plan}</strong>
                    <span>2 / 3</span>
                  </header>
                  {labels.planItems.map((item, index) => (
                    <p
                      className={index < 2 ? 'is-complete' : 'is-running'}
                      key={item}
                    >
                      <span>{index < 2 ? '✓' : '◌'}</span>
                      {item}
                    </p>
                  ))}
                </aside>
              </div>
            </div>
          )}

          {view === 'permission' && (
            <div className="a3s-web-task">
              <header className="a3s-web-task-header">
                <div>
                  <span className="a3s-web-task-icon">⌁</span>
                  <span>
                    <strong>{labels.runningTitle}</strong>
                    <small>{labels.runningHint}</small>
                  </span>
                </div>
              </header>
              <div className="a3s-web-permission-wrap">
                <section className="a3s-web-permission">
                  <header>
                    <span>♢</span>
                    <span>
                      <strong>{labels.permissionTitle}</strong>
                      <small>{labels.permissionHint}</small>
                    </span>
                  </header>
                  <dl>
                    <div>
                      <dt>{labels.operation}</dt>
                      <dd>{labels.operationValue}</dd>
                    </div>
                    <div>
                      <dt>{labels.reason}</dt>
                      <dd>{labels.reasonValue}</dd>
                    </div>
                    <div>
                      <dt>{labels.scope}</dt>
                      <dd>{labels.scopeValue}</dd>
                    </div>
                    <div>
                      <dt>{labels.risk}</dt>
                      <dd>{labels.riskValue}</dd>
                    </div>
                  </dl>
                  {decision === 'denied' ? (
                    <output>
                      {labels.denied}
                      <button
                        onClick={() => {
                          setDecision(null);
                        }}
                        type="button"
                      >
                        {labels.reset}
                      </button>
                    </output>
                  ) : (
                    <footer>
                      <button
                        onClick={() => {
                          setDecision('denied');
                        }}
                        type="button"
                      >
                        {labels.deny}
                      </button>
                      <button
                        className="is-primary"
                        onClick={allow}
                        type="button"
                      >
                        {labels.allow}
                      </button>
                    </footer>
                  )}
                </section>
              </div>
            </div>
          )}

          {view === 'result' && (
            <div className="a3s-web-result">
              <div className="a3s-web-result-mark">✓</div>
              <span className="a3s-web-result-label">{labels.delivery}</span>
              <h3>{labels.resultTitle}</h3>
              <p>{labels.resultHint}</p>
              <div className="a3s-web-result-stats">
                <span>
                  <i>✓</i> {labels.verified}
                </span>
                <span>{labels.filesChanged}</span>
                <span>{labels.tests}</span>
              </div>
              <section className="a3s-web-changes">
                <header>
                  <strong>{labels.openChanges}</strong>
                  <small>+18 −6</small>
                </header>
                {labels.changedFiles.map((file, index) => (
                  <p key={file}>
                    <span>{index === 0 ? 'RS' : 'TS'}</span>
                    <strong>{file}</strong>
                    <small>{index === 0 ? '+12 −4' : '+6 −2'}</small>
                  </p>
                ))}
              </section>
              <button
                className="a3s-web-demo-action"
                onClick={restart}
                type="button"
              >
                {labels.reset}
              </button>
            </div>
          )}
        </main>
      </div>

      <p className="a3s-playground-note">{labels.noBackend}</p>
    </section>
  );
}
