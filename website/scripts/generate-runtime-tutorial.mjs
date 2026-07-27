import { mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { highlight } from 'codehike/code';
import { format } from 'prettier';

const here = path.dirname(fileURLToPath(import.meta.url));
const outputPath = path.join(
  here,
  '..',
  'theme',
  'generated',
  'runtime-tutorial.json',
);

const theme = {
  name: 'a3s-runtime',
  type: 'dark',
  colors: {
    'editor.background': '#0b0d11',
    'editor.foreground': '#c9ced8',
    'editorLineNumber.foreground': '#444b57',
    'editorLineNumber.activeForeground': '#8c96a7',
    'editor.selectionBackground': '#233653',
  },
  tokenColors: [
    {
      scope: ['comment', 'punctuation.definition.comment'],
      settings: { foreground: '#626b78', fontStyle: 'italic' },
    },
    {
      scope: [
        'keyword',
        'storage',
        'storage.type',
        'storage.modifier',
        'keyword.control',
      ],
      settings: { foreground: '#91acd8' },
    },
    {
      scope: ['entity.name.function', 'support.function', 'meta.function-call'],
      settings: { foreground: '#dfbd7c' },
    },
    {
      scope: [
        'entity.name.type',
        'entity.name.class',
        'support.type',
        'support.class',
      ],
      settings: { foreground: '#b5a4d2' },
    },
    {
      scope: ['string', 'string.quoted', 'string.template'],
      settings: { foreground: '#91c4a6' },
    },
    {
      scope: ['constant.numeric', 'constant.language', 'constant.character'],
      settings: { foreground: '#d79d77' },
    },
    {
      scope: [
        'variable.parameter',
        'variable.other',
        'meta.object-literal.key',
      ],
      settings: { foreground: '#cbd1da' },
    },
    {
      scope: ['punctuation', 'meta.brace', 'meta.delimiter'],
      settings: { foreground: '#7a8492' },
    },
    {
      scope: ['keyword.operator', 'operator'],
      settings: { foreground: '#7ea8b8' },
    },
  ],
};

const steps = [
  {
    id: 'surfaces',
    layer: 'L01 / SURFACES',
    filename: 'src/main.rs',
    title: {
      zh: '先选一个接入入口',
      en: 'Choose an entry point',
    },
    body: {
      zh: '终端可以直接跑；要嵌进产品，就从 Rust、Node.js 或 Python SDK 开始。下面用 Rust 把完整路径走一遍。',
      en: 'Run the terminal app directly, or embed the Rust, Node.js, or Python SDK. The walkthrough below follows the Rust path.',
    },
    note: {
      zh: '同一个 Runtime，不同的产品外壳。',
      en: 'One runtime behind different product surfaces.',
    },
    tags: ['Terminal', 'Rust', 'Node.js', 'Python'],
    focus: [1, 6],
    code: `use a3s_code_core::{Agent, AgentEvent};

#[tokio::main]
async fn main() -> a3s_code_core::Result<()> {
    Ok(())
}`,
  },
  {
    id: 'session',
    layer: 'L02 / AGENT API',
    filename: 'src/main.rs',
    title: {
      zh: '把 Agent 绑到一个项目',
      en: 'Bind the agent to a project',
    },
    body: {
      zh: 'Agent 读取 ACL 和共享能力；AgentSession 负责当前 Workspace 与这一段对话。异步 build 会在第一轮开始前准备好相关资源。',
      en: 'Agent loads ACL and shared capabilities. AgentSession owns the current workspace and conversation, resolving resources before the first turn.',
    },
    note: {
      zh: 'Agent 可以复用，Session 与 Workspace 一一对应。',
      en: 'Reuse the Agent; bind each Session to a workspace.',
    },
    tags: ['Agent', 'AgentSession', 'Workspace'],
    focus: [5, 8],
    code: `use a3s_code_core::{Agent, AgentEvent};

#[tokio::main]
async fn main() -> a3s_code_core::Result<()> {
    let agent = Agent::new("agent.acl").await?;
    let session = agent
        .session_builder(".")
        .build()
        .await?;

    Ok(())
}`,
  },
  {
    id: 'context',
    layer: 'L03 / INTELLIGENCE',
    filename: 'src/main.rs',
    title: {
      zh: '给上下文设好边界',
      en: 'Put a boundary around context',
    },
    body: {
      zh: '自动压缩、token 上限和触发阈值属于 Session 配置，不需要藏进界面层。ContextAssembler 与 Memory 会据此准备模型输入。',
      en: 'Compaction, token limits, and thresholds belong to SessionOptions rather than UI code. ContextAssembler and memory use them when preparing model input.',
    },
    note: {
      zh: '大结果会变成 Artifact，模型只拿到受控预览。',
      en: 'Large results become artifacts; the model receives a bounded preview.',
    },
    tags: ['ContextAssembler', 'Memory', 'LlmClient'],
    focus: [6, 10],
    code: `use a3s_code_core::{Agent, AgentEvent, SessionOptions};

#[tokio::main]
async fn main() -> a3s_code_core::Result<()> {
    let options = SessionOptions::new()
        .with_auto_compact(true)
        .with_max_context_tokens(200_000)
        .with_auto_compact_threshold(0.8);

    let agent = Agent::new("agent.acl").await?;
    let session = agent
        .session_builder(".")
        .options(options)
        .build()
        .await?;

    Ok(())
}`,
  },
  {
    id: 'governance',
    layer: 'L04 / GOVERNANCE',
    filename: 'src/main.rs',
    title: {
      zh: '在工具执行前定规则',
      en: 'Set policy before tools run',
    },
    body: {
      zh: 'read 自动放行，write 每次询问，危险的 shell 命令直接拒绝。规则按 deny、allow、ask 的顺序求值。',
      en: 'Allow reads, ask before writes, and deny a dangerous shell pattern. Rules evaluate in deny, allow, then ask order.',
    },
    note: {
      zh: '权限、确认、预算与沙箱共用一条执行链。',
      en: 'Permission, approval, budgets, and sandboxing share one path.',
    },
    tags: ['validate', 'permission', 'confirm', 'sandbox'],
    focus: [8, 12],
    code: `use a3s_code_core::{
    permissions::PermissionPolicy,
    Agent, AgentEvent, SessionOptions,
};

#[tokio::main]
async fn main() -> a3s_code_core::Result<()> {
    let policy = PermissionPolicy::new()
        .allow("read(*)")
        .ask("write(*)")
        .deny("bash(rm:*)");

    let options = SessionOptions::new()
        .with_auto_compact(true)
        .with_permission_policy(policy);

    let agent = Agent::new("agent.acl").await?;
    let session = agent
        .session_builder(".")
        .options(options)
        .build()
        .await?;

    Ok(())
}`,
  },
  {
    id: 'tools',
    layer: 'L05 / WORKSPACE',
    filename: 'src/main.rs',
    title: {
      zh: '让 Workspace 决定可用工具',
      en: 'Let the workspace expose its tools',
    },
    body: {
      zh: 'Session 只注册当前 Workspace 真正支持、且权限允许的工具。发起 stream 后，模型调用 read、search、shell 或 Git 都会经过同一条受控路径。',
      en: 'A Session registers only tools the current workspace supports and policy permits. Every read, search, shell, or Git call follows the same governed path.',
    },
    note: {
      zh: '对象存储后端不支持本地命令时，Bash 与 Git 不会出现。',
      en: 'If a workspace cannot run local commands, Bash and Git stay hidden.',
    },
    tags: ['files', 'search', 'shell', 'git', 'MCP'],
    focus: [23, 26],
    code: `use a3s_code_core::{
    permissions::PermissionPolicy,
    Agent, AgentEvent, SessionOptions,
};

#[tokio::main]
async fn main() -> a3s_code_core::Result<()> {
    let policy = PermissionPolicy::new()
        .allow("read(*)")
        .ask("write(*)")
        .deny("bash(rm:*)");

    let options = SessionOptions::new()
        .with_auto_compact(true)
        .with_permission_policy(policy);

    let agent = Agent::new("agent.acl").await?;
    let session = agent
        .session_builder(".")
        .options(options)
        .build()
        .await?;

    let (mut events, lifecycle) = session
        .stream("Find the authentication entry points.", None)
        .await?;

    Ok(())
}`,
  },
  {
    id: 'evidence',
    layer: 'L06 / DURABILITY',
    filename: 'src/main.rs',
    title: {
      zh: '把过程交给界面，把现场留给下一次',
      en: 'Stream the run and keep the evidence',
    },
    body: {
      zh: '界面消费 AgentEvent；配置 SessionStore 后，save 会把会话、运行记录、Artifact、Trace 和验证结果作为同一代快照提交。',
      en: 'The UI consumes AgentEvent. With a SessionStore configured, save commits the session, runs, artifacts, traces, and verification data as one snapshot generation.',
    },
    note: {
      zh: '终端、Web 与 SDK 看到的是同一条事件协议。',
      en: 'Terminal, Web, and SDK clients share the same event protocol.',
    },
    tags: ['AgentEvent', 'Run', 'Artifact', 'Snapshot'],
    focus: [29, 39],
    code: `use a3s_code_core::{
    permissions::PermissionPolicy,
    Agent, AgentEvent, SessionOptions,
};

#[tokio::main]
async fn main() -> a3s_code_core::Result<()> {
    let policy = PermissionPolicy::new()
        .allow("read(*)")
        .ask("write(*)")
        .deny("bash(rm:*)");

    let options = SessionOptions::new()
        .with_auto_compact(true)
        .with_permission_policy(policy)
        .with_file_session_store(".a3s/sessions");

    let agent = Agent::new("agent.acl").await?;
    let session = agent
        .session_builder(".")
        .options(options)
        .build()
        .await?;

    let (mut events, lifecycle) = session
        .stream("Find the authentication entry points.", None)
        .await?;

    while let Some(event) = events.recv().await {
        match event {
            AgentEvent::TextDelta { text } => print!("{text}"),
            AgentEvent::End { .. } => break,
            _ => {}
        }
    }

    let _ = lifecycle.await;
    session.save().await?;
    Ok(())
}`,
  },
];

const result = [];
for (const step of steps) {
  const highlighted = await highlight(
    {
      value: step.code,
      lang: 'rust',
      meta: step.filename,
    },
    theme,
  );
  highlighted.annotations = [
    {
      name: 'focus',
      query: step.id,
      fromLineNumber: step.focus[0],
      toLineNumber: step.focus[1],
    },
  ];
  result.push({
    ...step,
    highlighted,
  });
}

await mkdir(path.dirname(outputPath), { recursive: true });
const output = await format(JSON.stringify(result), { parser: 'json' });
await writeFile(outputPath, output, 'utf8');
