<p align="center">
  <img src="./assets/readme/hero.svg" width="100%" alt="A3S Code：带有显式模型、策略、工具、事件与快照流的受治理 Agent 运行时">
</p>

<p align="center">
  <strong>Language / 语言:</strong>
  <a href="README.md">English</a> ·
  <a href="README.zh-CN.md">中文</a>
</p>

<p align="center">
  <a href="https://github.com/A3S-Lab/Code/releases"><img alt="GitHub release" src="https://img.shields.io/github/v/release/A3S-Lab/Code?style=flat-square&color=6ca3ff"></a>
  <a href="https://github.com/A3S-Lab/Code/actions/workflows/ci.yml"><img alt="CI status" src="https://img.shields.io/github/actions/workflow/status/A3S-Lab/Code/ci.yml?branch=main&style=flat-square&label=CI"></a>
  <a href="https://crates.io/crates/a3s-code-core"><img alt="Crates.io" src="https://img.shields.io/crates/v/a3s-code-core?style=flat-square&color=f0b44d"></a>
  <a href="https://www.npmjs.com/package/@a3s-lab/code"><img alt="npm" src="https://img.shields.io/npm/v/%40a3s-lab%2Fcode?style=flat-square&color=cb3837"></a>
  <a href="https://pypi.org/project/a3s-code/"><img alt="PyPI" src="https://img.shields.io/pypi/v/a3s-code?style=flat-square&color=3775a9"></a>
  <a href="./LICENSE"><img alt="MIT license" src="https://img.shields.io/badge/license-MIT-3ccf91?style=flat-square"></a>
</p>

**A3S Code** 是用于构建受治理编码 Agent 的异步 Rust 运行时。它将
Agent 循环、工作区工具、模型适配器、策略决策、版本化事件、
工作区检索与持久证据都放在显式契约之后。可通过 Rust、
Node.js、Python、Go，或通过 `a3s code` 终端应用使用。


<p align="center">
  <a href="#60-秒内起步">起步</a> ·
  <a href="#83-有什么新内容">v8.3</a> ·
  <a href="#80-有什么新内容">v8.0</a> ·
  <a href="#为何选择-a3s-code">为何选择 Code</a> ·
  <a href="#能力地图">能力</a> ·
  <a href="#配置运行时">配置</a> ·
  <a href="#架构">架构</a> ·
  <a href="#文档">文档</a>
</p>

## 8.3 有什么新内容

- **可协商的会话存储持久性（KRN-6）。** 聚合 CAS、仅追加 WAL、writer lease
  fencing、可选 AES-256-GCM 静态加密（at rest）、commit watch，以及引用感知的
  artifact GC，仅在已证明可用时才对外声明。
- **类型化工具结果信任（KRN-5）。** Trusted / workspace / external 标签穿越
  tool→model 边界；run-bound 调用在后续中间件之前接纳 prompt 信任；
  各 SDK 上的 `model_middleware_health` 均不含密钥。
- **工作区源快照（KRN-4）** 将检索结果绑定到可在派生索引重建后仍保留的
  防篡改身份。
- **可失败的 FFI 运行时 init（KRN-9）** 面向 Node.js 与 Python；以及宿主拥有的
  不可变内容适配器、检查点导出 sink，以及 Node.js / Python / Go 上的
  Skill 能力批处理。
- **Linux arm64 Python wheel** 以 `manylinux_2_39_aarch64`（glibc 2.39+）发布，
  以匹配捆绑的 zvec 运行时。

文档：[a3s-lab.github.io/Code](https://a3s-lab.github.io/Code/)（`v8.3.0`）。

## 8.0 有什么新内容

- **Run 拥有的时空组合。** Session、Run、Turn 与 Subtask 作用域现在构成
  一棵仅向下的权威与取消树，并带有有界的、逆序的效果结算。
- **Generation-exact 的能力投影。** Tool、Skill、Agent、Command、Hook、MCP、
  Context、Flow、Knowledge 与 UI 值以原子方式发布，并在每个已接纳 Run
  的生命周期内保持钉住。
- **精确的时间恢复。** Run 与逻辑检查点证据绑定 Code catalog、完整
  authority ceiling，以及可选的 A3S Use cursor。一个 N 检查点不能静默地
  通过 N+1 恢复。
- **可移植检查点 artifact。** 规范语义与逻辑状态作为单一可宿主存储的
  payload 做内容寻址，带有失败即关闭的漂移检查，以及在全新 Session 上的
  精确历史引导路径。
- **有界模型证据。** 工具请求、确定性结果变换、不可变原始内容引用、
  模型输入与能力面都以 digest 绑定，且不保留凭据或 prompt 明文。
- **收敛工作流结果。** 可恢复的工作流检查点与 Flow 决策声明携带有界、
  仅 digest 的结果收据，并绑定到规范执行身份；陈旧或不可读状态失败即关闭，
  而遗留记录仍可加载。
- **收敛工作流准入。** 动态 Flow 步骤投影到 Code 规划所用的同一
  `ExecutionPlan`，在恢复时从完整历史重建该计划，并使用可取消的
  每工作流并发门。独立 Flow 适配器还可使用 Agent 范围的优先级调度器，
  配合仅 digest 的步骤身份与所有者配额；会话绑定调用保留一个外层
  调度器租约，以避免嵌套单槽死锁。分离的子级继承 run/session 准入范围。
- **Provider-aware 的 generation 准入。** 常规、流式与结构化模型调用在
  会话、委派子项、直接工具与动态工作流之间共享同一类型化
  provider/model 容量身份。叶子 generation 通过既有调度器 actor 预留该容量，
  而不创建第二个队列；取消与丢弃的流会自动释放本地与共享预留。
  Rust 宿主可检查不含密钥的 `ModelGenerationPoolHealthSnapshot`；调度器
  仅为已完成的池周期保留有界的近期健康窗口。
- **Generation-fenced 的工作流重放。** 新的动态工作流运行钉住 Code 运行时
  构建，并暴露由持久不可变事实派生的仅 digest 延续身份。变更的源/输入、
  冲突的步骤定义，以及不受支持的运行时 generation 会在步骤执行前被拒绝；
  遗留的未钉住历史在迁移期间仍可读。稳定的 claim 身份现在以本地文件支持
  （或宿主注入）的租约门控每个 worker；心跳保持存活所有者被围栏；
  过期 worker 在工作流/步骤准入前被拒绝；父级取消会结算或保持租约围栏，
  而不是释放飞行中的 worker。宿主可绑定一个 `DynamicWorkflowControl` 句柄，
  以检查有界仅 digest 快照、读取可信历史、驱动运行，或请求/强制持久取消；
  本地 journal 使用跨进程锁，而 Flow 仍是唯一的事件权威。

Go 消费者必须将模块路径更新为
`github.com/A3S-Lab/Code/sdk/go/v8`。完整兼容性与发布记录见
[CHANGELOG.md](CHANGELOG.md)。

## 60 秒内起步

### 运行终端产品

```bash
brew install A3S-Lab/tap/a3s

# Or install from crates.io
cargo install a3s

cd /path/to/your/project
a3s code
```

终端产品流推理、工具活动、审批、任务
进展和差异。使用 `a3s code resume` 恢复持续工作或
`a3s code resume <session-id>`。

### 嵌入运行时

```bash
cargo add a3s-code-core
cargo add tokio --features macros,rt-multi-thread
```

```rust,no_run
use a3s_code_core::{Agent, AgentEvent};

#[tokio::main]
async fn main() -> a3s_code_core::Result<()> {
    let agent = Agent::new("agent.acl").await?;
    let session = agent.session_builder(".").build().await?;

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
    Ok(())
}
```

`Agent` 拥有解析配置和共享功能。 `AgentSession`
将他们绑定到一个工作区和对话。事件流是产品
边界：宿主可以呈现与运行时持续相同的生命周期，并且
重放。

代理执行也有一个显式的生命周期树。主机调用承认
`Session -> Run`；模型编排和每个提供者/工具迭代都拥有一个
转动，而技能和任务子级则递归为 `Turn -> Subtask -> Turn`。
工具效果和流桥随其回合而定。明确的背景
任务和回合后记忆提取只有在调用后才会被提升
转弯被验证，然后继续受到运行的监督，直到有界关闭。

## 为何选择 A3S Code

|要求 |运行时机制 |
| --------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **控制所有副作用** | JSON 参数验证、类型化工具功能、权限策略、人工确认、挂钩、预算、安全提供程序和取消共享一个调用路径。         |
| **保持上下文有界** |读取、搜索、命令输出、Git 结果和获取的页面公开范围或游标。大量证据转移到具有预览、大小和哈希值的有限工件中。           |
| **拥有 UI，无需分叉循环** |核心发出`AgentEvent`； SDK 流和持久运行使用无损 `EventEnvelopeV1` 协议。主机选择表示、身份、凭证和部署策略。 |
| **无需权限漂移即可更改模型形状** |封闭的工具呈现配置文件在权限可见性之后和模型请求之前运行；执行保持相同的固定工具值和治理。 |
| **从证据中恢复，而不是猜测** | `SessionSnapshotV1` 可以以原子方式提交会话状态、运行、工件、跟踪、验证报告和子任务记录作为一代。                                 |

一轮轮遵循可见的责任链：

```text
user request
    │
    ▼
workspace-bound AgentSession
    │ context + memory
    ▼
model adapter
    │ proposed tool call
    ▼
validation → permission → confirmation → budget → sandbox
    │ governed result
    ▼
AgentEvent / EventEnvelopeV1
    │
    └── runs + traces + artifacts + SessionSnapshotV1
```

这种分离使得交互式终端、SDK 应用程序和
后台服务共享相同的执行语义，但不共享 UI。

## 能力地图

Core crate 默认启用惰性 Moli 支持的搜索
`a3s-search` v3.1.0。 Moli是从打包的sidecar中解析出来的，经过验证
每用户缓存，或固定的 HTTPS 下载，并由所有本地代码共享
流程。最小嵌入可以使用`default-features = false`；铬合金和
Lightpanda 仍然是显式后端，而云后端、服务和
遥测仍然处于选择加入状态。

|面积 |有什么可用 |激活|
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
|代理运行时 |异步`Agent`、工作区限制`AgentSession`、发送、流式传输、恢复、替换、取消、关闭、重放和安全点`steer`/`interrupt`运行控制 |基线|
|受控工具 |文件、搜索、shell、Git、Web、结构化生成、批处理、程序、技能、MCP、委托、确定性结果投影和证据 |仅在工作空间和政策允许时才公开 |
|评估基材|提供者中立的执行目标/框架、仅摘要的事实日志、原子有界证据快照、独立的辅助运行、主机边界监督、重新启动安全调度租约、持久结果 CAS 以及严格版本化的 Rust/Node/Python/Go 线投影 |注入 `EvaluationPolicy`/`AuxiliaryExecutor` 以及可选的调度/结果存储；核心供应机制和生成的传输模式，而审阅者规则、调查结果、授权和云审计仍然由宿主拥有 |
|本土研究契约|版本化的摘要绑定研究运行、证据事实、声明、引文、具有出版完整性的证据图、具有结果收据绑定的工作流程计划、发现触发的重新运行谱系、再现性清单、出处收据、审查结果、项目事件以及具有生成的 Node/Python/Go 投影、有界字段和失败即关闭生命周期转换的版本化研究线信封 |主机绑定精确的源/证据快照和`RunCapabilityBindingV1`； A3S Use提供包/环境身份和桌面/云自己的科学政策、审查决策、保留和发布 |
|代码情报 |已保存文件符号、定义、声明、引用、实现、诊断、修订和过时状态元数据 |主机选择的本地工作区 |
|工作空间检索 |异步会话拥有的块目录、默认情况下的官方 zvec-rust FTS/BM25、内存支持的精确向量、混合 RRF、可选的确定性 CPU 重新排序、就绪/覆盖指标以及经过摘要验证的当前源结果 |每个会话明确选择语义/向量工作；基线词汇和符号搜索不需要嵌入模型或向量数据库；原生 zvec 构建需要经过验证的平台库 |
|背景和记忆|排名上下文、重复压缩、三层 V1 内存、类型化存储、召回、提取、非破坏性取代、V2 候选阴影、审核的仅活动词法/语义/一跳关系召回、确定性 RRF、验证修订版 CAS 快照刷新收据、精确命名空间令牌加速、主机持久安全刷新检查点、选择加入会话拥有的刷新计划、精确重启绑定和拥有的维护运行状况 |主持人选择； V2 需要精确的存储库/命名空间绑定和证据支持的激活；语义召回还需要类型化嵌入提供程序、调用者拥有的向量索引、显式刷新定时和精确的 schema-5 生成身份 |
|认知包 | Exact A3S Use生成绑定、宿主注入引用 Markdown 提供程序、有界源验证、重新启动检查和失败关闭检索 | Rust 宿主注入`CognitiveContextSession`；代码从不安装或解析包 |
| A3S Use运行时任务 |通过宿主拥有的调度程序进行精确的功能快照 v2 运行时工具投影和模型可见的受控调用 |原子使用支持的`SessionCapabilityBatch`中的阶段`UseRuntimeTaskProjectionAdapter`；代码从不启动预计的命令或直接获取包状态 |
|型号适配器| Anthropic、Zhipu、OpenAI 兼容 API 和自定义 `LlmClient` 实现；每个运行绑定调用都会通过一个显式中间件管道（信任→预算→证据→生成→提供者→使用）|配置或宿主注入；外部工具结果需要在及时使用之前进行编辑审查 |
|结构化输出|本机提供程序格式或模式验证提示、部分解析和修复回退 |基线|
| MCP 和技能 |隔离 MCP 传输以及文件系统、注册表、内联和实时会话技能 |配置或实时注册 |
|规划与授权|可选计划和目标、前台/后台工作人员、有界并行任务、进度和有针对性的取消 |手动工具可独立配置；自动化选择加入 |
|优先调度|跨会话、直接工具、分离的后台子进程和主机工作流程的代理范围`a3s-lane`优先级/先进先出准入，具有取消、饥饿安全老化、仅摘要所有者/提供者配额、仅配额叶预留、占用快照和有界累积运行状况计数器 |基线；调整`task_scheduler`，选择每个会话`TaskPriority`，检查`task_scheduler_stats()`或`task_scheduler_health()`；宿主可以使用 `TaskSchedulerQuota` 进行范围限制，使用 `model_generation_pool_health()` 进行会话的提供程序池，以及 `model_middleware_health()` / SDK 等效项进行无秘密中间件阶段计数器 |
|安全点运行控制|类型化、幂等 `steer` 和协作式 `interrupt` 请求，具有不可变的运行身份、乐观转向防护、有界收据、生命周期 Hook 和持久事件证据 | Host调用Session控制面；请求永远不会创建并发转录操作，也永远不会更改模型、权限、沙箱或预算 |
|可编程工作流程|有界 QuickJS `program` 调用、可重放的 A3S Flow 支持的动态工作流程、可恢复的步骤检查点和摘要绑定结果收据 | `program`基线；动态运行时显式注册|
|坚持|原子快照，具有文件存储意图/提交的 WAL 恢复、聚合 CAS (`save_snapshot_cas`)、编写者租赁防护 (`acquire_writer_lease`)、提交监视 (`watch_commits`)、可选 AES-GCM 静态加密（at rest） (`with_encryption_key`)、通过 SessionStore 往返的引用感知工件保留 (`reference_aware_artifact_gc`)、运行事件、跟踪、工件、验证、身份绑定工作流程/流程收据、检查点和可选的 RL 轨迹 |配置存储和主机策略；在依赖仅追加 WAL、聚合 CAS、租赁防护、监视、静态加密（at rest）、引用感知 GC 或其他 KRN-6 保证之前协商`SessionStoreCapabilities`； GC 之前固定工件 URI |
|状态图|哈希链接事件、类型化对象和关系、乐观补丁、严格重放、分叉、差异和 Flow 0.11 生命周期预测，包括取消、最终结果、进度和子操作 |显式应用程序使用 |
|代理解除契约|有界 `.a3s/asset.acl` 准入、规范身份、出处绑定和兼容性检查 |基线准入API |
|无头代理协议 |精确的发布/会话/运行开始、取消、检查点恢复、收据、原子观察的有界 `EventEnvelopeV1` 页面、每个会话分离的 Git 工作树和不可变的 `/v1/agent/changes` 补丁 | `AgentProtocolHarness` 复用普通代码会话，`AgentProtocolHost` 通过每个`AgentSession` 执行； `a3s code` 流程用品服务运输|
|无头网络搜索 | `a3s-search` v3.1.0，具有惰性 Moli 支持的 Google/Baidu/Bing/Brave 引擎、共享缓存生命周期和类型诊断； Chrome/Chromium 和 Lightpanda 仍可配置 |默认Cargo功能`headless-search`；使用 `default-features = false` 禁用 |
| SDK能力合约| Rust、Node.js、Python 和 Go 公开了有序产品功能清单、模式发现、Moli 诊断/配置和状态图 API |在可选集成之前调用每个 SDK 的能力发现功能 |
| S3 工作区 | S3 兼容对象后端 |Cargo功能`s3` |
|文件系统代理服务器|代理目录 cron 提供准备后准备、类型化故障状态和有界连接关闭 |Cargo功能`serve` |
|开放遥测|除基线 `tracing` 外，还导出 OTLP |Cargo功能`telemetry` |

可用性永远不会绕过政策。自动保存、自动压缩、目标、
自动授权、沙盒、人工审批、轨迹记录等
图形集成仅在主机配置它们时运行。内存提取是
可配置并可以禁用。

共同的评估基底遵循相同的边界：代码记录
仅摘要执行事实，读取有限证据，监督隔离
辅助运行，公开不可变的结果合约，并预测这些
通过为 Rust 生成的严格的 `EvaluationWireEnvelopeV1` 值，
Node.js、Python 和 Go。可选的文件支持结果和调度适配器添加
有界原子持久性和重新启动安全防护，无需获取所有权
主机授权或业务保留。主持人可以建立一个审稿人或
通过注入自己的策略和结构化执行器来验证者；核心没有
定义评分标准、查找词汇表、决策阈值、UI 或云审核
工作流程。参见
[Evaluation Substrate](manual/EVALUATION_SUBSTRATE.md)。

默认的系统提示符是分层组装的：紧凑的Agent 循环，
运行时权限/运行控制契约，规范的存储库工具模式，
并共享安全边界。主机运行时对每个人都保持权威
许可、批准、预算、取消和沙箱决策；提示文字
不授予当前会话尚未公开的能力。

科学工作流程使用相同的边界。 `a3s-code-core::research` 绑定
运行精确的项目、来源、证据和代码/使用能力标识；
记录仅摘要的观察结果；并发布出处并审查形状
可以由主机渲染。它故意留下包解析，
审稿人的标准、接受阈值、人工批准、保留和
发布到 A3S Use 和主机应用程序。参见
[Native Research Contracts](manual/RESEARCH_CONTRACTS.md)。

## 配置运行时

A3S Code对产品使用[A3S ACL](https://github.com/A3S-Lab/ACL)
配置。将凭据保存在环境变量中而不是源中。

```acl
default_model = "anthropic/claude-sonnet-4-20250514"

providers "anthropic" {
  api_key = env("ANTHROPIC_API_KEY")

  models "claude-sonnet-4-20250514" {
    name = "Claude Sonnet"
    tool_call = true
    limit = {
      context = 200000
      output = 8192
    }
  }
}

storage_backend = "file"
sessions_dir = ".a3s/sessions"
memory_dir = ".a3s/memory"
skill_dirs = [".a3s/skills"]
agent_dirs = [".a3s/agents"]

task_scheduler {
  max_active = 4
  aging_interval_ms = 30000
}
```

`Agent` 创建的每个会话都共享此调度程序。优先事项是
`urgent`、`interactive`（默认）、`foreground`、`background` 和
`maintenance`；相同的优先级仍然是 FIFO。提倡较旧的非紧急工作
每个`aging_interval_ms`一个级别，直至交互优先级，如此持续
交互式流量不能永远缺乏后台工作。

准入在有界队列（4,096 个条目）处进行反压；释放和
关闭通知使用单独的控制路径，因此取消、丢弃
即使准入队列已满，调用者和关闭也始终会取得进展
满。

`Agent::new` 接受 ACL 路径或内联 ACL。异步构建会话
因此配置、存储、队列、MCP 源和工作区服务是
在第一回合之前解决。

```rust,no_run
use a3s_code_core::{Agent, PlanningMode, SessionOptions, TaskPriority};

#[tokio::main]
async fn main() -> a3s_code_core::Result<()> {
    let options = SessionOptions::new()
        .with_planning_mode(PlanningMode::Auto)
        .with_tool_timeout(120_000)
        .with_auto_compact(true)
        .with_max_context_tokens(200_000)
        .with_auto_compact_threshold(0.8);

    let options = options.with_task_priority(TaskPriority::Interactive);

    let agent = Agent::new("agent.acl").await?;
    let session = agent
        .session_builder("/path/to/workspace")
        .options(options)
        .build()
        .await?;

    let stats = agent.task_scheduler_stats().await?;
    let same_scheduler = session.task_scheduler_stats().await?;
    println!("active={} pending={}", stats.active, stats.pending);
    assert_eq!(stats.max_active, same_scheduler.max_active);

    Ok(())
}
```

类型化会话选项接受自定义模型客户端、上下文提供者、内存
存储、会话存储、工作区后端、安全提供程序、确认
提供者、权限检查器和其他宿主拥有的扩展。

### 推荐的治理配置

在会话边界保持明确的治理。对于互动主持人来说，
默认询问，启用真正的确认通道，超时拒绝，以及
启用输出清理：

```rust,no_run
use a3s_code_core::{
    hitl::{ConfirmationPolicy, TimeoutAction},
    permissions::PermissionPolicy,
    SessionOptions,
};

let interactive = SessionOptions::new()
    .with_permission_policy(PermissionPolicy::strict())
    .with_confirmation_policy(
        ConfirmationPolicy::enabled().with_timeout(30_000, TimeoutAction::Reject),
    )
    .with_default_security();
```

对于无人值守的主机，请使用显式允许列表并拒绝其他所有内容。
不要故意安装禁用的确认策略：`enabled = false`
自动批准 `Ask` 兼容性决策。省略确认
提供商使任何意外的 `Ask` 或工具级升级失败关闭。

```rust,no_run
use a3s_code_core::{
    permissions::{PermissionDecision, PermissionPolicy},
    SessionOptions,
};

let read_only = PermissionPolicy {
    default_decision: PermissionDecision::Deny,
    ..PermissionPolicy::default()
}
.allow("read(*)")
.allow("search(*)")
.allow("ls(*)");

let unattended = SessionOptions::new()
    .with_permission_policy(read_only)
    .with_default_security();
```

`DefaultSecurityProvider` 执行污点跟踪和输出清理；它
不是进程隔离。附加一个`BashSandbox`用于外壳隔离并选择
进程内文件工具的适当工作区访问策略。直接
`tool()` 助手是可信的控制平面调用；使用`governed_tool()`时
主机尚未授权确切的调用。

### 绑定一个精确的认知包

`CognitiveContextSession`是Agentic Ontology的专用边界
认知包。嵌入主机获得并保留A3S Use租约，
然后将提供程序与经过审查的包、生命周期一起注入
生成、能力快照和知识面摘要：

```rust,ignore
use a3s_code_core::{CognitiveContextSession, SessionOptions};

// `binding` is reconstructed from the exact A3S Use capability snapshot.
// `use_provider` implements CognitiveContextProvider and performs cited
// search -> bounded Markdown read through the host-owned generation lease.
let cognitive = CognitiveContextSession::new(binding, use_provider)?;
let options = SessionOptions::new().with_cognitive_context(cognitive);
```

代码在每个提供者请求中重复完整的绑定，验证源
和即时注入之前的引文摘要，使结合持续存在
`SessionSnapshotV1`，并将 `cognitive_context_bound` 发射到普通运行中
事件流。恢复的会话需要宿主注入相同的绑定。
提供者失败、世代漂移、缺少引文或尝试添加
一般 RAG/图回退会中止转弯；个人记忆不被召回
认知包束缚的转变。注册表查找、安装、生命周期、
包文件和人工审查本体图仍保留在代码之外。

已经计划完整 A3S Use生成的宿主可以上演相同的
值为`CapabilityValue::Knowledge`。仅将一个值复制到每个值中
运行冻结配置。 `current_cognitive_package_binding()`报道
拥有的绑定对下一次运行可见；年长的
`cognitive_package_binding()` 访问器仅报告会话静态恢复
种子。在简历中，先发布一次确切的种子，然后再进入稍后的种子
知识生成。

包宿主可以单独暂存多个
`CapabilityValue::KnowledgeSurface`值。每个不可变绑定包含
仅公开表面名称、OKF 格式、内容摘要和规范精确
投影摘要。它是不可查询的，并且永远不会进入 Agent 上下文；它的
目的是关闭同源就绪边缘，例如`Flow -> OKF`，而不需要
隐式选择认知包。上面的奇异 `Knowledge` 值
仍然是唯一运行可见的认知权威。

### 绑定已审核的 A3S Use运行时任务

`UseRuntimeTaskProjectionAdapter` 消耗一个精确的 `toolTasks` 条目
A3S Use能力快照。嵌入主机提供
`UseRuntimeTaskDispatcher` 适配器由 A3S Use租赁支持
`RuntimeTaskDispatcher`，然后将适配器放置在其匹配的工具下
`CapabilityId` 与其余部分相同的 Use-backed `SessionCapabilityBatch`
那一代人。代码从不执行预计的命令本身或写入
工具进入可变兼容性注册表。

每次调用都会重复快照、范围、包和清单摘要，
有界 argv 下的生命周期生成、提供者和表面身份，
截止日期和产出契约。不匹配的响应无法关闭。的
`SessionCapabilityBatch` 保留 Run 的确切使用生成租约，
而调度程序通过运行时输出保留其包注册表租约
捕获和清理。

## 尊重工作区的工具

仅当工具的工作区公开其所需的功能时，工具才会被注册。
仅对象后端不会将本地 `bash` 或 `git` 定义通告给
模型。

|关注|内置表面|
| ------------------------ | | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
|文件和目录|预算单/多文件 `read`、`write`、可预览 CAS `edit`、`patch`、`ls`，以及统一的 `search` 与 `grep`、`glob`、zvec-rust FTS/BM25、语义诊断和混合检索模式|
|命令和源代码控制|有界 `bash` 加上类型化 `git` 操作、取消和 Unix 进程组终止 |
|代码情报 | `code_symbols`、`code_navigation`、`code_diagnostics`；源代码读取和突变保留在文件工具中|
|网络证据|质量门控无头 → HTTP/RSS → API `web_search` 具有共享准入、会话电路和请求合并；加上有界 `web_fetch`、源标准化和 SSRF 保护 |
|下载 |工作空间限制的二进制`download`，具有严格的范围验证、有限并行性、重试、校验和和原子发布 |
|成分|安全`batch`、沙盒化QuickJS`program`、结构化`generate_object`、统一`task`委托；隐藏的 `parallel_task` 别名保持主机兼容 |
|可扩展性| `Skill`、`search_skills`、命名空间 `mcp__<server>__<tool>` 和显式 `dynamic_workflow` |

每次调用都声明`ToolCapabilities`，包括只读，
幂等、可恢复、取消安全、分页、输出类型和并行
限制。 `batch` 在同一个 `step` 中同时运行安全只读调用，并且
在依赖步骤之间等待；突变和未知工具被序列化。
通用 `program` 工具仍然是实现更丰富的边界控制的逃生舱口
流，因此这种分阶段形式不会引入第二个工作流引擎。

每个受治理和直接的工具结果也带有可信的
`metadata.a3s_tool_result_evidence` 使用模式
`a3s.code.tool-result-evidence.v1`。有界记录区分原始记录
和模型可见的字节/令牌估计，将精确的重复内容与
SHA-256 `repeat_key`，命名估计器，声明损失模式，并点
到授权的不可变的完整输出引用，本地兼容性
工件，或内联摘要。这是观察证据：核心没有
声明提供商计费使用情况，并且不会重写这些工具内容
测量。

内容投影由会话固定的单独控制
`a3s.code.tool-result-transform-policy.v1`政策。保守默认
保留 100 KiB 前缀。 `ToolResultTransformPolicyV1::context_efficient()`
保留 UTF-8 安全的 64 KiB 头部和 32 KiB 尾部，折叠精确的重复行，
并采样超大的顶级 JSON 数组。 Rust、Node.js、Python 和 Go
公开相同的策略字段。该政策在`SessionSnapshotV1`中仍然存在，并且
恢复拒绝明确不同的策略，因此重放不能默默地改变
模型可见的工具结果。

每个跨越真实 Tool 执行器的结果还带有
`metadata.a3s_tool_result_transform_binding` 带架构
`a3s.code.tool-result-transform-binding.v1`。绑定记录了准确的
算法，完整策略的域分离摘要，以及它自己的
结合消化。代码在调用工具之前解析并验证它，因此
未绑定的结果在副作用发生后无法释放。快照加载
根据会话策略和匹配工具验证每个保留的绑定
结果证据。绑定标识了 Code 的确定性变换；它
不主张云策略权限、租户身份或提供商选择。

### 保留原有工具内容

托管 Rust 宿主可以将会话绑定到已授权的共享内容
无需传递提供者凭证、租户查找或原语的权限
后端选择器进入核心：

```rust,no_run
use a3s_code_core::{
    ImmutableContentAdapter, ImmutableContentAdapterBindingV1,
    ImmutableContentAdapterSession, ImmutableContentResult, SessionOptions,
};
use std::sync::Arc;

fn session_options(
    authority_digest: String,
    adapter: Arc<dyn ImmutableContentAdapter>,
) -> ImmutableContentResult<SessionOptions> {
    let binding = ImmutableContentAdapterBindingV1::new(
        authority_digest,
        16 * 1024 * 1024,
    )?;
    let retained_content = ImmutableContentAdapterSession::new(binding, adapter)?;

    Ok(SessionOptions::new().with_immutable_content_adapter(retained_content))
}
```

权威摘要是不透明且不含密钥的。主机适配器接收到
精确的描述符加上借用的字节，并且必须创建或解析一个不可变的，
内容寻址对象。代码验证返回的绑定、URI、SHA-256、
发布工具结果之前的媒体类型、大小和参考摘要。每个
保留工具返回的原始输出，包括无损有界结果；
从内联元数据中删除的大变化部分将单独保留。
提供者失败、取消、字节上限溢出或参考漂移
如果没有本地副本，则无法关闭。完整参考资料位于
`metadata.artifact.content_reference`，并且
`metadata.a3s_tool_result_evidence.content_ref` 指向相同的 URI。

`SessionSnapshotV1` 仅保留不可变内容绑定并需要
恢复时重新注入的确切适配器。被委托的孩子继承它。
如果未配置适配器，则现有的有界会话本地
`ArtifactStore` 仍然是有损原件的独立兼容性路径；
它不是共享内容或授权机构。云依然存在
负责授权、提供者和命名空间选择、租户
预测、保留和对象生命周期。参见
[Harness Boundary Evidence](manual/HARNESS_EVIDENCE.md)。

### 上下文高效的仓库工具

`read` 可以将 1-32 个已知文本文件打包到一个有序响应中。所共享的
预算包括标题和延续本身，因此结果达到
模型完整而不是依赖下游截断：

```json
{
  "files": [
    { "path": "src/lib.rs" },
    { "path": "src/config.rs", "offset": 40, "limit": 80 }
  ],
  "max_output_bytes": 65536
}
```

如果预算已满，则将`metadata.batch.continuation`复制回`files`。
偏移量和每个文件的剩余限制会提前，而无需重复完成
线。在其自己的段中报告了一个缺失或不可读的成员，同时
其他文件继续。

对于 `search` 与 `mode: "grep"` 的调用，`output_mode` 控制多少
证据进入上下文：

|模式|结果 |
| -------------------- | -------------------------------------------------------------------- |
| `content` |与可选上下文匹配的行（默认）|
| `files_with_matches` |仅限词法游标分页匹配路径 |
| `count` |每个文件的词法游标分页匹配行计数 |
| `summary` |没有渲染匹配的全扫描行和文件总数 |

非内容模式要求内置工作区后端计算匹配项，而无需
构造丢弃的匹配文本。在 `mode: "glob"` 中，`search` 保留了
默认后端的最近度或相关性顺序；请求`sort: "path"`时
光标页需要稳定的词汇顺序。使用 `mode: "bm25"` 表示有界
zvec-rust FTS/BM25 对工作区文本块的词汇排名。启用检索
清单支持的本地工作区构建一个有界的、会话本地块目录
异步并跨查询重用其 FTS 发布。类型化并选择加入
`WorkspaceLexicalEngine::ZvecRust`选择器可以使用官方的`zvec-rust`
当产品版本提供经过验证的`libzvec_c_api`时进行绑定。最小
`--no-default-features` 构建使用明确报告的可移植 BM25
实施；产品构建永远不会默默地切换引擎。
会话构建不等待索引； BM25 透明地使用
当第一个快照被接纳时，会话本地目录记分器。一次
持久生成已准备就绪，相同的 `bm25` 调用切换到其 zvec 帖子
无需更改模型可见的工具合约。目录路线分数
无需查询时文件读取。本机路由验证其有界结果
针对实时文件系统的候选者，因此编辑不会泄漏过时的文本。
自定义工作区后端使用相同的选定代码本地 BM25 记分器。
CPU 密集型标记化和每个文档规范化阶段使用 Rust
有界 Rayon 工作池并保留输入顺序，而本机收集
出版物在其原子生成边界后仍保持序列化。

当启用自动持久投影时，冷目录准入使用
便携式记分器作为其经过验证的后备方案，而不是打开一个本机
每个源文件的集合。工作区范围内的 zvec 生成仍然是
本机服务路径一旦准备好，因此启动成本与源字节成比例，而
面向模型的搜索契约保持不变。

需要显式 zvec-grep-style 重启持久性的宿主可以使用
`WorkspaceServices::local_with_indexed_retrieval`；默认本地代理
工作区自动使用相同的路径。这保留了相同的清单
观察者和块准入策略，在下面写入版本化的 zvec 代
`.a3s-code/index`，并让现有的 `search` `bm25` 模式使用该索引
自动。默认本地代理工作区现在会尽最大努力
配置；不可用或只读的缓存会回退到目录。
显式构造函数仍然是为了兼容性方便，因此框架
用户不需要选择加入或知道缓存是否可用。的
持久索引属于工作区所有且仅限 FTS；会话语义向量
仍归 A3S 内存所有。 MCP 不是必需的，也不属于核心的一部分
依赖图。
生成发布已脱离查询路径：已更改的内容快照
内置分段并自动升级，同时进行相同内容的源修订
重用现有的本地帖子。目录更新仅将最新的加入队列
通过短稳定窗口进行快照，因此编辑器保存突发不会
每个中间版本触发一个完整的本机构建。瞬态本机或
文件系统故障以有限退避重试，状态表面报告
当一代正在构建时，并且过时的一代在之后被收集
新的`CURRENT`已发布。发布资格入口点为
`core/examples/workspace_persistent_index_benchmark.rs` 用于孤立索引
计时和`core/examples/workspace_persistent_index_production.rs`
真正的清单支持的工作空间。后者报告发现/接纳，
并发热查询 p50/p95、相同内容生成重用、更改内容
发布、生成清理和重新启动。完整的地方门
是`scripts/workspace_search_production.sh`；发布时通过`--full`
入学需要完整的核心单元套房。

对延迟敏感的宿主可以构造
`ManifestWorkspaceBackend::new_deferred` 或
`new_deferred_with_access_policy`。后端保留普通的本地回退
当其清单为空时，搜索可用；呼叫
`backend.manifest().activate()` 打开一扇单向门，开始初始阶段
扫描和平台观察者。这让终端或 GUI 主机渲染它的第一个
存储库规模发现开始之前的交互框架不会减弱
工作区访问或更改急切的构造函数。

兼容性默认值是确定性的、非重叠的、UTF-8 安全的
行/字节分块（80 行或 64 KiB，每个文件最多 128 个块）。打字的
策略还支持固定字节窗口、递归调用者排序分隔符
具有有界重叠，以及 Rust 宿主提供的自定义范围分割器。代码
验证完整覆盖范围、转发进度、UTF-8 边界和所有大小
预算，然后拥有稳定的 ID、行锚、摘要和修订。重叠是
计入保留文本和矢量记录预算。目录快照是
不可变并排除生成的、非文本的、超大的、凭证、密钥和
`.a3s` 控制路径。在替换工作之前，文件更改会被逻辑删除；一个
读取失败会减少索引覆盖率，而不是返回过时的文本。的
目录是会话本地的，并与其清单支持的工作区一起发布
后端；其可选的持久 zvec 投影是一代版本的
工作区根目录。跨 UI 共享 `ManifestWorkspaceBackend` 的主机，
搜索，并且会话将其目录配置一次
`configure_chunk_catalog` 连接前`local_with_retrieval_backend`；
会话选项不能
默默地取代东道主拥有的战略或其预算。

基线工作区搜索不需要嵌入或重新排序模型：
精确、glob、zvec-rust FTS/BM25、代码智能和 RRF 在本地执行
当工作空间检索被省略时，CPU 和保持可用。密集语义
搜索必然需要文本到向量的函数，但该函数可能是
宿主注入的进程内 CPU 回调；它不需要是远程的或使用
图形处理器。可选的确定性 MMR 重新排序器也是无模型的 CPU 代码。

宿主可以实现公共`EmbeddingProvider`特征，而无需添加
模型运行时到 A3S 内存或代码核心。 `EmbeddingExecutor` 验证提供者/模型
描述符，确定性地批处理调用者承认的文本，强制文本和
调用前的预期向量字节预算、传播取消、应用
类型化有界重试，并拒绝部分、重复、未知、维度-
不匹配、非有限、非标准化或描述符漂移的响应。输入
文本和向量值是从代码拥有的 `Debug` 输出和错误中编辑的。
`SessionOptions::with_workspace_retrieval(WorkspaceRetrievalOptions::new(...))`
将该合约绑定到会话。 A3S 内存是单一精确的、会话拥有的
语义服务投影。嵌入经过一次验证，插入到
有界内存分区，并随会话释放；没有重复的
矢量投影或隐藏权限选择器。词汇投影是
独立并使用 zvec-rust FTS/BM25，因此词法故障只能降级
词汇覆盖率，而语义结果保留其记忆契约。
代码重用已承认的块目录，
开始索引而不延迟 `session_async`，合并来自
跨文件生成相同的目录，直至配置的输入，文本字节，
和预期向量字节限制，并将完成的文件发布为原子 A3S
内存分区。跨提供商批次拆分的文件仍未发布
直到每个向量都通过响应验证。较新的目录修订版
取消并丢弃未发布的一代而不更改已经有效的
分区。 `WorkspaceRetrievalOptions::with_semantic_readiness_timeout(...)`
可选地为第一个语义或混合查询提供有界的、事件驱动的等待
让当前这一代人做好准备或退化。省略保留
兼容的立即部分回退；硬性最大值为 30 秒，呼叫者
取消和会话关闭中断等待和会话构建
保持异步。

`AgentSession::workspace_retrieval_status` 报告正在构建、就绪、降级、
或关闭状态、修订、覆盖率、队列深度、故障和向量内存。
它的 `batching` 对象添加了当前生成的文档输入和字节，逻辑
批次、物理提供商请求（包括重试）、三限制请求
下限、刷新原因、首次文件原子发布的时间以及
所需的非文本输入计数为零。关闭会话会取消提供者，
在配置的截止日期内加入拥有的任务，停止代码拥有的本地任务
显化工作，并丢弃所有矢量状态。启用会话添加
`mode: "semantic"`和`mode: "hybrid"`统一为`search`工具；残疾人
会话保留现有架构。语义查询使用有界提供者
执行并报告准确的目录/矢量修订和部分覆盖
后备。每个候选人都会通过`WorkspaceServices`重读以验证其
渲染源文本之前的完整文件摘要和确切的块字节范围。一个
过时的、已删除的、不可读的或同时被取代的候选者永远不会
暴露。

检索是显式主机功能，而不是模型控制的切换。铁Rust
主机使用`with_workspace_retrieval(...)`启用它，并且可以清除较早的
`without_workspace_retrieval()`分层选择；节点省略
`workspaceRetrieval`，Python 分配`None`，Go 使用`nil` 保留它
禁用。清除该选项不会构造任何索引，也不会进行提供程序调用。
只有清单允许的 UTF-8 文本和源文件才会进入块目录。
在分块和嵌入之前排除非文本资产；文档解析，
OCR、知识工件编译属于独立知识
编译器边界。参见
[Workspace Retrieval Chunking](manual/WORKSPACE_RETRIEVAL_CHUNKING.md) 为
策略选择、自定义范围不变量、异步构造以及
有界重叠感知重排序器。

Node、Python 和 Go 公开类型化行、固定 UTF-8 窗口和递归
分隔符感知策略对象。省略会导致行分块；没有SDK接受
一个原始的策略名称。共享的跨 SDK 夹具锁定相同的字节
范围和无效窗口行为，而任意自定义范围回调
仍然是值得信赖的 Rust 宿主扩展。策略验证先于提供商
执行，Go 在回调注册之前完成它。

混合模式创建独立的精确文字、zvec-rust FTS/BM25、可选代码
情报符号和积极相似语义候选列表。它
将基于一的等级与倒数等级融合（`k=60`）融合，而不是混合
未校准的分数。精确的 ASCII 标识符令牌占据受保护层；
确定性的平局断路器和每个文件两个块的上限保留仅 RRF 的结果
稳定。 Rust 宿主可以显式启用第二个内存中确定性
与`WorkspaceRerankOptions::deterministic()`一起登台。它最多检查 100 个
融合候选者，每个样本最多 4 KiB 和 128 个词汇指纹
候选者，将间隔/样板相似性与 MMR 风格的多样性相结合，
并且最多使用 4 MiB 的已检查暂存。准确的标识符仍然受到保护；
无效的配置或临时预算失败会保留 RRF 排序。
Node 和 Python 宿主通过传递类型来选择加入
`DeterministicWorkspaceReranker`至`WorkspaceRetrievalOptions`；去分配
`NewDeterministicWorkspaceReranker()` 到类型化的 `Reranker` 字段。省略
该对象仅保留 RRF，并且没有 SDK 接受原始模式或算法名称。
所有四个限制在嵌入/源出口之前均经过验证；另外去
在回调注册之前验证它们。
仅 RRF 仍然是兼容性默认值。现在的 Core real-DeepSeek 矩阵
在确定性条件下限定行、固定窗口和递归分块
阶段；有效的整个文件 Rust 自定义拆分器仍然是明确的否定
控制。真正的 CLI ACL-主机组成和公共 Node.js、Python 和
Go SDK 现在还可以通过递归 512/64 以及针对 1 的确定性重新排名
版本化语料库和标准化报告契约。每个SDK完成3/3精确
Recall@5 1.0、MRR 0.5、1.0x 文档请求的任务和工具协议
放大、零非文本输入以及完整的关闭后矢量释放。
这些三任务奇偶校验运行不符合新的默认值。
2026 年 8 月 17 日以代码 `5aa9642` 发布后的 `v7.0.1` 重放重复了所有九个
通过公共 Node.js、Python 和 Go 执行精确任务和单搜索协议
SDK。这三个手臂消耗了 14,540、14,784 和 14,171 个 DeepSeek 代币，
分别； Recall@5、MRR、请求放大、非文本出口和
收盘后发布的数据保持不变。同样的结帐也通过了全部三个
Core DeepSeek 对抗场景和 Node.js/Python 真实配置烟雾
路径。请参阅
[cross-SDK evaluation](sdk/evaluation/README.md#v701-post-release-rerun) 对于
完整的诊断时序表和再现命令。
单独的编译门控生成矩阵结合了锁定的多语言
使用存储库授权的 DeepSeek 路线嵌入模型并通过 9/9
仅目标 Rust 跨三个任务进行编辑，威尔逊下限为 0.7008，
Recall@5 1.0，隐藏测试编译，1.0x 提供者放大，增量
更换，并完全释放。 64 代流失门验证了
更改的文件替换而不是累积向量。这些结果符合
有界选择加入生成工作流程；他们仍然没有证明自动的合理性
启用。参见[operations runbook](manual/WORKSPACE_RETRIEVAL_OPERATIONS.md)
用于 SLO 和回滚。

A3S CLI 还附带了一个合格的、默认关闭的 `local_cpu` 主机适配器
Linux x64/ARM64、Windows x64 和 Apple Silicon。它承认一个单独的
安装的修订版和 SHA-256 绑定的 FastEmbed/ONNX 工件集，不执行
运行时下载或源出口，使用两个输入微批次和一个本机
每个进程都有一个作业，并且在模型加载到不受支持的 x64 CPU 上之前会失败。本地人
[CLI CI](https://github.com/A3S-Lab/CLI/actions/runs/31917686424) 真实表现
对每个启用的离线推理、取消、恢复和 RSS 检查
目标。锁定的多语言 DeepSeek 任务仍为 Recall@5 1.0 的 3/3，
精确的 1.0 倍请求放大，以及零非文本输入。这增加了一个
嵌入路由，而不是新的默认排名：仅 RRF 保持兼容并且
确定性重新排序器仍然是可选的。

结果报告版本化算法、选择/冗余分数、
候选者和字节记账、截断和回退，无需公开查询
或源文本。融合和重新排名先于权威源访问，因此
每个选定的路径最多重读一次以获得完整摘要和精确字节范围
验证。此特定于代码的策略不是通用 A3S 内存的一部分
向量核。

一个单独的版本锁定真实嵌入模型矩阵现在证明了为什么提供者
兼容性和模型适应性是不同的门。英语 MiniLM 错过了
CJK 任务，而多语言 MiniLM 检索所有三个目标。同上
实向量，仅 RRF 保留排名 2/2/2 和确定性重新排名移动
它们为 5/2/3，因此模型选择是宿主拥有的，既不是模型也不是
可选的重新排序器从这个小装置开始在全球范围内推广。

锁定的九查询夹具保留了原始的 BM25 基线并添加了一个
独立的混合结果集，其确定性提供者仅承认
带注释的查询/文档对。 Hybrid Recall@10 和 MRR 为 1.0
固定装置，在不降低标识符排名的情况下将 Recall@10 提高了 33.3 点。
选择加入确定性阶段还记录 nDCG@10 1.0 和零选择
锁定装置上的几乎重复的证据。发行 25,000 张唱片
分析其与 RRF 的两个端到端签名 p95 差异为 -5.163 ms，
-2.322 ms（两次运行中均为 0 ms 正加法），保守估计为 75,346
占暂存字节并且没有回退。这种嘈杂的配对测量证明
预算，而不是算法加速。

使用 `edit` 和 `dry_run: true` 来接收准确的之前/之后的差异，而无需
写作。试运行被声明为只读，并且可以安全地进行批处理。应用
结果与 `expected_replacements` 和可选的 `max_replacements` 拒绝
在比较和交换写入之前发生陈旧或意外的广泛更改。

网络证据保留了类型化的重试决策。 `web_search`记录等级品质，
引擎结果、持续时间、电路状态和重试上下文； `web_fetch`
将传输失败和 HTTP 429 分开分类并保留
可解析的`Retry-After`延迟。这两条路径都不会从渲染中推断出可重试性
错误散文。

### 沙箱与凭证边界

本地会话自动附加 A3S 拥有的、失败即关闭的
`sandbox::native::NativeBashSandbox`。由独立机构支持
[`a3s-sandbox`](https://github.com/A3S-Lab/Sandbox) crate，它限制写入
活动工作区和专用运行暂存空间，保护代理控制
元数据、阻止常见凭证读取、清除环境机密并拒绝
命令网络访问、本地绑定和 Unix 套接字。它使用安全带
macOS、user/mount/PID/IPC/UTS 命名空间以及 Linux 上的 seccomp 和 AppContainer
加上 Windows 上的作业对象。
不涉及 Node.js 或 npm 沙箱运行时，并且本机不可用
边界由仅错误沙箱句柄表示，并且永远不会回退到
未沙盒的主机运行程序。顶级工具、工作流程和委派子运行
继承相同的句柄。主机仅使用`SessionOptions::with_sandbox_handle`
用另一个等效的隔离边界替换默认值。非本地
工作空间运行者保留其明确的宿主拥有的契约，并且只有
明确授权的`require_escalated`调用可以使用本地主机
命令跑步者。

Shell 隔离不会自动管理进程内文件工具。本地
主机应明确选择`LocalWorkspaceAccessPolicy::CredentialBoundary`
当直接工作区操作需要相同的凭据边界时。请参阅
[Advanced Developer Manual](manual/ADVANCED_DEVELOPER_MANUAL.md) 完整版
契约和东道主的责任。

## 上下文、记忆与模型

`ContextAssembler` 对文件系统、最近文件、ripgrep、内存进行排名和预算，
提示槽、项目指令、技能和自定义提供者输入。自动
压缩是可选的，并且可以在长时间会话中重新准备。它保留了最新的
请求和未解决的工具调用，同时将生成的摘要视为
不受信任的转录数据。

记忆将工作状态、短期状态和持久状态分开。当记忆活跃时，
语义提取默认启用，也可以禁用。它只记录
验证可重复使用的记忆的来源、置信度、范围、原因、工作空间、
会话和模式元数据，而不是机械地保留每个工具
结果或对话轮转。取代保留旧的 V1 项目以供审核
但将其排除在召回之外。

主机还可以安装一个绑定到一个类型的`DurableMemorySession`
确切的 A3S Memory V2 租户、主体和范围。目前的
`ShadowCandidates` 模式仅镜像成功的 V1 提取写入为
内容寻址、证据支持的 `Candidate` 节点。它永远不会激活或
调用 V2 节点，因此可以在不更改模型上下文的情况下测量迁移。
选择加入`ActiveRecall`模式另外查询仅显式激活
有界词法策略下的节点。宿主可以选择有界的、一跳
显式 `RelatedTo` 边上的扩展；代码从不遵循冲突边缘，
在图表中递归，或者扩大确切的命名空间。公众
`preview_recall` 诊断是纯粹的，无法授权即时注入。
绑定的 `a3s.memory.lexical.word-cjk-bigram.v1` 配置文件保留小写
单词匹配并为连续的汉字、假名、韩文添加重叠的二元组，
以及相关的 CJK 运行。它改善了同语言短语的变化，而无需
它本身声称跨语言或无令牌重叠语义检索。铁Rust
宿主可以显式附加`DurableMemorySemanticRecall`：代码执行
修订固定嵌入提供程序，搜索调用者拥有的 A3S 内存向量
索引，然后将每个向量命中视为不可信的候选者。它重新读取
确切的存储库命名空间，需要当前的活动修订版和内容
在确定性词汇/语义 RRF 之前进行摘要。语义失败保留
词法结果，索引保持惰性，除非主机明确
调用刷新或安装类型化的时间表。 `refresh_semantic_recall`获得
并重新计算节点和字节下完整的 Active-only A3S 内存快照
预算，将其嵌入索引外，自动替换确切的
命名空间/生成分区，并再次验证源。一个精确的
命名空间更改令牌让内置存储库执行最终证明
无需重新读取快照；返回 `None` 的存储库保留
原始第二个快照检查。 Drift之前需要分区失效
调用可以成功；传播无效错误并且没有收据
回来了。预发布失败会保留之前的完整分区。
成功的调用返回一个绑定源摘要/字节的无秘密收据，
可选的无内容源更改令牌、语义生成、节点计数、
向量修订、突变一致性和可选的精确向量索引
历史令牌。
所有发布、恢复和查询栅栏都会读取易出错的异步内容
`VectorIndex::observe()` 状态/令牌对。同步`index_status()`
表面仅保留作为本地缓存的诊断提示，因此持久
后端永远不需要阻止 Tokio 工作线程来满足兼容性 API。
克隆会话通过相同的方式序列化刷新和直接替换
实时生成锁。在后端广告原子索引修订版CAS，代码
在快照工作之前捕获基本修订，有条件地发布，以及
使用已发布的修订版有条件地清理。延迟独立
因此，运行时不能覆盖或删除新一代。生产
房东可以拨打`refresh_semantic_recall_requiring(IndexRevisionCas, ...)`
在存储库或嵌入工作开始之前拒绝较弱的后端。
Rust 宿主可以在现有拥有的内存中安装`ScheduledSemanticRefresh`
维护运行时间。主机选择间隔；代码拒绝缺失
生成前的语义绑定或非 CAS 后端，跳过错过的刻度，保留
克隆计划句柄上的最新成功接收，并完成
干净有界会话关闭期间的发布后验证。之后
第一次发布，同等精确的命名空间更改令牌，语义生成，
所有权纪元收据、CAS 捕获的修订版和完整索引状态证明
没有快照、嵌入或向量突变的无变化刻度。一个后端
如果没有令牌，则保留完整的有界快照验证。一个代币
更改会触发一个完整的活动快照；如果仅更改非活动状态，则代码
预收收据而不重新发布。源或索引漂移执行
完全验证的重建，并且替换所有者无需先前的即可开始
处理本地收据。需要跨所有者恢复的宿主可以序列化
`receipt.checkpoint()` 并将解码后的值传递给
`ScheduledSemanticRefresh::try_new_with_checkpoint`。故意设置检查站
省略存储库更改令牌，因为它仅在一个内有意义
存储库历史记录。它的第一次恢复运行总是验证一个完整的活动
快照和当前索引。只有平等的源身份、语义
同时生成、完整索引状态和精确向量索引历史标记
修订可以避免提供者和出版工作；在那次促销之后，下一次
稳定的蜱可以使用正常的零快照命名空间令牌路径。缺失或
不同的向量标记、不相关的存储库或向量历史记录、冲突索引
状态，或任何漂移都会触发完整的验证重建。直到这个证明
成功，`last_receipt()` 仍为空。重建保留一界，
主动所有权时代的无文本向量集。精确的语义记录 ID
将重用绑定到命名空间、生成、节点、修订和内容摘要，因此
仅索引漂移可以重新发布，无需提供者适配器输入和部分
源代码更改嵌入仅在原子发布完整之前错过
分区。只有发布后验证成功才能替换此缓存。关闭
释放其向量，同时保持收据可观察；直接显式刷新
保持不缓存且无条件。克隆的计划句柄也暴露有界
`metrics()` 当前所有权时代。累计计数器和最新
64 次运行区分已解决的已发布、未更改和失败的尝试，而
测量更改令牌请求和有效观察，快照节点/字节
读取、精确的缓存命中和未命中、
提供者适配器调用/输入字节，包括重试、发布工作、
和经过的时间。
这些观察结果不包含源文本、节点 ID、摘要、向量、提供者
身份或错误体；关闭保留它们以供检查和下一个所有者
开始一个新的空纪元。适配器边界计数并不遥远
传输或计费；主机将它们与提供商遥测相关联。

启用Code的`durable-memory-sqlite`功能的Rust宿主可以注入A3S
内存`SqliteVectorIndex`。它保留了准确的矢量历史记录，全局
在真正的关闭/重新开放期间修订 CAS、记录和完整性会计，因此
匹配的主机持久检查点可以使用一个源快照进行恢复，并且
不得重复嵌入或发布。后端是本地SQLite持久化；
在 Unix 和 Windows 上，复制或自动替换已关闭的数据库分支
下次打开时的历史标记。恢复必须替换数据库文件
而不是就地覆盖它。分布式租赁所有权，远程复制
CAS、故障转移和生产节奏仍然是主机资格。
仅释放的`durable_memory_semantic_refresh_benchmark`锁定本地
最初发布时的 10,000 个节点、384 维耐用性概况，
零快照稳定刻度、单节点源漂移、仅索引漂移、
主机同步检查点、真实文件/SQLite 关闭和重新打开、单一快照
恢复、热语义查询百分位数、磁盘上限和 Linux RSS。它
使用确定性进程内适配器并且明确不声明真实
嵌入质量、远程 CAS/租赁、提供商计费或远程故障转移。
激活需要独立的手册或验证证据。代码记录
最终上下文组装后接受准确的当前修订；一个
在模型调用之前删除不可记录或过时的项目。精确 V1/V2
内容重复项更喜欢经过审核的 V2 项目。锁定合成检索
固定装置通过关系扩展将 Recall@5 从 `0.60` 改进为 `0.90`，满足
预先声明的门，无需添加向量服务依赖项。一个单独的
产品夹具通过真实的驱动相同的无记忆、V1 和 V2 臂
`AgentSession`转：任务成功为`0.00`、`0.60`、`0.90`；接受写入
精确度和证据保真度均为`1.00`；冲突依然存在
非破坏性；并选择V2修订记录模型使用前的入场情况。
单独版本的多语言夹具可驱动真正的 `AgentSession` 转动
英语、简体中文、日语和韩语。它将 Recall@3 和 MRR 锁定在
`1.00`，每个任务一次模型调用，最多一个内存节点，零个候选
或外国命名空间泄漏。
然后，版本化的语义固定装置使用英语主动记忆和中文，
没有词汇重叠的日语、韩语和阿拉伯语查询。词汇方面
基线返回零正命中；类型化的语义召回达到 Recall@1
`1.00` 通过一次模型调用的真实会话，最多一个上下文节点，四个
持久的准入，以及零候选者、外国命名空间或陈旧向量
命中。其声明的单位向量验证服务机制，而不是生产模型
质量。
然后，版本化的多代理固定装置会绑定相同的确切版本
`DurableMemorySession` 到由一个文件支持的两个独立的 `Agent` 实例
存储库。单独的确定性主机环境故意发出
相同的本地运行 ID 序列；的
`a3s.code.memory.context.session-run-invocation-sequence-sha256.v2`个人资料记录
所有三届会议/运行招生，没有暴露候选人或外国校长
内容，允许一个代理在另一个代理关闭后继续，并重放所有内容
存储库重新启动后的三个入场。持久记忆永远不会被遗传
由被委托的孩子：共享仍然是东道国当局的明确决定。
然后重复重启固定装置关闭并恢复四个独立代理
三个完整的进程时期，每个会话保留一次运行。每个主机ID
生成器重置，因此保留的运行 ID 在 FIFO 之后会被故意重用
驱逐；所有 24 个不同的模型上下文仍然被允许。已验证的活动
修正仅到达最后一个纪元，不可变的历史记录保存在四个文件中
存储库打开，候选内容、过时修订内容和外国主体内容
保持缺席。与仍保留的运行的碰撞现在会在之前失败
模型使用而不是替换其历史。
内存构建本身不启动任何任务。配置的 V1 修剪，选择加入
经过验证的语义刷新和主机提供的整合作业仅在内部运行
会话拥有的`MemoryMaintenanceRuntime`；作业按照计划进行序列化，
错过的刻度被跳过，经过验证的无变化语义刻度避免嵌入和
发布、验证重建重用精确提交的嵌入和有界
每个时期的刷新工作是可以观察到的。干净
`session.close().await` 让发布的刷新完成源码验证
在最终抽排排水之前的总关闭期限内。维护保养
需要异步会话构建。整合工作仍然存在
对证据、乐观修正和幂等性负责； A3S内存
从来没有发明过这项政策。
免密V2命名空间、模式、召回策略、检索配置文件和
上下文身份配置文件保留在会话快照中。仅词汇
会话使用绑定模式 4。语义会话使用模式 5 并且另外
冻结语义权威摘要，精确嵌入修改和执行
策略、向量描述符、候选策略和融合配置文件。现场直播
存储库、提供程序和向量索引仍然由宿主拥有，并且必须注入
重启后再次；简历拒绝丢失或漂移的绑定，包括
查询、语义生成或准入身份算法更改。一个真实的
文件存储库测试还证明了激活前候选者的隔离，
激活后服务、访问历史重放和存储库发布
锁定会话拆卸。参见
[Durable Memory Integration](manual/DURABLE_MEMORY.md) 和
[Durable Memory Retrieval Evaluation](manual/DURABLE_MEMORY_RETRIEVAL_EVAL.md),
[Durable Memory Product Evaluation](manual/DURABLE_MEMORY_PRODUCT_EVAL.md),
[Durable Memory Multilingual Evaluation](manual/DURABLE_MEMORY_MULTILINGUAL_EVAL.md),
[Durable Memory Semantic Evaluation](manual/DURABLE_MEMORY_SEMANTIC_EVAL.md),
[Durable Memory Semantic Refresh](manual/DURABLE_MEMORY_SEMANTIC_REFRESH.md),
[Durable Memory Multi-Agent Evaluation](manual/DURABLE_MEMORY_MULTI_AGENT_EVAL.md)，以及
[Durable Memory Restart Endurance Evaluation](manual/DURABLE_MEMORY_RESTART_ENDURANCE_EVAL.md)
所有权、持久性、迁移规则、检索配置文件、向量
决策、端到端指标和声明的评估限制。

模型适配器规范文本、推理、图像、工具调用、令牌使用、
流式传输、取消和重试。结构化生成使用原生
提供者响应格式（如果可用）以及模式验证和修复
否则。面向提供商的模式仍然可用作仅主机验证
复合流客户端的元数据，并且客户端显式声明
阻塞结构化调用是否使用独立于它们的传输
流路径。 MCP 支持 stdio、SSE、可流式 HTTP、OAuth 客户端
凭据、刷新和实时会话范围的添加/删除操作。

## 无隐藏权威的编排

- 规划可以是自动的、强制的或禁用的；目标跟踪是可选的。
- `AgentDefinition` 和 `WorkerAgentSpec` 描述可重复使用和一次性
  工人在不削弱父母政策的情况下。
- 前台和后台任务公开进度、来源、结构化输出、
  取消和持久的任务记录。
- 顺序、并行、可恢复、循环、预算和检查点原语是
  可用于确定性主机工作流程。
- `program` 使用明确的工具、时间在 QuickJS 中运行有界 JavaScript，
  递归、调用计数和输出限制。
- `dynamic_workflow` 不在普通会话中，直到主机注册
  A3S Flow 支持的运行时。

动态工作流程可以独立绑定会话分叉的结构化生成
与 `maxConcurrentGenerations` (1-4);没有会话分叉的提供商仍然存在
单程。当会话公开 `ModelGenerationPool` 时，本地
bound 与共享调度程序中的提供程序池相交，因此
工作流程无法通过分叉客户端来绕过容量。流动阶梯体也接受 `maxConcurrentSteps` (1-32,
默认4); waiting 是取消感知的并且仅启动沙箱超时
入学后。每个步骤都有一个源自其运行的仅摘要标识，
步骤、处理程序和有界输入。持久的完成步骤恢复必然是
确切的运行 ID、原始查询和步骤 ID，而不是充当交叉运行
查询缓存。恢复运行会根据持久性重建其完整计划
历史记录，因此进度不会丢失当前进程之前发出的步骤。
新的运行还坚持精确的运行时构建要求；延续
身份是根据每个上持久的 RunCreated/StepCreated 事实重新计算的
恢复，而不在身份中存储源、输入或输出明文。的
工人声称身份故意排除不断变化的计划进度，因此
接管并重试使用一个稳定的摘要。本地索赔元数据位于
`.a3s/workflow/leases`； A3S Flow 的事件历史记录仍然是唯一的工作流程
权限，并且 sidecar 只包含有界摘要、所有者租约和
尝试状态。宿主可以通过以下方式获取控制句柄
`DynamicWorkflowTool::control(run_id, source, input, ctx)`。它`inspect()`
投影省略源、输入、步骤参数、输出和所有者令牌；
`history()` 是一个明确可信的完整历史逃生舱口。变异
控制操作获取与模型可见工具相同的工人租赁，
然后协调 Flow 的持久取消/终端转换并解决
租约。本地工作区历史记录由一个小的跨进程文件包装
锁定，以便独立工作人员无法破坏 JSONL 附加；乐观流
顺序冲突仍然保留重试权限。远程/数据库支持
宿主可以通过以下方式提供类型化的`Arc<dyn FlowEventStore>`
`with_flow_event_store`（或注册弱注册表助手
`register_dynamic_workflow_with_event_store`);然后代码使用同一个存储
对于模型可见的工具及其控制手柄，无需在内存中创建
影子日记。 `control.health()` 返回有界声明计数器，包括
此过程观察到的持久接管尝试； `control.diagnostics()`
该视图与可选的代理范围调度程序运行状况快照相结合。

委派的任务、工作流和技能子运行保留父沙箱并
将本地权限策略与父检查器相交。孩子自动批准
设置无法放弃主机升级边界。

## 事件、持久化与重放

`AgentEvent`涵盖文本、推理、工具、确认、计划、任务、
内存、压缩、预算、验证和最终状态。 SDK 并运行
通过 `EventEnvelopeV1` 重放投影这些值，从而保留其
版本、事件类型、完整负载和可选元数据。较旧的 SDK 客户端
可以保留他们尚不理解的未来事件名称和有效负载。

受控运行在工具中添加了五个摘要绑定审计事件，并统一
提供者中立的模型边界。 `tool_request_bound` 记录请求
工具的来源、序列化参数字节和域分隔摘要
标识符、名称和权限之前的确切后挂钩参数，
确认、预算或执行结果。因此，被拒绝的请求仍然存在
可审计，无需将其参数明文复制到新快照中。
`run_capability_bound` 记录实际模型-可见工具、工作区服务
表面、运行拥有的治理绑定、配置的可序列化策略
身份、执行上限和当前语义准备/生成；它
仅当该表面发生变化时才重复。在每次完成之前，流式传输，
结构化或流式结构化输入，`model_presentation_bound` 绑定
冻结类型配置文件，其经过权限过滤的源计数/摘要/令牌
估计，以及准确呈现的定义计数/摘要/令牌估计。的
随后的`model_input_bound`携带相同的唯一肯定呼叫序列，
有界计数器/序列化字节测量和域分隔的 SHA-256
实际消息、系统输入、工具定义、面向提供商的摘要
结构化指令，并识别语义/混合工具结果。每次之后
成功调用，`model_usage_bound`关联代码的提示估计和
规范化`LlmClient`令牌/缓存使用与确切的输入快照和
通过测量不同调用 ID 下精确重复的工具结果内容
有界字节/令牌计数器和摘要；它不要求网关计费
权威。仅主机验证模式被排除，因为它们不会被发送
到模型。新快照不存储工具参数、提示、工具结果、
源文本、向量、凭证或端点明文和精确的运行重放
无需并行审核存储即可保留它们。现有生命周期事件保留
他们记录的有效负载。摘要提供完整性和相关性，而不是
加密；不要仅仅因为将它们导出到不太受信任的边界
明文不存在。参见[Harness Boundary Evidence](manual/HARNESS_EVIDENCE.md)。

配置好的`SessionStore`可以持久保存完整的`SessionSnapshotV1`
几代人。运行公开状态、活动工具、有序事件重放、独占
分页光标和保留间隙检测。文件持久化使用原子性
更换；工件受项目计数和字节的限制；验证保留
主张与证据分开。

Rust 宿主可以映射一个完整的快照和一个可选的精确工具间映射
将 `LoopCheckpoint` 舍入到 `SessionCheckpointExportV1`，或注入类型化的
`SessionCheckpointExportSink` 直接获得相同的经典神器
来自每个已完成的动力工具轮边界。代码关闭功能 Turn，
排出所有因果先前的运行事件，捕获语义快照，并且
在循环前进之前确认持久性。如果会话目录减少
同时，此检查点视图保留源运行的冻结状态
认知绑定和完整范围的能力标识，而不是下一个
润一代。导出包含有界规范 JSON 有效负载以及
无秘密
单独绑定快照组件的`SessionCheckpointDescriptorV1`，
逻辑恢复组件，并按大小和 SHA-256 完成有效负载。进口
重新计算每个绑定并拒绝非规范字节、模式漂移、更改
轮数、外部会话、缺失或终端源运行以及描述符
漂移，包括会话/源运行认知或功能不匹配。运行时 API 密钥
仍然被现有的持久会话排除
合约，导出的 `Debug` 表示会编辑有效负载字节。代码
不分配对象 URI、检查点 ID、保留规则、批准或分叉
血统；授权主机存储字节，云拥有这些业务
记录。参见[Harness Boundary Evidence](manual/HARNESS_EVIDENCE.md#portable-session-checkpoints)。

对于恢复入院，`AgentProtocolRunRecoverExactV1` 携带完整的
`SessionCheckpointDescriptorV1`。 `AgentProtocolHost` 验证并固定
匹配之前的会话执行租约下的本地`LoopCheckpoint`
捕获工作区基线或创建目标运行。完整的请求
摘要结算收据，而描述符摘要是目标的一部分
Run 的不可变输入标识：覆盖的边界将被拒绝，而无需
new Run，相同的请求在源保留后仍然可重放，并且
另一个检查点无法重用该目标运行 ID。

`AgentProtocolHarness::execute_checkpoint_recovery()` 另外匹配
描述符精确提供的字节，解码语义快照并
从该一个有效负载的逻辑边界，构建一个未发布的会话，并且
仅在精确运行准入成功后才发布。它执行不
快照加循环预写。没有目标运行的持久会话必须
匹配检查点的语义生成；已经持久化的目标使用
正常的精确重放/冲突规则，并且不相关的实时会话永远不会
更换。这是一种可见的准入，而不是外部数据存储
交易：云仍然拥有检查点授权和修订/CAS
与其他作家进行击剑。现有`AgentProtocolRunRecoverV1`
对于故意的主机，命令和 HTTP 线路契约保持不变
请求最新存储的边界。

每个新的逻辑检查点还携带`RunCapabilityBindingV1`：确切的
代码目录生成和摘要，规范完整权限上限
摘要，以及任何精确的 A3S Use光标。恢复固定并比较该身份
在保留目标运行之前，因此 N 个检查点无法通过 N+1 恢复，
即使在切换比赛准备时。主机恢复丢失的会话可以使用
`execute_checkpoint_recovery_with_capability_batch()` 重建一个精确的
来自未受影响的零代的历史一代。代码不接受
`latest`查找或部分批量；不匹配会导致会话和目标运行
未发表。

可选的状态图是一个补充协调运行时，不是隐藏的
会话状态：

```text
external or agent event
        │
        ▼
hash-linked GraphEventRecord log
        │ strict projection
        ▼
typed objects + typed relations
        │ matching behaviors
        ▼
optimistic GraphPatch → new version or explicit rejection
```

应用程序选择图形重放、分支、差异和流投影时
多个代理或行为需要一个可审计的共享模型。

## 运行时入口

|表面|套餐 |预期用途 |
| -------- | -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
|终端| [`a3s code`](https://github.com/A3S-Lab/CLI) |基于Core和共享[A3S TUI](https://github.com/A3S-Lab/TUI)构建的交互式编码产品 |
|铁Rust| [`a3s-code-core`](https://crates.io/crates/a3s-code-core) |完整的运行时 API 和扩展特征 |
| Node.js | [`@a3s-lab/code`](https://www.npmjs.com/package/@a3s-lab/code) |用于异步生命周期、流、工具、存储、编排、MCP 和状态图的本机 N-API 绑定
|Python | [`a3s-code`](https://pypi.org/project/a3s-code/) |具有同步和异步应用程序 API 的原生 PyO3/bootstrap 包 |
|去 | [`github.com/A3S-Lab/Code/sdk/go/v8`](sdk/go/README.md) | Pure-Go 客户端，具有用于会话、流、工具、临时语义检索、运行、验证和 MCP 的版本化本地桥 |

```bash
# Node.js
npm install @a3s-lab/code

# Python
python -m pip install a3s-code

# Go
go get github.com/A3S-Lab/Code/sdk/go/v8
```

v8.3.0 中的 Python 发布工作流程使用稳定的 `cp310-abi3` 接口，
Apple Silicon 针对 macOS 11+，Intel 针对 macOS 12+、Linux x86_64
在 glibc 2.28+ 上，以及在 glibc 2.39+ 上的 Linux arm64 (`manylinux_2_39_aarch64`)。
Windows x86_64 和arm64 轮子也已发布。每个原生轮都承载
匹配的 Moli sidecar，而纯 Python 引导程序将其提取到
共享验证缓存；因此，一个轮子涵盖了每个轮子上的 CPython 3.10–3.14
目标。

如果 `python3.14 -m pip` 报告 `No module named pip`，则修复该解释器
在安装 SDK 之前，然后安装到同一个解释器中：

```bash
python3.14 -m ensurepip --upgrade
python3.14 -m pip install --upgrade pip
python3.14 -m pip install a3s-code
```

在 Intel Mac 上，本机滚轮是为 macOS 12 (`x86_64`) 构建的。可选的
`local_cpu` ONNX 嵌入适配器不包含在英特尔 CLI 版本中；
保持检索模型自由或配置明确授权的远程
而是嵌入提供者。

本机 SDK 包明确启用了 Core `headless-search`、`s3` 和
`serve` 具有保留完整产品表面的功能。直接生Rust
嵌入器默认接收惰性 Moli 搜索层，并且可以省略浏览器
与`default-features = false`的依赖堆栈。 pure-Go 包使用
匹配`a3s-code-go-bridge`释放资产，无需CGO；桥束
对于每个受支持的 GNU/macOS/Windows 目标，包括匹配的 Moli sidecar。
所有官方 SDK 都公开相同的有序 `sdk-capabilities` 库存、事件
协议、状态图操作和 Moli 诊断/配置 API。使用
该契约用于协商可选功能。 Node.js、Python 和 Go 宿主可以注入类型化异步嵌入
会话拥有、内存支持的语义和混合工作空间的提供者
检索。提供程序取消遵循查询和会话生命周期，并且
没有 SDK 需要矢量数据库服务。仅允许远程嵌入
保守的源路径，
拒绝硬链接别名，并在读取时重新验证逻辑和解析路径
源可以离开工作区边界之前的时间。返回的片段是
根据当前权威来源重读和摘要检查。请参阅
[Node.js](sdk/node/README.md)、[Python](sdk/python/README.md)、以及
[Go](sdk/go/README.md) 针对特定表面示例和有意 API 的指南
差异。

## 架构

```text
Rust host / Node SDK / Python SDK / Go SDK / a3s code
                         │
                       Agent
                         │
                    AgentSession
        ┌────────────────┼────────────────┐
        │                │                │
 context + memory   model adapters   governed tools
        │                │                │
        └────────────────┼────────────────┘
                         │
       events + runs + traces + artifacts + snapshots
                         │
              optional StateGraph / Flow bridge
```

Core 拥有生命周期、排序和执行契约。公共扩展
边界包括`LlmClient`、`ContextProvider`、`MemoryStore`、
`SessionStore`，工作区服务特征，工具，权限，确认，
挂钩、安全性、MCP 传输和图形存储。

模型容量是同一执行契约内的资源边界。一个
会话的编排准入消耗一个全局调度程序槽；每个
提供者调用获取其类型的仅配额保留
`ModelGenerationPool`。两个决策都使用一个参与者和优先级队列，
它保留了全局公平性，同时允许嵌套模型调用
单槽运行下的进度。本地信号量和调度程序租赁是
由一份 RAII 许可证拥有，包括流媒体和取消路径。

可安装的认知包仍归 A3S Use 所有。代码消耗了他们的
精确的不可变能力生成和项目本地工具、技能、代理、
命令、Hook、MCP、上下文、流程、知识和 UI 值到类型上
具有原子发布和可逆效果的会话/运行范围。所有权、
生成、生命周期和兼容性契约定义在
[Scoped Capability Architecture](manual/SCOPED_CAPABILITY_ARCHITECTURE.md)。

身份切片在[`core/src/capability`](core/src/capability)中下发：
类型化使用包/光标和本地目录生成，密封源拥有
描述符批次和有界规范`CapabilitySet`。架构工程
返回一个不可变的`Arc`；空的产品投影仍然保留其用途
光标、混合光标、内置阴影、冲突、缺失
在读者可以固定之前，依赖关系和资源限制溢出会失败
设置。运行时值仍处于确定性身份类型之外。

生命周期切片添加密封`CapabilityScope<Session/Run/Turn/Subtask>`
标记和目录绑定 `CapabilityCeiling` 值。借用的打字租赁
不能比其他作用域类型活得更久或模仿其他作用域类型；子作用域只能删除
能力、工作区操作和执行预算，同时保留每一项
需要家长治理守卫。在使用支持的目录上运行必须消耗
确切的非克隆使用快照租约。它的主管拥有所有子范围，
任务、可逆效应和上游租约，然后在有界内关闭它们
与最后释放的使用租约相反的顺序。

执行组合切片使这些范围可操作，而不是
描述性的。现在，一棵取消树作为主机调用的根并被承认
能力层次结构。每个提供者响应及其工具调用共享一个
转；前台委托组成`Turn -> Subtask -> Turn`，同时显式
后台委托被提升到调用 Turn 之外，但仍然存在
运行监督。在释放确切的用途之前运行关闭工作的解决方案
租约，因此任何任务或可逆效应都不会悄无声息地逃脱其暂时的所有者。

投影切片添加一个封闭的`CapabilityValue`平面，不可变
`CapabilityProjection` 代，非克隆读者租约，以及
`CapabilityTxn<Staged/Prepared/Validated>`。只有经过验证的交易才能
通过目录的生成和摘要 CAS 提交。准备失败，
验证、取消或丢失的提交竞赛会离开当前一代
不变并将准备好的效果移动到有界反向清理。退役效果
保持固定状态，直到释放最后一个旧的投影租约。闭合值
飞机包含有界的`UiBinding`文件； UI 名称、内容摘要、表面-
摘要、角色、依赖类型和大小漂移在发布前失败。

交付的`HOST-CAP1`让会话应用完整的功能
通过`SessionCapabilityBatch`生成。发布以原子方式绑定
投影及其特定世代的 A3S Use租赁提供商。每次跑步别针
一个投影，冻结兼容性工具/技能图，获得一个新的
real 对精确游标使用快照租用，并使用相同的工具 `Arc`
模型定义和受控执行。旧的运行保留 N，而后来的运行看到
N+1；取消、关闭、准备失败和名称冲突不暴露
部分一代。每个承认的运行和实时检查点都保留一个规范
目录加天花板装订；恢复在目标准入之前验证它或
在新会话上执行一个精确的主机提供的引导程序。兼容性
工具、技能和 MCP 包装 API 不能
影子已发布的投影。 CLI 现在使用常驻批处理
会话和短暂的 Code Exec 运行时，该运行时会停止使用发现
运行入场。桌面探测并需要确切的主机契约，然后
仅接受规范代码目录和使用快照证据的成功。
知识通过单独持续的精确认知而被运行冻结
边界如下所述。流和 UI 通过精确的方式由主机消耗
`projected_flow` 和 `projected_ui` 手柄如下所述；两者都不是默默地
转换为模型可见的工具。

交付的`HOST-AGENT1`将该批次扩展到类型化的代理定义，而无需
将包权限移至代码中。每次运行都融合了兼容性和
共享时将代理投影到独立的`AgentRegistry`名称映射中
它们确切的不可变 `Arc<AgentDefinition>` 值；自动选择，
`task` 和 `parallel_task` 绑定到同一注册表。规范别名
无法跨越兼容性边界互相影响，以及后来的工作人员
或代理目录注册无法替换已发布的代理。安承认
N Run 在 N+1 发布后继续通过 N 进行委托并保留 N 的
精确的 A3S 通过前台子完成使用租约。

已交付 `HOST-COMMAND1` 扩展同一批并运行准入边界至
斜线命令。每个阻塞或流式调度都会冻结兼容性
命令映射，合并投影生成而不克隆命令对象，
并通过该快照执行。内置和兼容性名称冲突
在发布之前失败，包括旧的可变注册表路径。安尼
在 N+1 发布期间已执行的命令将继续执行 N 且
保留 N 的确切 A3S Use 租约，直到执行完成。

交付的`HOST-HOOK1`将批次扩展到不可变的`HookBinding`值
将一个 Hook 定义与其确切的处理程序配对。运行预计合并入学
具有冻结兼容性的绑定 Hook 快照并在
可选的会话静态外部执行器；外部`Skip`无法绕过
预计的政策。会话/技能生命周期事件类型在发布之前失败，
官方 SDK 注册自动更新定义和回调，并且
分离的观察加上超时阻塞回调在运行下解决
在其确切的 A3S Use租约在配置的范围内释放之前，主管
关闭截止日期。

交付`HOST-MCP1`将核心批次扩展到每服务器不可变
`McpBinding` 值。每个绑定都会冻结一个精确初始化的`McpClient`并且
一个排序的、有界的 `tools/list` 结果；运行包装器通过调用原始工具
该客户端而不是解析可变的`McpManager`。 N 次运行和前景
因此，委托子级保留 N 个定义和 N 个调用者，而
父运行保留 N 的独立精确 A3S 在 N+1 上使用快照租约
出版。连接准备是可逆的 代码效果，取消
无法推进目录，并且清理仅在之后关闭旧连接
最后的旧投影阅读器掉落。
适配器使用从已经派生的主机构造的配置
选择使用运行时/网关证据。代码不检查包，解决
不透明`gateway:*`端点身份，选择提供商，或自己使用切换，
路线排水和恢复。 A3S Use 和官方 CLI 现在可以精确预测
通过该接缝延伸 MCP 表面。一次性 CLI/桌面主机组成
仅当承认的 Streamable HTTP 时，其受信任的运行时/私有网关才会延迟
Surface 要求它解析不透明的提供者/参考/路径证据；仅限录音室
几代人都没有开始。它保留该进程拥有的主机，直到会话结束
关闭预计的客户端，然后关闭网关。通过
其余的官方主机保持单独的集成边界。

通过 `HOST-CONTEXT1` 承认一般 `ContextProvider` 价值观
同一批次并将其精确的 `Arc` 值复制到每个运行冻结代理中
配置。 N 上允许的运行保留了 N 个提供商和确切的 N 个使用租约
N+1 发布后。描述符/提供者名称漂移、冲突
会话静态提供者，并尝试走私持久认知包
在目录发布之前，通过常规上下文类别进行的绑定失败。
委派的孩子故意保持孤立的提示上下文，因此放弃
父上下文表面是单调的子范围缩小而不是查找
会话最新提供商。知识仍然是一种明显的精确权威切割；
UI 遵循下面独特的仅限主机剪切。

交付的 `HOST-FLOW1` 将匿名 Flow 运行时值替换为命名的
`FlowBinding`。 `WorkflowSpec::name` 是公共能力名称，
将精确持久规范与拥有其事件的 `FlowEngine` 绑定对
存储、运行时、观察者、重放和运行时构建兼容性。有主持人来电
`AgentSession::projected_flow` 接收保留精确信息的非克隆手柄
代码投影和A3S使用租赁。 N 句柄在 N+1 之后继续经过 N
出版；不兼容的运行时构建和描述符/规范名称漂移失败
在发布之前，缺少查找不会获得租约，并且会话关闭会取消
主动重放。除非有明确的受控工具适配器，否则流将保持仅主机状态
已安装。驻留 CLI 现在适应无依赖、依赖工具、
依赖于 MCP 和依赖于 OKF 的 A3S Use流程穿过此边界：它重新验证并
摘要阶段源，完成工作区本地本机 TypeScript 预检，
并发布与同包工具、MCP 和摘要绑定的精确绑定
知识面依赖性。
失败的预检或取消的工作区锁定争用留下了可见的痕迹
世代不变。动态多范围 OKF 搜索仍然是一个单独的
兼容性拥有的查询适配器而不是成为 Flow 权威。

交付`HOST-KNOWLEDGE1` 正好承认一个`CognitiveContextSession` 通过
原子会话批处理并仅在承认的情况下安装其确切的提供程序
运行配置。 An N Run 保留了 N 的认知提供者、包绑定、
和 A3S 在 N+1 出版物中使用租约。每个`RunSnapshot`都记录着自己的
精确的认知绑定，而`SessionSnapshotV1`记录可见的绑定
到下一次运行，因此旧的运行证据在切换后仍然有效。恢复或
会话静态提供者是恢复种子：第一个预计的知识值
必须在后代能够进步之前复制精确的绑定，并且
去除不能暴露陈旧的种子。多个知识权威和任何
与通用主机混合在发布之前上下文失败。知识
主机保留 OKF 验证、索引、检索、保留和精确查询
租赁所有权。

同一个门还允许多个不可变的`KnowledgeSurfaceBinding`值
作为仅准备就绪的证据。他们的规范摘要结合了格式、内容和
精确的预测摘要；它们不公开任何检索方法，因此不算作
认知权威。承认的运行使用相同的代码和使用来固定它们
代，允许相关主机能力拒绝丢失或混合
OKF 发布前的证据。

交付的`HOST-UI1`通过相同的方式承认不可变的`UiBinding`值
原子会话批处理。每个绑定都包含有界的、无路径的条目 HTML plus
有序 CSS 和 JavaScript 字节、验证内容身份、演示
元数据和规范的表面摘要。描述符名称和表面摘要
必须与绑定完全匹配，并且 UI 就绪边缘可能仅针对工具，
同一代的技能、MCP 或 Flow 值。有主持人来电
`AgentSession::projected_ui` 接收保留该精确信息的非克隆手柄
文档、代码生成和跨 N+1 出版物的新 A3S Use租赁；
缺少查找不会获取租约，会话关闭会发出取消信号。核心
不解析或渲染 HTML，拥有 origin/CSP/navigation/state，暴露环境
文件系统/网络/进程/秘密权限，或路由 UI 后端消息。
A3S Use 现在发布版本化、完整的规范 UI 依赖项和托管
MCP 证据，并且常驻 CLI 在符合条件之前重新验证它
托管 MCP、技能、提供者合格的运行时工具任务、依赖封闭
一批中的流量、知识面和 UI 值。工具、MCP 和
OKF 相关的 Flow 边缘针对相同的精确包生成进行解析；
提供者缺席或失踪
证据在发布之前就失败了，并且运行时工具和扩展 MCP 都没有
使用兼容性注册。官方渲染器主机仍然采用
单独的集成工作。范围内的 CLI/桌面主机有意保留
其更窄的托管 MCP/技能/UI 切割，以及惰性可信 HTTP 运行时/网关
组合和显式的 Session-close-before-Gateway-shutdown 生命周期。

已交付 `CAP-PROFILE1` 新增一封闭
`ToolPresentationProfileV1` 到会话并运行。自适应保留
历史提示敏感选择器，直接呈现每个权限可见
定义，代码呈现了现有的 `program` 工具，具有有界紧凑
签名目录，禁用则不显示。始终进行权限过滤
首先运行；轮廓投影无法添加工具名称或更改其参数
架构。确切的配置文件在简历中持续存在，委托运行继承
父上限，Node.js、Python 和 Go 公开类型化的 Profile 对象。这个
仅是一个模型表示平面：A3S Use 仍然拥有包解析，
赠款、生成、切换、租赁和恢复，同时受控执行
使用相同的固定工具`Arc`值。

就绪切片仅从
表面边缘已经存在于该不可变集合中。确定性最小
波浪为家属准备先决条件；循环和不完整的阶段
批次在任何适配器启动之前失败，而先决条件失败会阻止
相关激活并以相反顺序回滚已完成的效果。的
plan 保留集合的生成、摘要和精确使用游标边界。代码
不检查包清单或执行使用依赖项解析，
安装、拨款、生命周期切换或恢复。

来源按关注点分组为`agent_api/`、`tools/`、`workspace/`、
`context/`、`llm/`、`mcp/`、`orchestration/`、`store/` 和 `state_graph/`。
Node.js 和 Python 绑定在同一 Core 上保持独立的本机 crate。
Go SDK 通过一个长期存在的、经过功能检查的本地来到达该核心。
桥接过程。

## 文件系统优先的 Agent 与发布

`AgentDir` 使可重复使用的代理可以作为文件进行审查：

```text
agent-dir/
├── instructions.md
├── agent.acl
├── skills/
├── tools/
└── schedules/
```

工具规范可以连接MCP服务器或有界的PTC脚本。随着
`serve` 功能，宿主可以提供代理目录并运行 cron 计划。
可观察的守护进程句柄仅在计划、会话和
工具准备好； shutdown 取消正在进行的工作，关闭拥有的会话，以及
在有限的期限内加入。文件永远不会绕过工作区、权限、
确认或验证政策。

`AgentReleaseManifest`承认版本化的`.a3s/asset.acl`合约，推导
模式感知规范 ACL 和 SHA-256 身份，并验证运行时
激活前的兼容性。秘密声明是类型化的注入槽；
值保留在发布文档之外。 OCI 镜像构建完成后，
`bind_publication` 仅替换其工件摘要和确切声明的
出处参考，然后重新承认最终的规范清单。这样就避免了
将清单嵌入到其摘要的图像中是不可能的自我参考
该清单声明。

发布许可验证元数据。它**不**构建或运行 OCI
工件、实现健康行为或自己的部署生命周期。的
[minimal publication fixture](fixtures/agent-release-contract/README.md)
打包单独的 `a3s code harness` 可执行文件，发布一个 OCI 映像
清单，在摘要解析后生成最终的ACL，保留准确的ACL
由该 ACL 绑定的规范构建器出处对象，并且可以验证
通过本地 Docker 进行摘要固定的生命周期。阅读
[Agent Release Contract](manual/AGENT_RELEASE_CONTRACT.md) 整合前
v1 架构或声明外部运行时认证。

## 显式边界

- 核心是一个可嵌入的运行时，而不是托管代理服务或终端
  小部件库。
- 单独的 A3S CLI 拥有交互式 TUI、帐户适配器、演示文稿
  策略和可选的 A3S 操作系统集成。
- 托管自己的用户身份、凭证访问、部署策略和信任
  关于直接主机工具调用的决策。
- 沙箱、持久性、自动压缩、目标、委托和图表
  投影需要明确的主机配置；记忆提取残留
  主机可配置。
- 一个会话一次允许一个影响转录本的操作；并发
  发送、流式传输、附件、命令或恢复操作会快速失败。

## 文档

|指南|焦点 |
| -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| [User Guide](manual/USER_GUIDE.md) · [Chinese](manual/USER_GUIDE_CN.md) |安装、配置、会话、工具和常见工作流程 |
| [Advanced Developer Manual](manual/ADVANCED_DEVELOPER_MANUAL.md) · [Chinese](manual/ADVANCED_DEVELOPER_MANUAL_CN.md) |扩展契约、安全性、生命周期和生产集成 |
| [SDK API Design](manual/SDK_API_DESIGN.md) |跨语言 API 约定和对齐 |
| [Capability Verification](manual/CAPABILITY_VERIFICATION.md) |每个宣传功能的第一原则证据分类账、SDK 运行时门、证据差距闭合和性能政策 |
| [Scoped Capability Architecture](manual/SCOPED_CAPABILITY_ARCHITECTURE.md) | A3S Use所有权、类型化范围、不可变生成、可逆效应、迁移门和验证不变量 |
| [Performance Qualification](manual/PERFORMANCE_QUALIFICATION.md) |发布配置文件工作负载、包含规则、p50/p95/max 结果、资源上限、密封集成、运行链接和工件摘要 |
| [Harness Model-Call Evidence](manual/HARNESS_EVIDENCE.md) |功能/输入/使用快照、重复上下文诊断、事件排序、编辑边界、验证和重放 |
| [Evaluation Substrate](manual/EVALUATION_SUBSTRATE.md) |提供者中立的执行事实、有界证据、隔离的辅助运行、重新启动安全调度、持久结果 CAS 和所有权边界 |
| [Go SDK](sdk/go/README.md) |桥安装、会话、事件流、直接工具、错误和版本兼容性 |
| [Code Intelligence Design](manual/CODE_INTELLIGENCE_DESIGN.md) |语言运行时、能力边界、生命周期和验证 |
| [Workspace Retrieval Baseline](manual/WORKSPACE_RETRIEVAL_BASELINE.md) |架构、质量预算、生命周期和对抗性信任边界 |
| [Workspace Retrieval Qualification](manual/WORKSPACE_RETRIEVAL_QA.md) |发布测试、独立预言机、性能证据和 DeepSeek E2E 范围 |
| [Workspace Search Real-Model Qualification](manual/WORKSPACE_SEARCH_REAL_LLM.md) |一种用于自主搜索模式选择和透明本机 zvec 加速的有界 ACL 模型门 |
| [Workspace Search Production Qualification](manual/WORKSPACE_SEARCH_PRODUCTION.md) |确定性的本机/便携式测试加上真正的清单支持的规模、并发性、重建、清理和重新启动门 |
| [Workspace Retrieval DeepSeek Evaluation](manual/WORKSPACE_RETRIEVAL_DEEPSEEK_EVAL.md) |配对任务/重新排序消融、内置块矩阵、跨 SDK 真实模型奇偶校验、自定义负控制、非文本边界、指标和批处理跟进 |
| [Workspace Retrieval Chunking](manual/WORKSPACE_RETRIEVAL_CHUNKING.md) |内置/自定义策略、验证、异步生命周期、非文本边界和重新排序计划 |
| [Workspace Retrieval Operations](manual/WORKSPACE_RETRIEVAL_OPERATIONS.md) |生产 SLO、遥测、状态响应、生成门和仅配置回滚 |
| [Workspace Retrieval Backends](manual/WORKSPACE_RETRIEVAL_BACKENDS.md) | zvec-rust 词法索引、内存语义向量、资源边界、打包和回滚 |
| [Terminal-Bench Evaluation](manual/TERMINAL_BENCH.md) | Harbor 适配器、精确的任务交付、本地 Codex 评估以及符合排行榜的证据 |
| [Agent Directory Tools](manual/AGENT_DIR_TOOLS_DESIGN.md) |文件系统优先工具和代理定义|
| [Agent Release Contract](manual/AGENT_RELEASE_CONTRACT.md) |准入模式、身份、兼容性和安全边界 |
| [Changelog](CHANGELOG.md) |发布历史记录和迁移相关的更改 |

## 开发

从 A3S Code存储库目录运行检查：

```bash
python3 scripts/check_scoped_capability_architecture.py
python3 scripts/check_capability_verification.py
cargo fmt --all -- --check
cargo test -p a3s-code-core
cargo test -p a3s-code-core --all-features
cargo clippy -p a3s-code-core --all-targets --all-features -- -D warnings
node scripts/sdk_api_alignment_check.mjs
cargo test -p a3s-code-go-bridge
go -C sdk/go test ./...
cargo run --release -p a3s-code-core --example workspace_retrieval_benchmark
```

功能检查器将所有 27 个广告产品区域保持连接到
证据分类账。专用 CI 作业构建并加载 Node.js 和 Python 本机
运行其宿主语言契约之前的模块；成功的 Rust
单独的`cargo check`不算作SDK运行时证据。

检索基准发出 schema-v5 JSON。它保持锁定的 25,000 x 384
精确向量门与四文件、512 块的本机词汇/混合分开
配置文件，并且当 p95 预算、批处理、重新排序或清理门被设置时失败
超过了。请参阅
[qualification report](manual/WORKSPACE_RETRIEVAL_QA.md)供参考
配置文件、包含规则和测量结果。

目标[Performance Qualification](.github/workflows/performance.yml)
工作流程运行发布模式收敛、检索、流程/状态图、代码
智能、上下文/内存、持久语义刷新/SQLite 恢复，以及
当其关键路径发生变化时并按每周计划进行持久性配置文件。
它保留机器可读的 JSON 工件。的
[qualification record](manual/PERFORMANCE_QUALIFICATION.md)
捕获工作负载和包含规则、观察到的百分位数、资源结果、
运行链接和工件摘要。普通 CI 门确定性工作
放大和资源上限；远程模型和公共搜索引擎
延迟是单独报告的，而不是被视为稳定的核心速度
测量。

真实提供商和公共搜索引擎测试将被忽略，除非它们的外部测试
先决条件已配置。所需的密封 CI 单独驱动器固定
MinIO、工作流管理的 Chrome/CDP 和本地 OpenTelemetry Collector
生产集成边界。

通过本地 Codex 登录运行 context-tool real-LLM 套件：

```bash
A3S_CONTEXT_TOOLS_USE_CODEX_LOGIN=1 scripts/context_tools_real_llm.sh
```

或者，将 `A3S_CONFIG_FILE` 指向 ACL 提供程序配置并
在没有 `A3S_CONTEXT_TOOLS_USE_CODEX_LOGIN` 的情况下运行相同的脚本。

针对 ACL 运行串行、消耗配额的 DeepSeek 对抗性 E2E 套件
`default_model` 使用 `deepseek` 提供程序的配置：

```bash
A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
  cargo test -p a3s-code-core --test test_deepseek_adversarial_e2e -- \
  --ignored --test-threads=1 --nocapture
```

该套件使用一次性工作区和内存存储。事实证明
模型驱动的提示注入遏制、绝对路径工作空间隔离、
秘密编辑，并在执行之前取消已启动的命令
取消后的副作用。它从不记录提供商凭据。

## 许可证

[MIT](LICENSE)
