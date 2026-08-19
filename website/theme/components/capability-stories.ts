export type Locale = 'zh' | 'en';

export type Localized = {
  zh: string;
  en: string;
};

export type CapabilityKey =
  'hitl' | 'progressive' | 'runtime' | 'intelligence' | 'ctx';

export type CapabilityStory = {
  key: CapabilityKey;
  index: string;
  eyebrow: string;
  title: Localized;
  body: Localized;
  prompt: Localized;
  availability: Localized;
  stages: Localized[];
};

export const sectionCopy = {
  zh: {
    eyebrow: 'A3S CODE / DISTINCTIVE CAPABILITIES',
    title: '五个关键瞬间，看懂一次任务如何安全地变聪明',
    body: '从执行前确认到代码语义，从按需发现平台能力到找回过去会话：选择一个场景，查看真实 A3S Code TUI 中的交互顺序与边界。',
    guide: '查看完整 TUI 指南',
    select: '选择能力演示',
    play: '播放',
    pause: '暂停',
    replay: '重播',
    context: 'ctx:12%',
    workspace: '~/workspace/a3s',
  },
  en: {
    eyebrow: 'A3S CODE / DISTINCTIVE CAPABILITIES',
    title: 'Five decisive moments that make a coding run safer and smarter',
    body: 'From approval before execution to code semantics, on-demand platform discovery, and past-session recall: select a scenario to see the real interaction order and boundaries in the A3S Code TUI.',
    guide: 'Read the complete TUI guide',
    select: 'Select a capability demo',
    play: 'Play',
    pause: 'Pause',
    replay: 'Replay',
    context: 'ctx:12%',
    workspace: '~/workspace/a3s',
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
    prompt: {
      zh: '测试通过后，将 main 分支推送到 origin',
      en: 'Push main to origin after the tests pass',
    },
    availability: {
      zh: 'Default 模式 · 风险感知',
      en: 'Default mode · risk-aware',
    },
    stages: [
      { zh: '准备精确调用', en: 'Prepare the exact call' },
      { zh: '执行前暂停', en: 'Pause before execution' },
      { zh: '选择授权范围', en: 'Choose the grant scope' },
      { zh: '继续执行或拒绝', en: 'Resume or deny' },
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
    prompt: {
      zh: '部署发布知识包，并打开运行视图',
      en: 'Deploy the release knowledge package and open its run view',
    },
    availability: {
      zh: '登录后可用 · 权限过滤',
      en: 'Available after login · permission-filtered',
    },
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
    prompt: {
      zh: '并行检查 core、Node 和 Python 发布包',
      en: 'Check the core, Node, and Python releases in parallel',
    },
    availability: {
      zh: '登录后注册 · 批量执行',
      en: 'Registered after login · batch execution',
    },
    stages: [
      { zh: '确认远程调用', en: 'Authorize the remote call' },
      { zh: '解析 Worker 并提交', en: 'Resolve the worker and submit' },
      { zh: '流式追踪进度', en: 'Stream progress' },
      { zh: '聚合结果', en: 'Aggregate results' },
    ],
  },
  {
    key: 'intelligence',
    index: '04',
    eyebrow: 'CODE INTELLIGENCE',
    title: {
      zh: 'Agent 与 TUI 共享同一份代码语义',
      en: 'Agent and TUI share one semantic code runtime',
    },
    body: {
      zh: '基于已保存文件提供符号、定义、声明、引用、实现与诊断。Agent 工具与 /ide 使用同一运行时，脏缓冲区不会伪装成已发布语义。',
      en: 'Saved files provide symbols, definitions, declarations, references, implementations, and diagnostics. Agent tools and /ide share the runtime; dirty buffers never masquerade as published semantics.',
    },
    prompt: {
      zh: '/ide',
      en: '/ide',
    },
    availability: {
      zh: 'Rust · TypeScript / JavaScript',
      en: 'Rust · TypeScript / JavaScript',
    },
    stages: [
      { zh: '打开内置 IDE', en: 'Open the built-in IDE' },
      { zh: '输入语义命令', en: 'Enter a semantic command' },
      { zh: '查询已保存版本', en: 'Query the saved version' },
      { zh: '选择并跳转结果', en: 'Select and jump to a result' },
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
    prompt: {
      zh: '/ctx RemoteUI view link',
      en: '/ctx RemoteUI view link',
    },
    availability: {
      zh: '本地索引 · 跨会话 · 可追溯',
      en: 'Local index · cross-session · traceable',
    },
    stages: [
      { zh: '搜索历史', en: 'Search history' },
      { zh: '查看命中', en: 'Inspect the matches' },
      { zh: '一次性附加', en: 'Attach once' },
      { zh: '保存并保留来源', en: 'Save with provenance' },
    ],
  },
];

export function localized(value: Localized, locale: Locale) {
  return value[locale];
}
