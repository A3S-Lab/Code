import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { highlight } from 'codehike/code';
import { format } from 'prettier';

const here = path.dirname(fileURLToPath(import.meta.url));
const websiteRoot = path.join(here, '..');
const outputPath = path.join(
  websiteRoot,
  'theme',
  'generated',
  'runtime-tutorial.json',
);
const theme = JSON.parse(
  await readFile(path.join(websiteRoot, 'codehike-theme.json'), 'utf8'),
);

function focusRange(code, firstLine, lastLine = firstLine) {
  const lines = code.split('\n');
  const from = lines.findIndex((line) => line.includes(firstLine));
  const to = lines.findIndex((line, index) => {
    return index >= from && line.includes(lastLine);
  });

  if (from < 0 || to < 0) {
    throw new Error(`Could not find focus range: ${firstLine} → ${lastLine}`);
  }

  return [from + 1, to + 1];
}

const steps = [
  {
    id: 'surfaces',
    layer: 'L01 / SURFACES',
    filename: 'runtime.py',
    language: 'Python',
    title: {
      zh: '选用 Python SDK',
      en: 'Start with the Python SDK',
    },
    body: {
      zh: '下面只使用 `a3s_code` 已公开的同步 API。先取得当前项目路径，再从 ACL 配置创建 Agent。',
      en: 'This walkthrough uses the public synchronous `a3s_code` API. Start with the current project path and create an Agent from its ACL config.',
    },
    note: {
      zh: '`closing` 会在退出代码块时调用 `agent.close()`。',
      en: '`closing` calls `agent.close()` when the block exits.',
    },
    tags: ['Python', 'a3s_code', 'Agent'],
    focusText: ['from a3s_code import Agent', 'with closing'],
    code: `from contextlib import closing
from pathlib import Path

from a3s_code import Agent

workspace = str(Path.cwd())

with closing(Agent.create("agent.acl")) as agent:
    pass`,
  },
  {
    id: 'session',
    layer: 'L02 / AGENT API',
    filename: 'runtime.py',
    language: 'Python',
    title: {
      zh: '创建项目会话',
      en: 'Create a project session',
    },
    body: {
      zh: 'Session 把 Agent 绑定到一个项目目录。`LocalWorkspaceBackend` 明确告诉 Runtime，文件和搜索工具应该在哪个工作区运行。',
      en: 'A Session binds the Agent to one project. `LocalWorkspaceBackend` tells the runtime exactly where file and search tools should operate.',
    },
    note: {
      zh: 'Agent 可以复用；Session 对应一次项目会话。',
      en: 'Reuse the Agent; create a Session for each project conversation.',
    },
    tags: ['SessionOptions', 'LocalWorkspaceBackend'],
    focusText: ['options = SessionOptions()', 'with closing(agent.session'],
    code: `from contextlib import closing
from pathlib import Path

from a3s_code import Agent, LocalWorkspaceBackend, SessionOptions

workspace = str(Path.cwd())
options = SessionOptions()
options.planning_mode = "disabled"
options.workspace_backend = LocalWorkspaceBackend(workspace)

with closing(Agent.create("agent.acl")) as agent:
    with closing(agent.session(workspace, options)) as session:
        pass`,
  },
  {
    id: 'context',
    layer: 'L03 / INTELLIGENCE',
    filename: 'runtime.py',
    language: 'Python',
    title: {
      zh: '限制上下文大小',
      en: 'Bound the context window',
    },
    body: {
      zh: '自动压缩阈值和 token 上限写在 SessionOptions 中。达到阈值后，Runtime 会整理历史，而不是继续把所有内容塞进模型输入。',
      en: 'Compaction thresholds and token limits belong in SessionOptions. When the threshold is reached, the runtime compacts history instead of growing the prompt without a bound.',
    },
    note: {
      zh: '大工具输出会保存为 Artifact，模型只接收受控预览。',
      en: 'Large tool output is stored as an artifact; the model receives a bounded preview.',
    },
    tags: ['auto_compact', 'tokens', 'Artifact'],
    focusText: ['options.auto_compact = True', 'max_context_tokens'],
    code: `from contextlib import closing
from pathlib import Path

from a3s_code import Agent, LocalWorkspaceBackend, SessionOptions

workspace = str(Path.cwd())
options = SessionOptions()
options.planning_mode = "disabled"
options.auto_compact = True
options.auto_compact_threshold = 0.8
options.max_context_tokens = 128_000
options.workspace_backend = LocalWorkspaceBackend(workspace)

with closing(Agent.create("agent.acl")) as agent:
    with closing(agent.session(workspace, options)) as session:
        pass`,
  },
  {
    id: 'governance',
    layer: 'L04 / GOVERNANCE',
    filename: 'runtime.py',
    language: 'Python',
    title: {
      zh: '明确哪些工具能执行',
      en: 'Decide which tools may run',
    },
    body: {
      zh: '这个示例只允许读取、列目录、搜索和代码导航；写文件、Shell 与 Git 一律拒绝。未命中的工具也按 `deny` 处理。',
      en: 'This example allows reads, directory listing, search, and code navigation. File writes, shell, and Git are denied, and unmatched tools default to `deny`.',
    },
    note: {
      zh: '示例不会触发待确认状态，因此事件循环不会停在无人处理的确认请求上。',
      en: 'The policy does not create pending approvals, so the event loop cannot stall on an unhandled confirmation.',
    },
    tags: ['PermissionPolicy', 'allow', 'deny'],
    focusText: ['options.permission_policy', 'default_decision'],
    code: `from contextlib import closing
from pathlib import Path

from a3s_code import (
    Agent,
    LocalWorkspaceBackend,
    PermissionPolicy,
    SessionOptions,
)

workspace = str(Path.cwd())
options = SessionOptions()
options.planning_mode = "disabled"
options.auto_compact = True
options.auto_compact_threshold = 0.8
options.max_context_tokens = 128_000
options.permission_policy = PermissionPolicy(
    allow=["read*", "ls*", "glob*", "grep*", "code_*"],
    deny=["write*", "edit*", "patch*", "bash*", "git*"],
    default_decision="deny",
)
options.workspace_backend = LocalWorkspaceBackend(workspace)

with closing(Agent.create("agent.acl")) as agent:
    with closing(agent.session(workspace, options)) as session:
        pass`,
  },
  {
    id: 'tools',
    layer: 'L05 / WORKSPACE',
    filename: 'runtime.py',
    language: 'Python',
    title: {
      zh: '消费执行事件',
      en: 'Consume execution events',
    },
    body: {
      zh: '`session.stream()` 返回 `AgentEvent`。代码直接区分文本、工具开始和错误，不需要解析终端输出。',
      en: '`session.stream()` yields `AgentEvent` values. The code handles text, tool starts, and errors directly instead of parsing terminal output.',
    },
    note: {
      zh: '工具能否出现，由 Workspace 能力和 PermissionPolicy 共同决定。',
      en: 'Workspace capabilities and PermissionPolicy jointly decide which tools are visible.',
    },
    tags: ['EventType', 'stream', 'tools'],
    focusText: ['for event in session.stream', 'raise RuntimeError'],
    code: `from contextlib import closing
from pathlib import Path

from a3s_code import (
    Agent,
    EventType,
    LocalWorkspaceBackend,
    PermissionPolicy,
    SessionOptions,
)

workspace = str(Path.cwd())
options = SessionOptions()
options.planning_mode = "disabled"
options.auto_compact = True
options.auto_compact_threshold = 0.8
options.max_context_tokens = 128_000
options.permission_policy = PermissionPolicy(
    allow=["read*", "ls*", "glob*", "grep*", "code_*"],
    deny=["write*", "edit*", "patch*", "bash*", "git*"],
    default_decision="deny",
)
options.workspace_backend = LocalWorkspaceBackend(workspace)

with closing(Agent.create("agent.acl")) as agent:
    with closing(agent.session(workspace, options)) as session:
        for event in session.stream(
            "Find the authentication entry points. Do not change files."
        ):
            if event.type == EventType.TEXT_DELTA and event.text:
                print(event.text, end="", flush=True)
            elif event.type == EventType.TOOL_START:
                print(f"\\n→ {event.tool_name or 'tool'}")
            elif event.type == EventType.ERROR:
                raise RuntimeError(event.error or "A3S Code run failed")`,
  },
  {
    id: 'evidence',
    layer: 'L06 / DURABILITY',
    filename: 'runtime.py',
    language: 'Python',
    title: {
      zh: '保存运行记录',
      en: 'Save the run record',
    },
    body: {
      zh: '`FileSessionStore` 为 Session 提供持久化后端。事件流结束后读取最后一次 Run，并用 `save()` 提交当前快照。',
      en: '`FileSessionStore` provides the persistence backend. After the stream ends, inspect the latest run and commit the current snapshot with `save()`.',
    },
    note: {
      zh: '`SessionSnapshotV1` 会一起保存会话、Run、Artifact、Trace 和验证结果。',
      en: '`SessionSnapshotV1` keeps the session, runs, artifacts, traces, and verification results together.',
    },
    tags: ['FileSessionStore', 'Run', 'SessionSnapshotV1'],
    focusText: ['options.session_store', 'session.save()'],
    code: `from contextlib import closing
from pathlib import Path

from a3s_code import (
    Agent,
    EventType,
    FileSessionStore,
    LocalWorkspaceBackend,
    PermissionPolicy,
    SessionOptions,
)

workspace = str(Path.cwd())
options = SessionOptions()
options.planning_mode = "disabled"
options.auto_compact = True
options.auto_compact_threshold = 0.8
options.max_context_tokens = 128_000
options.permission_policy = PermissionPolicy(
    allow=["read*", "ls*", "glob*", "grep*", "code_*"],
    deny=["write*", "edit*", "patch*", "bash*", "git*"],
    default_decision="deny",
)
options.workspace_backend = LocalWorkspaceBackend(workspace)
options.session_store = FileSessionStore(".a3s/sessions")

with closing(Agent.create("agent.acl")) as agent:
    with closing(agent.session(workspace, options)) as session:
        for event in session.stream(
            "Find the authentication entry points. Do not change files."
        ):
            if event.type == EventType.TEXT_DELTA and event.text:
                print(event.text, end="", flush=True)
            elif event.type == EventType.TOOL_START:
                print(f"\\n→ {event.tool_name or 'tool'}")
            elif event.type == EventType.ERROR:
                raise RuntimeError(event.error or "A3S Code run failed")

        runs = session.runs()
        if runs:
            current = runs[-1]
            print(f"\\nrun={current['id']} status={current['status']}")
        session.save()`,
  },
];

const result = [];
for (const step of steps) {
  const focus = focusRange(step.code, ...step.focusText);
  const highlighted = await highlight(
    {
      value: step.code,
      lang: 'python',
      meta: step.filename,
    },
    theme,
  );
  highlighted.annotations = [
    {
      name: 'focus',
      query: step.id,
      fromLineNumber: focus[0],
      toLineNumber: focus[1],
    },
  ];
  const { focusText: _focusText, ...publicStep } = step;
  result.push({
    ...publicStep,
    focus,
    highlighted,
  });
}

await mkdir(path.dirname(outputPath), { recursive: true });
const output = await format(JSON.stringify(result), { parser: 'json' });
await writeFile(outputPath, output, 'utf8');
