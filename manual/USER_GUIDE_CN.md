# A3S Code 用户与开发者指南

本文描述 A3S Code 7.x 的当前合同。这里集中说明稳定入口与关键边界，完整选项和 Wire
Shape 请查阅带版本的网站文档。

## 1. 选择宿主语言

| 宿主    | 安装方式                                   | 运行时边界                                      |
| ------- | ------------------------------------------ | ----------------------------------------------- |
| Rust    | `cargo add a3s-code-core`                  | 原生异步 Core API                               |
| Node.js | `npm install @a3s-lab/code`                | N-API Native Module                             |
| Python  | `pip install a3s-code`                     | 从对应 GitHub Release 获取的 PyO3 Native Module |
| Go      | `go get github.com/A3S-Lab/Code/sdk/go/v7` | Pure-Go Client 与版本完全一致的 Bridge Process  |

Node.js 和 Python 应优先使用异步生命周期 API。Go Module 与 Bridge Asset 必须来自同一
个 Release。Rust 的 Session 构建以异步为先，因为 Store、MCP Discovery、Workspace
Service 和 Retrieval Provider 都可能需要 I/O。

## 2. 使用 ACL 配置模型

ACL 是受支持的产品配置格式。`Agent.create` 可以接收 `.acl` 文件路径或 Inline ACL。

```acl
default_model = "openai/my-model"

providers "openai" {
  api_key = env("OPENAI_API_KEY")
  base_url = "https://api.openai.com/v1"

  models "my-model" {
    name = "My Model"
    tool_call = true
    temperature = true
    limit = { context = 200000, output = 8192 }
  }
}

task_scheduler {
  max_active = 4
  aging_interval_ms = 30000
}
```

凭据应通过环境变量进入 ACL。模型标识使用 `provider/model`。Chat Model 与 Embedding
Model 是两条独立 Route，配置前者不会隐式配置后者。

## 3. 创建、使用并关闭 Session

### Node.js

```js
import { Agent } from "@a3s-lab/code";

const agent = await Agent.create("agent.acl");
const session = await agent.sessionAsync("/repo", {
  planningMode: "auto",
  goalTracking: true,
});

try {
  const result = await session.send("Find the authentication entry points.");
  console.log(result.text);
} finally {
  await session.closeAsync();
  await agent.closeAsync();
}
```

### Python

```python
from a3s_code import Agent, SessionOptions

agent = await Agent.create_async("agent.acl")
options = SessionOptions()
options.planning_mode = "auto"
options.goal_tracking = True
session = await agent.session_async("/repo", options)

try:
    result = await session.send_async("Find the authentication entry points.")
    print(result.text)
finally:
    await session.close_async()
    await agent.close_async()
```

### Rust

```rust
use a3s_code_core::{Agent, SessionOptions};

let agent = Agent::new("agent.acl").await?;
let session = agent
    .session_builder("/repo")
    .options(SessionOptions::new())
    .build()
    .await?;

let result = session.send("Find the authentication entry points.", None).await?;
println!("{}", result.text);
session.close().await;
```

每个 Session 只拥有一个 Conversation Lifecycle。会改变 Transcript 的操作采用 Fail-fast
Single-flight。重叠的 `send`、`stream`、Attachment、Slash Command 或 Resume 会返回
Busy-session Error，不会进入不可见队列。发起下一轮之前应完整消费或取消当前 Stream。

## 4. 理解工具面

Workspace Backend 决定哪些工具能够注册。应检查 `toolNames()` 与 `toolDefinitions()`，
不要假定宿主一定是本地文件系统。

| 层级      | 模型可见工具                                                                | 说明                                                                            |
| --------- | --------------------------------------------------------------------------- | ------------------------------------------------------------------------------- |
| Workspace | `read`、`write`、`edit`、`patch`、`download`、`search`、`ls`、`bash`、`git` | 每个工具都由 Capability 控制；`download` 还要求可写的 Local Workspace。         |
| Runtime   | `web_fetch`、`web_search`、`batch`、`program`                               | Nested Call 会继承 Invocation Scope、Cancellation 与 Budget。                   |
| Session   | `task`、`generate_object`、`search_skills`、`Skill`                         | Delegation 可以关闭；兼容别名 `parallel_task` 可直接调用，但不进入模型 Schema。 |
| Dynamic   | MCP 与 Host-registered Tool                                                 | MCP 工具使用 `mcp__<server>__<tool>` Namespace。                                |

`search` 是唯一面向模型的仓库检索合同，每次调用都必须传入 Mode 和 Query。

```json
{
  "mode": "bm25",
  "query": "workspace permission policy",
  "path": "core/src",
  "include": "*.rs",
  "limit": 8
}
```

| Mode       | 用途                                                         | 是否需要 Embedding           |
| ---------- | ------------------------------------------------------------ | ---------------------------- |
| `grep`     | 正则内容检索                                                 | 否                           |
| `glob`     | 路径发现                                                     | 否                           |
| `bm25`     | Native Lexical Ranking                                       | 否                           |
| `semantic` | Session Vector 上的 Exact Cosine Ranking                     | 是                           |
| `hybrid`   | 对 Exact、Lexical、Symbol 与 Semantic Evidence 做 RRF Fusion | 可选，没有向量时保留其他通道 |

路径已知时可用 `read.files` 完成有界、有序的批量读取。机械编辑应先设置
`dry_run: true` 预览，再用相同参数和 `expected_replacements` 执行；需要独立上限时再加
`max_replacements`。

## 5. 启用异步内存工作区检索

Dense Retrieval 是需要宿主显式启用的 Capability。它不是持久化 Vector Database，也
不会因为选择了某个 Chat Model 而自动打开。宿主提供带类型的 Embedding Provider；
A3S Code 负责文本准入、切块、批处理、响应校验、Exact In-memory Vector Partition、
Hybrid Ranking、当前源校验、资源计量和清理。

```js
import {
  CallbackEmbeddingProvider,
  RecursiveWorkspaceChunkingStrategy,
  WorkspaceRetrievalOptions,
} from "@a3s-lab/code";

const provider = new CallbackEmbeddingProvider(
  {
    provider: "host-embeddings",
    model: "code-search-v1",
    dimension: 768,
    normalization: "unit",
  },
  async ({ inputs, signal }) => {
    const response = await embeddingClient.embed(
      inputs.map(({ text }) => text),
      { signal },
    );
    return {
      vectors: inputs.map((input, index) => ({
        id: input.id,
        values: response[index],
      })),
    };
  },
);

const retrieval = new WorkspaceRetrievalOptions(
  provider,
  null,
  new RecursiveWorkspaceChunkingStrategy(8 * 1024, 512, [
    "\n\n",
    "\n",
    ". ",
    " ",
  ]),
);
retrieval.maxRecords = 100_000;
retrieval.maxBytes = 128 * 1024 * 1024;

const session = await agent.sessionAsync("/repo", {
  workspaceRetrieval: retrieval,
});

console.log(session.workspaceRetrievalStatus());
console.log(
  await session.hybridSearch({
    query: "where session shutdown releases temporary indexes",
    limit: 8,
  }),
);
await session.closeAsync();
```

Session 构建会在 Corpus Embedding 完成前返回。状态依次为 `building`、`ready`、
`degraded` 与 `closed`。完成的 File Partition 会原子发布，因此构建期间的查询可以使用
Partial Coverage。关闭 Session 会取消 Provider Work，在 Deadline 内 Join Indexer，
并释放所有已计量的 Vector Record 与 Byte。

只有 Manifest 准入的 UTF-8 文本会进入切块和 Embedding。Generated、Oversized、
Credential-bearing、`.a3s` Control 与 Non-text File 会被排除。返回结果前，Code 会重新
读取源文件，验证 Full-file Digest 和精确 Chunk Range。Stale、Deleted、Unreadable 或
Superseded Chunk 不会暴露给模型。

内置索引是 Exact、Bounded 的 `InMemoryVectorIndex`。重新创建 Session 会重建 Projection，
不同 Session 不共享索引。如果产品需要持久化或共享 Vector Service，应由 Embedding Host
独立提供，同时保留 Code 的当前源校验边界。

`a3s` CLI 只能从受信任用户 ACL，或通过 `--config` 显式选择的文件启用检索。自动发现的
工作区 `.a3s/config.acl` 可以关闭继承的检索能力，但不能授权源代码出站，也不能选择
Backend。应在远程 Embedding 路由与互斥的 `local_cpu` Artifact Manifest 之间选择；前者
必须显式设置 `allow_source_egress = true`。Chat `default_model` 与 Embedding 路由相互独立，
因此 DeepSeek Chat 路由不会自动变成 Embedding Endpoint。两种 ACL 和验证命令见
[工作区检索运维手册](WORKSPACE_RETRIEVAL_OPERATIONS.md)。

## 6. 在正确边界应用治理

模型选择的 Tool Call 会经过 Active-skill Restriction、Permission Policy、Confirmation、
Hook、Budget Check、Lane Admission、Timeout、Cancellation、Recursive-call Protection、
Output Sanitization、Artifact Limit 与 Workspace Path Check。

`session.tool(...)` 等 Direct SDK Call 属于 Trusted Host Control Plane。因为调用由宿主
选择，它们会跳过面向模型的 Permission 与 Confirmation。宿主协调的调用如果仍需经过
Session Permission 和 Confirmation，应使用 Governed Direct API。

```python
from a3s_code import PermissionPolicy, SessionOptions

options = SessionOptions()
options.permission_policy = PermissionPolicy(
    allow=["read(*)", "search(*)"],
    deny=["bash(*)"],
    default_decision="deny",
)
```

宿主必须先完成自身用户的 Authentication 与 Authorization，再把请求转换为 Trusted
Direct Call。工具可见不等于获得授权。

## 7. 使用带类型的 Store 与有界上下文

Backend 选择使用带类型的对象，不使用原始 Backend Name。

```python
from a3s_code import FileMemoryStore, FileSessionStore, SessionOptions

options = SessionOptions()
options.memory_store = FileMemoryStore("./.a3s/memory")
options.session_store = FileSessionStore("./.a3s/sessions")
options.session_id = "review-session"
options.auto_save = True
options.auto_compact = True
options.auto_compact_threshold = 0.75
```

Session Snapshot 会持久化 Conversation 与受支持的 Runtime Contract，但不会序列化 Live
Provider Callback、Credential、MCP Process 或临时 Workspace Vector Index。Resume 必须
重新注入必要的 Live Resource，并会拒绝与 Persisted Policy 不兼容的配置。

Memory Store 保存学习到的条目，Workspace Retrieval 索引当前源代码。它们是两个独立
系统，不应被描述成同一种存储。

## 8. 组合 Skills、MCP、Delegation 与 Workflow

- Skill 是从显式目录或 Registry 加载的 Markdown Package。A3S Code 不提供隐藏的默认
  Skill Catalog。
- MCP Manager 通过配置的 Transport 发现外部工具。Child Session 只继承宿主显式提供
  的 Manager 和 Policy。
- `task` 启动一个有界 Specialist Run。独立 Fan-out 使用带类型的 Host Helper 或隐藏的
  兼容别名，不增加 Model-visible Schema。
- Agent-wide Priority Scheduler 对 Session、Direct Tool、Detached Child 与 Host Workflow
  施加同一个 Capacity Boundary。
- `program` 执行有界 QuickJS Orchestration。Dynamic Workflow 使用 A3S Flow 提供 Replay
  与 Shared Budget。

无人值守的工作负载应显式设置 Worker Count、Depth、Step、Token、Timeout 和 Output
Limit。Parent Session 关闭前必须取消或 Join 它拥有的 Child。

## 9. 验证结果并观察运行

Verification Command 会把“已完成”的声明转换为记录下来的证据。

```js
const report = await session.verifyCommands("release-readiness", [
  {
    id: "tests",
    kind: "test",
    description: "Focused tests pass",
    command: "npm test",
    required: true,
    timeoutMs: 120000,
  },
]);

console.log(report);
console.log(session.verificationSummaryText());
```

Run 与 Event API 为 Headless Host 提供稳定、带版本的 Record。Trace Event、Verification
Report、Tool-result Evidence、Artifact 与 Scheduler Snapshot 都是 Observation，不是权限
或 Capacity Reservation。

## 10. 测试和资格认证

应从受影响的 Crate 或 SDK 运行聚焦测试。仓库的远程门禁包括 Formatting、Strict
Clippy、Default 与 All-feature Rust Test、Go Race Test、Native Node.js/Python Runtime
Suite、Workspace Retrieval Churn、文档检查和 Capability-ledger Validation。

性能证据分成两种门禁：

1. 确定性的工作量与资源上限，例如 Provider Request、Retry、Record、Byte、Candidate、
   Queue Depth、Tool Round 与完整的关闭后释放；
2. Release Build 的延迟资格认证，使用固定 Corpus、Warmup、重复 Sample、p50/p95/Max、
   Machine Metadata 和保留的 JSON Artifact。

Remote Model、Public Search Engine 与第三方服务延迟必须和 Local Core Latency 分开报告。
Job Timeout 只能防止任务挂死，不能当作产品性能数据。

Release 工作流会验证收敛、25,000 Record 检索、Flow/State Graph、5,000 File Code
Intelligence Workspace、25,000 Item Context Assembly 与 2,500 Item Memory Recall，以及
1–2 MiB Session Persistence。[性能资格认证](PERFORMANCE_QUALIFICATION.md)记录成功运行、
精确包含范围、p50/p95/Max、资源测量与 Artifact Digest。

20 个领域的证据台账、已关闭的 Code-owned Gap 与仍需部署环境验证的外部边界见
[能力验证](CAPABILITY_VERIFICATION.md)。向量质量、资源、生命周期与 DeepSeek 资格认证见
[工作区检索 QA](WORKSPACE_RETRIEVAL_QA.md)。

## 11. 权威参考

- [带版本的网站文档](https://a3s-lab.github.io/Code/)
- [Node.js SDK](../sdk/node/README.md)
- [Python SDK](../sdk/python/README.md)
- [Go SDK](../sdk/go/README.md)
- [高级开发者手册](ADVANCED_DEVELOPER_MANUAL_CN.md)
- [SDK API 设计](SDK_API_DESIGN.md)
- [工作区检索运维手册](WORKSPACE_RETRIEVAL_OPERATIONS.md)

如果本概览与某个版本的 SDK Declaration 不一致，应以该 Release 的 SDK Declaration 和
可执行 Contract Test 为准。
