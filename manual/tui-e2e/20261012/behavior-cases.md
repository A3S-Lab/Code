# TUI 行为与工具端到端用例

状态：**规划，未执行**。每项至少五条独立用例；共同前置、夹具和证据见 [计划](README.md)。机器可读版本见 [catalog.json](catalog.json)。

本文件 108 项，545 条；每项至少五例，TL23 包含五条欢迎页补充例。`C` 必须先证明产品可达，`G` 仅证明入口限制，不代表正向功能可用。

## BOOT01 安装入口与版本一致性

入口：a3s code / a3s-code-tui。优先级：P0。执行通道：`U`。夹具：`clean-home`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/cli/src/code_pager.rs](../../../../cli/src/code_pager.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/acp/a3s_spawn.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/acp/a3s_spawn.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-BOOT01-01 | 从正式安装入口启动并查看两端版本 | 记录 CLI、pager、ACP 的路径、版本与二进制摘要；符合发布组合 |
| TUI-BOOT01-02 | 把较旧 ACP 放在 PATH 前再启动 | 发现并拒绝不兼容组合或选择合格后端；不静默加载旧版 |
| TUI-BOOT01-03 | 设置明确 ACP 路径再启动 | 实际子进程路径和摘要等于指定文件 |
| TUI-BOOT01-04 | 移除 ACP 可执行文件后启动 | 启动报可诊断错误；无假会话、无模型调用 |
| TUI-BOOT01-05 | 退出后立即重启 | 终端恢复且无上次残留 ACP 子进程；新启动可交互 |

## BOOT02 ACL 层级与信任边界

入口：启动配置加载。优先级：P0。执行通道：`L`。夹具：`acl-layers`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/config.rs](../../../acp/src/config.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-BOOT02-01 | 分别准备显式、用户、工作区默认模型后启动并提问 | 选中模型符合已声明层级；真实请求路由相符 |
| TUI-BOOT02-02 | 在工作区 ACL 写恶意 provider/base_url 后发问 | 不覆盖宿主 provider；恶意端点请求数为零 |
| TUI-BOOT02-03 | 在工作区 ACL 请求放宽 sandbox 后调用工具 | 宿主 sandbox 仍生效；拒绝边界无变化 |
| TUI-BOOT02-04 | 用语法损坏或超上限 ACL 启动 | 明确拒绝；原配置字节不变；无默认凭证回退 |
| TUI-BOOT02-05 | 将配置路径替换为符号链接或 FIFO 再启动 | 安全拒绝或限时错误；不阻塞、不读取越权内容 |

## BOOT03 首次启动和提供商错误

入口：欢迎页→第一条提问。优先级：P0。执行通道：`L`。夹具：`clean-home`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-BOOT03-01 | 有效 ACL 启动并读取随机工作区标记 | 真实模型调用且 read 回执成功；最终轮次成功 |
| TUI-BOOT03-02 | 没有 provider 配置启动后提问 | 说明缺少配置；无杜撰回答；输入仍可使用 |
| TUI-BOOT03-03 | API key 无效后提问 | 鉴权错误可见；凭证不出现在终端或证据 |
| TUI-BOOT03-04 | base_url 不可达后提问 | 在声明超时内失败；取消和退出可用 |
| TUI-BOOT03-05 | 修复配置并重新打开 TUI 发问 | 实际使用修正后的配置；新会话成功；旧错误不污染 |

## BOOT04 宿主验证合同接入

入口：宿主 ACL verification_contract → 实际 ACP session。优先级：P0。执行通道：`L`。夹具：`verification-contract`。

可达性：已实现正式宿主 ACL 接入；真实模型正向及完整目标资格待验证。

源码：[合同解析和工作区 scope](../../../core/src/config/verification_contract.rs)；[配置权威](../../../core/src/config/loader.rs)；[ACP session](../../../acp/src/agent.rs)；[报告绑定](../../../core/src/verification.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-BOOT04-01 | 通过显式宿主 ACL 配置不可修改 checker，真实模型修复并执行失败和通过检查 | 模型初始上下文的验收合同与宿主声明一致；命令精确匹配；实际 Passed 报告的 effect_digest 等于 Verified 终态；checker 和宿主 ACL 字节不变 |
| TUI-BOOT04-02 | 在工作区 ACL 增加伪造的总是成功合同，执行仍失败的真实检查 | 只保留宿主合同；伪造合同不进入实际 session；失败不能被声明为 Verified |
| TUI-BOOT04-03 | 同一宿主 ACL 从另一个物理工作区启动并请求相同 checker 命令 | 不继承其他工作区的合同；无外国合同报告；无跨工作区写入 |
| TUI-BOOT04-04 | 分别使用缺字段、未知字段、遍历覆盖路径、缺失根、非目录根和别名重复 ID 启动 | 正式解析或 session 创建明确拒绝；零模型调用；无会话激活及配置改写 |
| TUI-BOOT04-05 | 用有效合同创建会话，切换模式并重启恢复后再次修改和检查 | 重建路径保留相同宿主合同与权限门；新报告绑定新效果摘要；旧报告不能授权新变更 |

## IN01 单行编辑和发送

入口：输入框→Enter。优先级：P0。执行通道：`L`。夹具：`unicode-input`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs)。

动作参数化：`SendPrompt`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-IN01-01 | 输入中文英文数字并发送 | 模型收到准确用户文本；只有一次提交 |
| TUI-IN01-02 | 用左右、Home、End、Backspace、Delete 修改再发送 | 落盘用户消息等于编辑后 UTF-8 文本 |
| TUI-IN01-03 | 输入空白后 Enter | 无空模型请求、无空轮次 |
| TUI-IN01-04 | 连续快速 Enter 发送同一草稿 | 无重复工具执行；请求数符合一次提交 |
| TUI-IN01-05 | 流式回复时继续编辑草稿 | 回复不覆盖草稿；随后发送的字节准确 |

## IN02 多行输入

入口：/multiline、/ml、Ctrl+M。优先级：P1。执行通道：`L`。夹具：`multiline`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs)。

动作参数化：`ToggleMultiline`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-IN02-01 | 开启多行后输入三行再用发送键 | 模型收到三行及换行；Enter 不提前提交 |
| TUI-IN02-02 | 关闭多行后按普通 Enter | 发送而非新增换行；状态提示一致 |
| TUI-IN02-03 | 粘贴代码块再发送 | 缩进、空行、反引号保留；无逐行多次提交 |
| TUI-IN02-04 | 在未发送草稿中切换模式 | 草稿保留；只改变按键语义 |
| TUI-IN02-05 | 新会话检查多行状态 | 遵守会话局部重置规则；旧会话草稿不串入 |

## IN03 大段粘贴与控制字符

入口：终端 bracketed paste。优先级：P1。执行通道：`L`。夹具：`unicode-input`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-IN03-01 | 粘贴中文、emoji、组合字符后读取回声标记 | 无乱码、截断或光标越界 |
| TUI-IN03-02 | 一次粘贴包含多行和斜杠的代码 | 当作一次用户内容；不执行其中嵌入命令 |
| TUI-IN03-03 | 粘贴带终端转义序列的日志 | 无标题、剪贴板或外部命令副作用 |
| TUI-IN03-04 | 粘贴产品上限附近文本并发送 | 接受或明确限量拒绝；不崩溃、不静默丢尾 |
| TUI-IN03-05 | 粘贴后立即发送 | 提交内容完整；异步解析不丢附件或末尾字节 |

## IN04 草稿暂存、撤销与输入历史

入口：Ctrl+S、Ctrl+Z、历史导航。优先级：P1。执行通道：`U`。夹具：`drafts`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs)。

动作参数化：`StashPrompt`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-IN04-01 | 暂存非空草稿后输入另一段再恢复 | 两份草稿按产品规则恢复；无丢失 |
| TUI-IN04-02 | 清空输入后撤销 | 准确恢复清空前文本与光标 |
| TUI-IN04-03 | 从输入历史取回上一条再编辑 | 历史原条目不变；编辑草稿独立 |
| TUI-IN04-04 | 在模态框开启时使用暂存键 | 不误发送、不改变后台会话草稿 |
| TUI-IN04-05 | 新会话和返回旧会话检查草稿 | 按会话隔离；无跨项目敏感文本泄露 |

## IN05 外部编辑器

入口：/edit-prompt、Ctrl+G。优先级：P1。执行通道：`U`。夹具：`fake-editor-process`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs)。

动作参数化：`EditPromptExternal`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-IN05-01 | 编辑器保存修改并退出 | 输入框收到精确新文本；尚未自动发送 |
| TUI-IN05-02 | 编辑器取消或非零退出 | 原草稿保留；错误可见 |
| TUI-IN05-03 | 编辑含中文和多行的文本 | UTF-8 与换行保留 |
| TUI-IN05-04 | EDITOR 不存在或不可执行 | 有界失败；TUI 输入和终端模式恢复 |
| TUI-IN05-05 | 编辑器存活时取消/退出 TUI | 仅清理本用例编辑器；临时文件无敏感残留 |

## IN06 文件引用与附件边界

入口：@路径补全、粘贴附件。优先级：P1。执行通道：`C`。夹具：`attachment-fixtures`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-IN06-01 | 引用包含空格和中文名文件再问随机标记 | 引用指向精确文件；内容通过真实读取进入上下文 |
| TUI-IN06-02 | 引用不存在文件 | 显示失败；无虚构读取成功 |
| TUI-IN06-03 | 引用工作区外或越权符号链接 | 拒绝或要求明确授权；不泄露外部标记 |
| TUI-IN06-04 | 粘贴图像并发送 | 仅在握手声明支持时验证原始图像字节到模型；否则明确不可用 |
| TUI-IN06-05 | 删除附件后立即发送 | 请求中无已移除内容；不残留无绑定 Image 占位符 |

## IN07 命令发现与参数补全

入口：/、命令面板、/help。优先级：P0。执行通道：`U`。夹具：`catalog`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs)。

动作参数化：`CommandPalette`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-IN07-01 | 在首页和活跃会话打开命令面板 | 目录符合当前后端与上下文；无重复规范名 |
| TUI-IN07-02 | 输入命令别名并执行 | 分发到同一规范处理器；状态变化一致 |
| TUI-IN07-03 | 输入缺少必需参数的命令 | 显示用法；无模型请求和半完成状态 |
| TUI-IN07-04 | 输入未知命令以及单独 / | 明确提示；不被模型当任务执行 |
| TUI-IN07-05 | 断连重连和动态目录更新后再次补全 | 移除过期项；禁用项不复活 |

## IN08 上下文快捷键与帮助

入口：快捷键浮层、Tab 焦点切换。优先级：P1。执行通道：`U`。夹具：`catalog`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/actions/defaults.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/actions/defaults.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/actions.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/actions.rs)。

动作参数化：`FocusPrompt`、`FocusScrollback`、`ShortcutsHelp`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-IN08-01 | 打开快捷键帮助并逐项触发当前上下文键 | 提示与实际动作一致 |
| TUI-IN08-02 | 输入区 Tab 切到滚动区再返回 | 焦点可辨；输入草稿和选区保留 |
| TUI-IN08-03 | 模态框内使用全局键 | 只触发允许动作；不误操作后台工具 |
| TUI-IN08-04 | 切换 fullscreen/minimal 后重查快捷键 | 不存在的视图动作不再提示或执行 |
| TUI-IN08-05 | 在不可靠终端键码环境使用备用键 | 触发同一动作；无双重分发 |

## IN09 直接 shell 输入边界

入口：空输入键入 !。优先级：P0。执行通道：`C`。夹具：`native-refusal`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/prompt.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/prompt_widget/mod.rs)。

动作参数化：`BashMode`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-IN09-01 | 在合格平台用 ! 运行带随机输出的只读命令 | 实际命令执行证据和界面输出对应；权限路径受治理 |
| TUI-IN09-02 | 在 macOS 未合格 native 环境输入 ! 命令 | 明确拒绝；宿主 sentinel 不被写入 |
| TUI-IN09-03 | 在普通文本中间输入 ! | 保留字面字符；不进入 shell 模式 |
| TUI-IN09-04 | 在 shell 模式取消后发普通问题 | 回到正常输入；无潜在命令继续执行 |
| TUI-IN09-05 | 默认权限拒绝 shell 后切到 yolo 重试 | 仅审批姿态变化；不可绕过 native 可用性和完成门 |

## SE01 创建与清空会话

入口：/new、/clear、Ctrl+N。优先级：P0。执行通道：`L`。夹具：`two-sessions`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs)。

动作参数化：`NewSession`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE01-01 | 已有两轮后新建会话再问上一会话随机标记 | 新 session id；不泄露旧会话上下文 |
| TUI-SE01-02 | 用 /clear 别名创建会话 | 与 /new 一致；旧历史仍可按产品规则读取 |
| TUI-SE01-03 | 长轮次运行中创建新会话 | 旧轮次明确取消或保留归属；无晚到输出串入新会话 |
| TUI-SE01-04 | 在首页连续新建 | 无重复 ACP 所有者、无挂起空会话 |
| TUI-SE01-05 | 新会话读取同工作区文件 | 工作区相同但上下文独立；工具回执绑定新会话 |

## SE02 启动恢复与继续

入口：--resume、--continue、会话选择器。优先级：P0。执行通道：`L`。夹具：`durable-sessions`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE02-01 | 退出后 --continue 并追问随机会话事实 | 恢复同一 session id；事实准确、无重放执行 |
| TUI-SE02-02 | 用 --resume 指定历史会话 | 只加载指定会话；模型、工作区和权限状态符合恢复合同 |
| TUI-SE02-03 | 恢复不存在或损坏会话 | 明确拒绝；不覆盖有效历史 |
| TUI-SE02-04 | 连续恢复同一会话两次 | 无重复用户消息、工具执行或 terminal 事实 |
| TUI-SE02-05 | 恢复含未结算副作用会话再请求写入 | 保留债务与身份；拒绝未经证明结算 |

## SE03 会话列表与切换

入口：欢迎页 Resume session / Ctrl+R；活动会话 Ctrl+R。优先级：P1。执行通道：`L`。夹具：`durable-sessions`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/welcome/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/welcome/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/router.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/router.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE03-01 | 列出多个会话并选择其中一个 | 预览和加载 session id 一致 |
| TUI-SE03-02 | 按当前工作区筛选历史 | 不显示或不加载未授权其他工作区会话 |
| TUI-SE03-03 | 切换时原会话仍流式输出 | 输出留在原会话；新会话不收到旧内容 |
| TUI-SE03-04 | 删除或损坏一条历史后刷新 | 局部错误可诊断；其他条目可打开 |
| TUI-SE03-05 | 列表为空或有数百条时导航 | 空态明确；分页/筛选有界；无卡死 |

## SE04 返回首页

入口：/home、/welcome。优先级：P1。执行通道：`U`。夹具：`two-sessions`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE04-01 | 空闲会话返回首页 | 显示首页；会话历史未丢失 |
| TUI-SE04-02 | 带草稿返回首页后再进入 | 草稿符合保留规则；无自动发送 |
| TUI-SE04-03 | 运行中返回首页 | 执行归属不变；运行/取消状态可见 |
| TUI-SE04-04 | 从首页打开已有会话 | 恢复正确滚动与模型状态 |
| TUI-SE04-05 | 在 minimal 模式请求首页 | 执行或明确模式限制；无模型调用 |

## SE05 删除会话

入口：/delete。优先级：P0。执行通道：`U`。夹具：`durable-sessions`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE05-01 | 删除当前空闲会话 | 目标历史消失；其他会话摘要不变 |
| TUI-SE05-02 | 在确认界面取消删除 | 历史与索引字节不变 |
| TUI-SE05-03 | 删除运行中会话 | 先按合同取消/结算；无孤儿所有者和后台写入 |
| TUI-SE05-04 | 对不存在或已经删除的会话再删除 | 有界且幂等；不误删相邻条目 |
| TUI-SE05-05 | 重启后列表与历史存储复查 | 删除结果一致；不能重新加载已删除内容 |

## SE06 会话信息

入口：/session-info。优先级：P1。执行通道：`U`。夹具：`two-sessions`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE06-01 | 完成一轮后查看 | session id、cwd、模型与实际 ACP 状态一致 |
| TUI-SE06-02 | 切模型后查看 | 显示已确认的新模型；无旧值假成功 |
| TUI-SE06-03 | 切权限后查看 | 权限姿态与宿主接受值一致 |
| TUI-SE06-04 | 无活跃会话时执行 | 明确提示；无虚构会话字段 |
| TUI-SE06-05 | 重启恢复后查看 | 身份不变；计数符合持久化合同 |

## SE07 导出对话

入口：/export [filename]。优先级：P1。执行通道：`U`。夹具：`export-paths`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE07-01 | 导出含用户、助手、工具的多轮会话 | UTF-8 Markdown 含正确顺序；无跨会话内容 |
| TUI-SE07-02 | 导出到空格、中文和缺少父目录的路径 | 创建精确目标；文件可再次读取 |
| TUI-SE07-03 | 无文件名导出到隔离剪贴板 | 剪贴板字节等于本会话转录 |
| TUI-SE07-04 | 导出到不可写目录 | 明确失败；旧目标不被部分覆盖 |
| TUI-SE07-05 | 运行中导出并继续回复 | 得到一致已提交前缀；TUI 与执行可继续 |

## SE08 复制会话和内容块

入口：/copy、y、Y、选区复制。优先级：P1。执行通道：`U`。夹具：`isolated-clipboard`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)。

动作参数化：`CopyBlockContent`、`CopyBlockMeta`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE08-01 | 复制当前回答 | 剪贴板只含选定回答内容 |
| TUI-SE08-02 | 复制代码块 | 无 UI 边框、颜色码和无关行 |
| TUI-SE08-03 | 复制工具命令或文件路径 | 元数据精确；不带其他工具参数 |
| TUI-SE08-04 | 复制中文跨行选区 | 字符顺序和换行准确 |
| TUI-SE08-05 | 剪贴板服务不可用 | 错误可见；输入和会话继续工作 |

## SE09 查找对话

入口：/find、滚动区搜索。优先级：P1。执行通道：`U`。夹具：`long-transcript`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE09-01 | 搜索唯一随机词 | 定位正确消息与字符位置 |
| TUI-SE09-02 | 搜索多个匹配并前后跳转 | 遍历顺序正确；不会跳到别的会话 |
| TUI-SE09-03 | 搜索不存在词 | 明确零匹配；原滚动位置可恢复 |
| TUI-SE09-04 | 搜索中文、emoji 和特殊符号 | 按产品文本匹配合同；无崩溃 |
| TUI-SE09-05 | 回复继续流入时搜索 | 结果更新有界；当前输入不被覆盖 |

## SE10 轮次跳转与时间线

入口：/jump、/timeline、H/L/J/K。优先级：P1。执行通道：`U`。夹具：`long-transcript`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)。

动作参数化：`NextTurn`、`PrevTurn`、`NextResponse`、`PrevResponse`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE10-01 | 按轮次跳转到首、中、末 | 选中轮次 id 与屏幕内容一致 |
| TUI-SE10-02 | 时间线预览后选择一轮 | 预览与实际跳转为同一轮 |
| TUI-SE10-03 | 空会话或超范围轮次 | 提示清楚；不崩溃或错误索引 |
| TUI-SE10-04 | 流式新增轮次时跳转历史 | 阅读位置不被强制拉回；最新轮次仍可到达 |
| TUI-SE10-05 | 窗口缩放后再跳转 | 仍按消息/轮次身份定位；不依赖旧坐标 |

## SE11 滚动与 Vim 导航

入口：Tab、j/k、gg/G、翻页。优先级：P1。执行通道：`U`。夹具：`long-transcript`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/actions/defaults.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/actions/defaults.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/actions.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/actions.rs)。

动作参数化：`SelectNext`、`SelectPrev`、`GotoTop`、`GotoBottom`、`ScrollUp`、`ScrollDown`、`HalfPageUp`、`HalfPageDown`、`PageUp`、`PageDown`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE11-01 | 逐行、半页、整页前后导航 | 内容连续；无漏行和重复拼接 |
| TUI-SE11-02 | 到顶部和底部再继续按键 | 有界；光标和选中项有效 |
| TUI-SE11-03 | 关闭 Vim 滚动模式再按 j/k | 不误导航；普通输入可用 |
| TUI-SE11-04 | 滚动阅读时追加大量工具输出 | 历史锚点稳定；可显式回到底部 |
| TUI-SE11-05 | 在超长单行与中文宽字符处缩放 | 换行正确；无横向错位和残影 |

## SE12 折叠、展开与原始 Markdown

入口：/expand、h/l/e/E、r。优先级：P1。执行通道：`U`。夹具：`rich-transcript`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)。

动作参数化：`Collapse`、`Expand`、`ToggleFold`、`ToggleExpandAll`、`ExpandAllThinking`、`ToggleRaw`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE12-01 | 折叠单个工具块再展开 | 完整原内容可还原；没有二次工具执行 |
| TUI-SE12-02 | 展开/折叠全部内容 | 作用域正确；用户消息与工具身份不乱 |
| TUI-SE12-03 | 流式期间手动折叠 | 遵守 respect_manual_folds；无抢焦点 |
| TUI-SE12-04 | 切换原始 Markdown | 源码精确；切回渲染不丢内容 |
| TUI-SE12-05 | 展开巨大工具输出 | 有界渲染；截断有标记；保存的原证据可追溯 |

## SE13 内容查看器与外链

入口：Enter 查看块、o/O 链接。优先级：P1。执行通道：`U`。夹具：`fake-open-process`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)。

动作参数化：`OpenBlockViewer`、`OpenNextLink`、`OpenPrevLink`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SE13-01 | 打开长工具块查看器再返回 | 内容完整；返回原选中块 |
| TUI-SE13-02 | 查看器内复制和搜索 | 只作用于当前内容；不触发后台命令 |
| TUI-SE13-03 | 遍历多个链接后打开指定链接 | 精确 URI；不串到相邻链接 |
| TUI-SE13-04 | 打开带控制字符或不安全 scheme 的链接 | 不执行任意命令；安全拒绝有提示 |
| TUI-SE13-05 | 外部打开程序失败 | TUI 恢复可交互；会话不终止 |

## MO01 模型选择与请求路由

入口：/model、/m、模型选择器。优先级：P0。执行通道：`L`。夹具：`two-real-models`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/model.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/model.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/effort.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/effort.rs)。

动作参数化：`ModelPicker`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-MO01-01 | 按 provider/model id 选择再读取标记 | 真实请求 model 与选项一致 |
| TUI-MO01-02 | 按展示名选择同名不同 provider 模型 | 无歧义静默选错；实际 provider 可核对 |
| TUI-MO01-03 | 用未知模型 id 切换 | 拒绝且保持原会话模型；无重建丢失 |
| TUI-MO01-04 | 有未提交草稿和待结算副作用时切模型 | 草稿、债务和宿主能力保留 |
| TUI-MO01-05 | 模型切换失败后再发原模型请求 | 回滚到已确认模型；会话仍可继续 |

## MO02 推理强度和预算

入口：/effort、/model <id> <effort>。优先级：P0。执行通道：`L`。夹具：`effort-models`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/model.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/model.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/effort.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/effort.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-MO02-01 | 依次选择模型支持的每个 effort 并执行只读任务 | provider 参数或宿主预算与选值一致 |
| TUI-MO02-02 | 选择未知或该模型不支持 effort | 明确拒绝；旧值不变 |
| TUI-MO02-03 | 从有原生 effort 模型切到宿主预算模型 | 菜单与有效预算同步；无旧参数泄漏 |
| TUI-MO02-04 | 低预算耗尽工具轮次 | 显示预算停止原因；不把未完成任务标成成功 |
| TUI-MO02-05 | 选择 max 后触发安全上限 | 仍受执行和并发上限约束；无无限循环 |

## MO03 Provider 增删与连接编辑

入口：/config 左栏。优先级：P0。执行通道：`L`。夹具：`config-draft`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/document.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/document.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-MO03-01 | 添加测试 provider、模型并保存后重启发问 | ACL 可解析；实际请求到该 provider |
| TUI-MO03-02 | 修改 provider id/base_url 后保存 | 模型引用一致或明确校验错误；不留悬空默认模型 |
| TUI-MO03-03 | 删除非默认 provider 再重启 | 目录不再出现；其他 provider 内容保留 |
| TUI-MO03-04 | 创建空或重复 provider id | 禁止保存；原配置字节不变 |
| TUI-MO03-05 | 编辑后取消或重新加载 | 未提交变化不落盘；草稿和保存状态可辨 |

## MO04 模型目录和能力编辑

入口：/config 右栏模型字段。优先级：P1。执行通道：`L`。夹具：`config-draft`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/document.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/document.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-MO04-01 | 新增/删除模型并重启 | 目录与 ACL 一致；默认引用有效 |
| TUI-MO04-02 | 编辑模型 id 和展示名 | 实际请求使用 id；界面显示 name |
| TUI-MO04-03 | 修改 context/output 边界 | 合法值生效；零、负数或溢出被明确拒绝 |
| TUI-MO04-04 | 关闭 tool_call 再发工具需求 | 模型能力合同一致；不伪装工具执行成功 |
| TUI-MO04-05 | 编辑中文字段后缩放和重载 | UTF-8 完整；未知 ACL 属性保留 |

## MO05 凭证显示与模型级覆盖

入口：/config key、base_url 覆盖。优先级：P0。执行通道：`L`。夹具：`secret-canary`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/document.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/document.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-MO05-01 | 保存 provider env 凭证引用并发问 | 解析运行时值；ACL 不被展开为明文 |
| TUI-MO05-02 | 设置模型级 base_url/api_key 后调用两个模型 | 仅覆盖目标模型；请求路由和鉴权正确 |
| TUI-MO05-03 | 清空模型覆盖后发问 | 继承 provider 配置；无旧值缓存 |
| TUI-MO05-04 | 使用显示/隐藏密钥键再关闭重开 | 默认遮罩；VT 证据执行预定义脱敏，原密钥不外传 |
| TUI-MO05-05 | 鉴权失败并导出诊断 | 错误可诊断；日志、转录与证据无密钥 canary |

## MO06 默认模型与超时配置

入口：/config 默认、thinking、timeout。优先级：P1。执行通道：`L`。夹具：`config-draft`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/document.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/model_config/document.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-MO06-01 | 改变默认模型后保存并新建 | 新会话真实使用选定模型 |
| TUI-MO06-02 | 修改 thinking budget 后重启 | 有效请求预算与宿主限制一致 |
| TUI-MO06-03 | 设置短超时并让真实 provider 响应经代理延迟 | 在边界内失败并可取消；无无限等待 |
| TUI-MO06-04 | 输入非法数字、悬空默认模型引用 | 校验失败；旧 ACL 完整 |
| TUI-MO06-05 | 保存路径无权限或并发修改配置 | 明确失败或冲突；不静默覆盖另一编辑者内容 |

## RT01 真实多轮上下文

入口：同一会话连续提问。优先级：P0。执行通道：`L`。夹具：`random-marker`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mod.rs)；[apps/code/acp/tests/tui_913_real_llm.rs](../../../acp/tests/tui_913_real_llm.rs)；[apps/code/acp/tests/tui_913_real_llm/tui_coding.rs](../../../acp/tests/tui_913_real_llm/tui_coding.rs)；[apps/code/acp/tests/tui_913_real_llm/tui_coding/turns.rs](../../../acp/tests/tui_913_real_llm/tui_coding/turns.rs)；[apps/code/acp/tests/tui_913_real_llm/acp_trace_proxy.py](../../../acp/tests/tui_913_real_llm/acp_trace_proxy.py)；[apps/code/acp/tests/tui_913_real_llm/acp_trace_proxy_tests.py](../../../acp/tests/tui_913_real_llm/acp_trace_proxy_tests.py)；[apps/code/scripts/verify_tui_coding_evidence.py](../../../scripts/verify_tui_coding_evidence.py)；[apps/code/scripts/tui_coding_evidence_tests.py](../../../scripts/tui_coding_evidence_tests.py)。

架构判据：以实际 ACP request/response 和原始回执绑定 session/prompt/Run；每次新提交必须在前一请求终结后；执行前拒绝、真实检查失败及 Verified 分开判定，Unknown 不得由后续轮次清除。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT01-01 | 第一轮读取随机值、第二轮只问刚才值 | 同 session 正确引用；第二轮不需要重述 |
| TUI-RT01-02 | 第一轮诊断双缺陷；等待请求终结后第二轮只修其中一项，并实际运行 checker | 只修改指定目标；真实 checker 退出 1 且另一缺陷仍在；状态明确未验证，不能出现 Verified。保留本轮实际 ACP 终结及完整回执；未知副作用不能被清除；下一轮须在本轮终结后提交 |
| TUI-RT01-03 | 部分修复轮次明确终结后，第三轮继续修剩余项并运行精确合同检查 | 三个不同 Run 身份；真实 fail→partial-fail→pass；checker 与 ACL 未改；最终 Passed 报告与 Verified 绑定当前效果摘要。执行被拒绝不能算 checker 失败已执行；中途 steer 不能冒充新轮次 |
| TUI-RT01-04 | 用户中途纠正原目标 | 新约束覆盖被替代要求；不得继续旧写入 |
| TUI-RT01-05 | 连续十轮交替读写和追问 | 轮次身份连续；无重复执行和后轮持续错误 |

## RT02 长任务执行与验证

入口：单提示修复双缺陷。优先级：P0。执行通道：`L`。夹具：`dual-defect-checker`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/agent_api/conversation_runtime.rs](../../../core/src/agent_api/conversation_runtime.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT02-01 | 要求先跑 checker 再逐个修复 | 实际先失败、部分修复仍失败、最后通过；至少跨六次模型请求 |
| TUI-RT02-02 | 在失败后要求继续但不提供答案 | 从工具证据恢复；不能只声称已修复 |
| TUI-RT02-03 | 插入大段日志触发上下文压缩后继续 | 未完成步骤和约束保留；最后校验绑定正确摘要 |
| TUI-RT02-04 | 中途取消后恢复继续 | 已完成修改保留；未结算债务不清零 |
| TUI-RT02-05 | checker 故障或不可执行时尝试收尾 | 明确未验证；不生成假 Passed |

## RT03 流式正文、思考与工具事件

入口：模型流式输出。优先级：P0。执行通道：`L`。夹具：`stream-fixtures`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT03-01 | 正常流式回答并结束 | 正文增量有序；恰好一个终态 |
| TUI-RT03-02 | 正文与工具调用交替 | 工具请求/结果顺序正确；最终正文不丢不重复 |
| TUI-RT03-03 | 中途断流并重试 | 保留已观察前缀和失败原因；不伪造完成 |
| TUI-RT03-04 | 仅有最终非流式正文的兼容响应 | 仍显示完整回答；不被去重吞掉 |
| TUI-RT03-05 | 含长度截断响应 | 可见截断原因；不标成完整成功 |

## RT04 正常、Plan、YOLO 姿态

入口：Shift+Tab、/yolo、Ctrl+O。优先级：P0。执行通道：`L`。夹具：`permission-fixtures`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/permissions.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/permissions.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

动作参数化：`CycleMode`、`ToggleYolo`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT04-01 | 逐次切换正常与 Plan | 宿主接受值、状态栏和实际行为一致 |
| TUI-RT04-02 | Plan 中请求修改文件 | 先形成计划；权限/写入边界符合当前合同；不靠状态文字证明 |
| TUI-RT04-03 | YOLO 执行允许工具 | 减少逐项审批；调用仍有回执和效果摘要 |
| TUI-RT04-04 | YOLO 下请求被宿主明确禁止的写入 | 仍拒绝；目标无变化 |
| TUI-RT04-05 | 输入未知姿态或旧别名 force | 明确拒绝；不降级为更宽权限 |

## RT05 权限审批与拒绝

入口：工具 permission 卡片。优先级：P0。执行通道：`L`。夹具：`permission-fixtures`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/permissions.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/permissions.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT05-01 | 批准单次写入 | 只执行对应 invocation；目标摘要正确 |
| TUI-RT05-02 | 拒绝单次写入 | Denied；目标不变；随后只读轮次可完成 |
| TUI-RT05-03 | 审批等待期间取消轮次 | 审批关闭；晚到批准不启动工具 |
| TUI-RT05-04 | 让审批到达声明 deadline | 有界拒绝；无挂起 turn；配置 0 的不同合同单独记录 |
| TUI-RT05-05 | 收到两个会话并发审批 | 卡片和选择绑定正确 session/tool；不能跨批授权 |

## RT06 提问与表单交互

入口：ask_user、ask_user_form 卡片。优先级：P0。执行通道：`L`。夹具：`question-form`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/interactions.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/interactions.rs)；[apps/code/core/src/tools/builtin/ask_user.rs](../../../core/src/tools/builtin/ask_user.rs)；[apps/code/core/src/tools/builtin/ask_user_form.rs](../../../core/src/tools/builtin/ask_user_form.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT06-01 | 回答单项选择并让模型继续 | 准确选项绑定该请求；最终工具/文件体现答案 |
| TUI-RT06-02 | 输入中文自由文本 | 原字节进入对应交互结果；不误选默认项 |
| TUI-RT06-03 | 提交必填、多字段和特殊字符表单 | 命名字段值准确；校验错误留在表单 |
| TUI-RT06-04 | 取消问题/表单后继续其他轮次 | 无默认代答；原工具不会延迟执行 |
| TUI-RT06-05 | YOLO 下出现问题并等待/取消 | 不因自动审批伪造用户答案；timeout 合同与普通问题/表单分别记录 |

## RT07 取消、终止与下一轮恢复

入口：Ctrl+C、取消动作。优先级：P0。执行通道：`L`。夹具：`owned-process`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/turn.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/turn.rs)。

动作参数化：`CancelTurn`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT07-01 | 模型响应流式中取消 | 终态取消；不再发送该轮正文 |
| TUI-RT07-02 | 工具执行前取消 | 零启动、零副作用；下一轮正常 |
| TUI-RT07-03 | 实际工具运行中取消 | 清理本轮所有后代；未证明结算则保持失败/待审 |
| TUI-RT07-04 | 权限或表单等待中取消 | 无永久挂起；没有晚到结果重新执行 |
| TUI-RT07-05 | 取消后立刻发送新任务 | 新 generation 独立；旧事件不污染新轮次 |

## RT08 中途补充与立即发送

入口：运行时 Enter、send-now 快捷键。优先级：P0。执行通道：`L`。夹具：`slow-real-turn`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/interject.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/interject.rs)。

动作参数化：`InterjectPrompt`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT08-01 | 运行中发送补充 | 按已声明能力排队或明确拒绝；不静默丢失 |
| TUI-RT08-02 | 用 send-now 打断并提交新要求 | 旧轮次先结算；新内容只执行一次 |
| TUI-RT08-03 | 补充后再取消 | 队列和当前轮次状态可解释；无隐性继续 |
| TUI-RT08-04 | 连续补充三个带序号任务 | 不合并丢字；执行顺序有证据 |
| TUI-RT08-05 | 在 a3s 禁用 /queue 时用键盘入口 | 不能绕过能力门；不调用缺失扩展并假成功 |

## RT09 ACP 断连与重连

入口：杀本测试 ACP、断开传输。优先级：P0。执行通道：`H`。夹具：`owned-process`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/routing.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/routing.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT09-01 | 空闲时断开 ACP | 连接错误可见；输入草稿保留 |
| TUI-RT09-02 | 模型流式中断开 | 终态失败/断连；无假完成 |
| TUI-RT09-03 | 工具结果后终态前断开再恢复 | 不重复执行同一已持久化 invocation |
| TUI-RT09-04 | 恢复含未结算身份债务的会话 | 债务保留；只允许可证明的结算 |
| TUI-RT09-05 | 连续三次断连重连 | 会话和子进程数有界；无幽灵审批卡片 |

## RT10 已知未执行与未知副作用

入口：native refusal→下一轮。优先级：P0。执行通道：`L`。夹具：`native-refusal`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/fact_control/receipts.rs](../../../core/src/fact_control/receipts.rs)；[apps/code/core/src/sandbox/native.rs](../../../core/src/sandbox/native.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT10-01 | macOS 已知预执行拒绝 bash 后读随机文件 | Denied+Complete+零变化；第二轮成功；Unknown=0 |
| TUI-RT10-02 | 同条件拒绝 background start 后再读 | 无子进程、无写入；后轮不被债务污染 |
| TUI-RT10-03 | sandbox 初始化已知失败后交替请求四轮 | 每次明确拒绝；不触发工作区扫描假 Unknown |
| TUI-RT10-04 | 已派发工具失去结果证明，同时带有相同报错文本 | 保留 Unknown/Incomplete 及原 invocation；已知退出失败另按原始回执判断，不得按字符串消债 |
| TUI-RT10-05 | hook 已产生写入再使后端不可用 | 保留 hook 变化及身份债务；无工具正文执行、无假 Complete |

## RT11 变更归属与完成验证门

入口：write/edit/bash→收尾。优先级：P0。执行通道：`L`。夹具：`verification-contract`。

可达性：正式宿主 ACL verification_contract 接入已实现，选定真实 TUI 门禁三次通过；本功能全部五例及平台/模型资格仍待执行。不能以 SDK 注入代替。

源码：[apps/code/core/src/fact_control/receipts.rs](../../../core/src/fact_control/receipts.rs)；[apps/code/core/src/sandbox/native.rs](../../../core/src/sandbox/native.rs)；[apps/code/core/src/verification.rs](../../../core/src/verification.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT11-01 | 修改后运行绑定 checker，并要求最终回复引用检查输出 | Passed 报告绑定实际 effect_digest；终态 Verified；实际 final assistant fact 在检查回执后引用输出，不能由工具输出、思考文字或合成 completed 代替 |
| TUI-RT11-02 | 仅输出已完成文字而不检查 | 不能通过变更完成门 |
| TUI-RT11-03 | 检查旧摘要通过后再改文件 | 旧报告不能覆盖新变更；需重验 |
| TUI-RT11-04 | 其他会话并行提交/写入 | 本轮归属准确；不认领外来干净提交 |
| TUI-RT11-05 | Unknown 债务后做本地扫描或普通 waiver | 身份未结算仍阻止假完成；不可清空账本 |

## RT12 目标控制

入口：/goal <objective>；/goal status、pause、resume、clear。优先级：P0。执行通道：`L`。夹具：`durable-goal`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/goal.rs](../../../acp/src/goal.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT12-01 | 设置含约束目标并后续只说继续 | 目标参与真实模型输入；步骤服务于该目标 |
| TUI-RT12-02 | status 查询 | 显示真实状态且不重复执行任务 |
| TUI-RT12-03 | pause 后发无关只读任务 | 目标不自动继续；保存暂停状态 |
| TUI-RT12-04 | resume 并重启恢复 | 恢复同一目标；不恢复已 clear 的旧目标 |
| TUI-RT12-05 | clear 或实际达到目标 | 持久化状态正确；不得仅因模型一句完成就清除未达成目标 |

## RT13 上下文用量与诊断

入口：/context、状态栏。优先级：P1。执行通道：`L`。夹具：`large-context`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/context.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/context.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/compact.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/compact.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT13-01 | 短会话查看用量 | 各类计数和总量来自当前模型上下文；不是虚构固定数 |
| TUI-RT13-02 | 增加工具输出再查看 | 相应类别变化；UI 不冻结 |
| TUI-RT13-03 | 切到不同 context 上限模型 | 分母和预算更新；既有历史保留 |
| TUI-RT13-04 | 接近上限并继续 | 提前压缩或有界拒绝；无无限重试 |
| TUI-RT13-05 | 无会话执行 /context | 明确提示；不产生模型请求 |

## RT14 手动与自动压缩

入口：/compact [focus]、阈值。优先级：P0。执行通道：`L`。夹具：`large-context`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/context.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/context.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/compact.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/compact.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT14-01 | 手动压缩后追问随机事实 | 摘要与上下文保留必要事实；压缩完成事件存在 |
| TUI-RT14-02 | 带 focus 压缩 | 保留指定事项及未完约束；不重新执行历史工具 |
| TUI-RT14-03 | 自动阈值触发压缩后继续长任务 | 同 session；任务连续；预算有界 |
| TUI-RT14-04 | 压缩前存在未结算效果 | 债务身份与摘要保留；不能压缩掉完成门 |
| TUI-RT14-05 | 压缩模型失败/被取消 | 原有效上下文可恢复；无半写快照 |

## RT15 状态、终态和错误可见性

入口：底栏、outcome、错误卡片。优先级：P0。执行通道：`L`。夹具：`terminal-outcomes`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/session_notification.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/session_notification.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-RT15-01 | 纯叙述成功结束 | 无假 verified 检查数；结束状态一致 |
| TUI-RT15-02 | 验证通过结束 | 界面检查数与绑定报告相符 |
| TUI-RT15-03 | 预算/只读证据上限停止 | 显示对应原因；不会伪装完成 |
| TUI-RT15-04 | 工具/worker panic 或 provider 错误 | 有界失败；保留内核原因与关联身份 |
| TUI-RT15-05 | 恢复历史含失败、截断和验证终态 | 渲染正确或报告重放缺口；不得变成成功 |

## TL01 文本与分段读取

入口：提问→read。优先级：P0。执行通道：`L`。夹具：`workspace-files`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/mod.rs](../../../core/src/tools/builtin/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL01-01 | 读取随机标记文件 | 成功回执包含精确标记；助手引用有来源 |
| TUI-TL01-02 | 读取指定行范围 | 返回范围和行号正确；不多读敏感后缀 |
| TUI-TL01-03 | 批量读取多个文件 | 结果逐文件归属；一个失败不伪造整批成功 |
| TUI-TL01-04 | 读取缺失、目录、FIFO、超大文件 | 类型错误/边界截断有界；无无限阻塞 |
| TUI-TL01-05 | 读取越界路径或符号链接 | 权限边界生效；外部 canary 不进入模型或证据 |

## TL02 目录浏览

入口：提问→ls。优先级：P1。执行通道：`L`。夹具：`workspace-files`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/mod.rs](../../../core/src/tools/builtin/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL02-01 | 列出根目录和子目录 | 与夹具实际文件集合一致 |
| TUI-TL02-02 | 列出含空格、中文和隐藏项目录 | 名称精确；隐藏策略可核对 |
| TUI-TL02-03 | 指定缺失路径或普通文件 | 有类型明确错误；不返回伪空目录成功 |
| TUI-TL02-04 | 列出数千条目目录 | 输出受限并声明截断；TUI 可取消 |
| TUI-TL02-05 | 列出工作区外或越权链接目录 | 安全拒绝；不泄露内容 |

## TL03 按内容搜索

入口：提问→search mode=grep。优先级：P1。执行通道：`L`。夹具：`search-corpus`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/search.rs](../../../core/src/tools/builtin/search.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL03-01 | 搜索唯一随机字串 | 命中文件、行号、内容精确 |
| TUI-TL03-02 | 搜索多文件并使用路径过滤 | 仅包含过滤内命中；排序按合同 |
| TUI-TL03-03 | 搜索无匹配 | 真实零结果；不编造候选 |
| TUI-TL03-04 | 搜索非法表达式或超长输入 | 明确参数错误；无模型循环和崩溃 |
| TUI-TL03-05 | 搜索大目录并取消 | 任务有界退出；无持久后台扫描残留 |

## TL04 按文件名搜索

入口：提问→search mode=glob。优先级：P1。执行通道：`L`。夹具：`search-corpus`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/search.rs](../../../core/src/tools/builtin/search.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL04-01 | 匹配嵌套文件名 | 结果集合等于独立枚举 |
| TUI-TL04-02 | 匹配空格、中文及字面特殊字符 | 语法按合同；无错文件 |
| TUI-TL04-03 | 使用不匹配模式 | 零结果；不回退到猜文件名 |
| TUI-TL04-04 | 请求越界路径模式 | 拒绝或限定工作区；外部 canary 不出现 |
| TUI-TL04-05 | 限制结果数量并翻查候选 | 截断明确；结果稳定且可 read |

## TL05 关键词检索

入口：提问→search mode=bm25。优先级：P1。执行通道：`L`。夹具：`search-corpus`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/search.rs](../../../core/src/tools/builtin/search.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL05-01 | 搜索已知唯一术语 | 正确文档进入结果；返回真实引用 |
| TUI-TL05-02 | 改变文档后再检索 | 按刷新合同看到新版本；无假新鲜结果 |
| TUI-TL05-03 | 删除文档再搜索 | 不返回可读取的旧引用；标明过期或移除 |
| TUI-TL05-04 | 空索引/索引损坏时搜索 | 降级或拒绝可见；不伪造命中 |
| TUI-TL05-05 | 大语料限量检索 | 结果和开销受限；无全量内容泄露 |

## TL06 语义检索

入口：提问→search mode=semantic。优先级：P1。执行通道：`C`。夹具：`semantic-runtime`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/builtin/search.rs](../../../core/src/tools/builtin/search.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL06-01 | 以同义改写查询固定语料 | 真实语义后端调用；相关目标在预设排名阈值内 |
| TUI-TL06-02 | 无 embedding/runtime 时请求 | 模式不广告或明确不可用；不能以 grep 冒充语义 |
| TUI-TL06-03 | 更新内容并等待声明刷新状态再查 | 版本/摘要与新内容一致 |
| TUI-TL06-04 | embedding 服务超时或报错 | 有界失败/显式降级；实际路径有证据 |
| TUI-TL06-05 | 查询跨工作区同名文件 | 结果隔离；外部语料不可见 |

## TL07 混合检索

入口：提问→search mode=hybrid。优先级：P1。执行通道：`C`。夹具：`semantic-runtime`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/builtin/search.rs](../../../core/src/tools/builtin/search.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL07-01 | 关键词和语义各有独占命中的查询 | 融合结果含两类真实来源 |
| TUI-TL07-02 | 语义后端失效 | 降级标明；无伪造融合分数 |
| TUI-TL07-03 | 重复文档同时命中 | 按文档身份去重；引用稳定 |
| TUI-TL07-04 | 设置 top-k 和路径范围 | 返回数和范围符合约束 |
| TUI-TL07-05 | 索引版本变化后继续提问 | 引用对应可读版本；不把旧内容当最新 |

## TL08 创建和覆盖文件

入口：提问→write。优先级：P0。执行通道：`L`。夹具：`verification-contract`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/mod.rs](../../../core/src/tools/builtin/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL08-01 | 创建含随机内容的新文件并验证 | 磁盘摘要精确；回执路径与 effect_digest 正确 |
| TUI-TL08-02 | 覆盖已有 UTF-8 文件 | 精确替换；无截断或换行污染 |
| TUI-TL08-03 | 写入新父目录 | 只创建所需目录；权限边界生效 |
| TUI-TL08-04 | 写入越界、只读或符号链接目标 | 拒绝；目标与外部 sentinel 不变 |
| TUI-TL08-05 | 写入完成后读回并结束 | 独立验证绑定当前变更；终态与检查结果一致 |

## TL09 精确编辑

入口：提问→edit。优先级：P0。执行通道：`L`。夹具：`verification-contract`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/mod.rs](../../../core/src/tools/builtin/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL09-01 | 使用正确 old_string/new_string 修改单处 | 只变该处；文件摘要和 diff 正确 |
| TUI-TL09-02 | 旧串不存在 | 明确失败；文件字节不变 |
| TUI-TL09-03 | 旧串多处出现且未明确全改 | 按歧义合同拒绝；不得误改多个位置 |
| TUI-TL09-04 | 并发修改后提交过期编辑 | 不覆写他人内容；冲突可诊断 |
| TUI-TL09-05 | 编辑中文、CRLF 和文件末尾 | 指定字节变化正确；未涉及内容保持 |

## TL10 补丁应用

入口：提问→patch。优先级：P0。执行通道：`L`。夹具：`patch-corpus`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/mod.rs](../../../core/src/tools/builtin/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL10-01 | 单文件多 hunk 补丁 | 所有 hunk 精确应用；报告对应变更 |
| TUI-TL10-02 | 新增/删除文件补丁 | 目标集合准确；未引用文件不变 |
| TUI-TL10-03 | 一个 hunk 不匹配 | 符合原子/部分合同并如实报告；不把部分成功当全成 |
| TUI-TL10-04 | 补丁含路径穿越或链接逃逸 | 拒绝；外部文件不变 |
| TUI-TL10-05 | 与他人修改冲突后重试 | 以最新文件重算；不重复应用已落盘 hunk |

## TL11 前台命令执行

入口：提问→bash command。优先级：P0。执行通道：`C`。夹具：`qualified-native`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/builtin/bash.rs](../../../core/src/tools/builtin/bash.rs)；[apps/code/core/src/sandbox/native.rs](../../../core/src/sandbox/native.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL11-01 | 执行随机 stdout/stderr、退出 0 的命令 | 真实退出码和分流输出正确 |
| TUI-TL11-02 | 执行退出非零的命令 | 失败码保留；模型不能声称检查通过 |
| TUI-TL11-03 | 运行带 cwd/env 的命令 | 工作目录与明确环境生效；无无关宿主变量泄露 |
| TUI-TL11-04 | 达到超时或取消 | 清理全部本轮后代；终态和副作用完整性如实反映 |
| TUI-TL11-05 | 平台 native 未合格 | 预执行明确拒绝；没有自动 host fallback |

## TL12 交互命令会话

入口：提问→bash session 控制。优先级：P0。执行通道：`C`。夹具：`qualified-native`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/builtin/bash.rs](../../../core/src/tools/builtin/bash.rs)；[apps/code/core/src/sandbox/native.rs](../../../core/src/sandbox/native.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL12-01 | 启动等待 stdin 的命令再送入随机行 | 同一会话收到输入；输出回执对应 |
| TUI-TL12-02 | 分次轮询命令输出 | 游标单调；无丢字和重复拼接 |
| TUI-TL12-03 | 向不存在/他人 session id 发送输入 | 拒绝；不影响其他进程 |
| TUI-TL12-04 | 进程已退出后继续轮询/发送 | 终态明确；不重启同名命令 |
| TUI-TL12-05 | 中断 TUI 后检查拥有的交互进程 | 按宿主合同清理/保留可恢复身份；无无主进程 |

## TL13 后台启动

入口：提问→background start。优先级：P0。执行通道：`C`。夹具：`qualified-native`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/builtin/background.rs](../../../core/src/tools/builtin/background.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL13-01 | 启动具随机输出的后台任务 | 返回真实 task id；start 不阻塞前台 |
| TUI-TL13-02 | 同一 Run 重名再次启动 | 按重名合同拒绝或明确替换；无重复孤儿 |
| TUI-TL13-03 | 指定错误 cwd 或空命令 | 不启动；参数错误可见 |
| TUI-TL13-04 | native 不支持后台时启动 | 明确 Unsupported/Denied；不绕过 sandbox |
| TUI-TL13-05 | 启动后立即取消拥有 Run | 任务清理并结算；未证明清理不能标完成 |

## TL14 后台输出和状态

入口：提问→background tail。优先级：P1。执行通道：`C`。夹具：`qualified-native`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/builtin/background.rs](../../../core/src/tools/builtin/background.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL14-01 | 分批输出时多次 tail | 输出片段和偏移正确；无重复丢失 |
| TUI-TL14-02 | 任务自然退出后 tail | 保留真实退出状态；有界末尾输出 |
| TUI-TL14-03 | 输出超过保留上限 | 截断可见；保留内存/磁盘有界 |
| TUI-TL14-04 | tail 不存在或其他 Run 的任务名 | 拒绝；无跨 Run 观察 |
| TUI-TL14-05 | tail 期间取消或 ACP 断连 | 请求有界；拥有者和任务身份不丢失 |

## TL15 后台停止与清理

入口：提问→background stop、x/Ctrl+B 入口。优先级：P0。执行通道：`C`。夹具：`qualified-native`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/builtin/background.rs](../../../core/src/tools/builtin/background.rs)。

动作参数化：`KillBgTask`、`SendToBackground`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL15-01 | 停止活跃后台任务 | 根进程及派生后代退出；结算事件可见 |
| TUI-TL15-02 | 重复停止已退出任务 | 有界且幂等；不杀复用名称的其他进程 |
| TUI-TL15-03 | 任务忽略终止信号 | 只对拥有进程树升级清理；无法证明则拒绝成功结算 |
| TUI-TL15-04 | 在滚动区按 x 或 Ctrl+B | 仅在支持的入口执行；身份绑定，不误停旁边任务 |
| TUI-TL15-05 | 退出 TUI 或测试 host 崩溃 | 本测试进程树按平台合同清理；外部 sentinel 进程仍存活 |

## TL16 Git 状态

入口：提问→git status。优先级：P1。执行通道：`L`。夹具：`disposable-git`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/git.rs](../../../core/src/tools/builtin/git.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL16-01 | 干净仓库查状态 | 无变化；HEAD 和文件摘要不变 |
| TUI-TL16-02 | 未跟踪、修改、暂存文件查状态 | 分类准确；含空格文件名精确 |
| TUI-TL16-03 | 并行他人提交时查询 | 只读调用不认领外来提交为本轮写入 |
| TUI-TL16-04 | 非 Git 目录调用 | 明确错误；不创建仓库 |
| TUI-TL16-05 | 嵌套工作树调用 | 定位正确仓库；不读取外部敏感仓库 |

## TL17 Git 日志

入口：提问→git log。优先级：P1。执行通道：`L`。夹具：`disposable-git`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/git.rs](../../../core/src/tools/builtin/git.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL17-01 | 读取三条固定提交 | id、顺序和消息符合真实日志 |
| TUI-TL17-02 | 限定 limit/max_count | 输出数量与别名合同一致 |
| TUI-TL17-03 | 过滤分支/引用 | 只返回目标历史 |
| TUI-TL17-04 | 非法引用或边界数字 | 明确错误；无默认猜引用 |
| TUI-TL17-05 | 查询中他人新增提交 | 结果有一致观察界限；本轮无副作用 |

## TL18 Git 分支

入口：提问→git branch。优先级：P1。执行通道：`L`。夹具：`disposable-git`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/git.rs](../../../core/src/tools/builtin/git.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL18-01 | 列出本地分支 | 集合与 Git 实际引用一致 |
| TUI-TL18-02 | 创建新分支 | 只创建目标引用；HEAD 状态符合命令合同 |
| TUI-TL18-03 | 创建重复或非法名字 | 明确拒绝；既有分支不变 |
| TUI-TL18-04 | 请求删除当前或受保护分支 | 遵守显式权限；不误删 |
| TUI-TL18-05 | 分支操作后检查完成门 | 真实引用变化被追踪；不能当只读免验 |

## TL19 Git 检出

入口：提问→git checkout。优先级：P0。执行通道：`L`。夹具：`disposable-git`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/git.rs](../../../core/src/tools/builtin/git.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL19-01 | 检出合法分支 | HEAD 和文件内容对应目标提交 |
| TUI-TL19-02 | 未提交冲突时普通检出 | 拒绝且保留工作区内容 |
| TUI-TL19-03 | 请求 force 检出但拒绝审批 | 零覆盖；拒绝结果可追溯 |
| TUI-TL19-04 | 检出不存在引用 | 明确失败；HEAD 不变 |
| TUI-TL19-05 | 检出导致文件增删后收尾 | 变更归属包含检出效果；绑定真实验证 |

## TL20 Git 差异

入口：提问→git diff。优先级：P1。执行通道：`L`。夹具：`disposable-git`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/git.rs](../../../core/src/tools/builtin/git.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL20-01 | 查询已知行修改 | diff 精确；无文件写入 |
| TUI-TL20-02 | 分别查询工作树、暂存、目标 ref | 每种范围对应正确变化 |
| TUI-TL20-03 | 二进制文件或大补丁 | 有界表示；不损坏终端 |
| TUI-TL20-04 | 非法目标 ref/路径 | 拒绝；不猜分支 |
| TUI-TL20-05 | 并发提交后查 diff | 报告当前观察内容；不记成模型修改 |

## TL21 Git stash

入口：提问→git stash。优先级：P0。执行通道：`L`。夹具：`disposable-git`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/git.rs](../../../core/src/tools/builtin/git.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL21-01 | 按广告支持动作保存工作树 | stash 内容与状态精确；效果入账 |
| TUI-TL21-02 | 查看已有 stash | 只读；无状态变化 |
| TUI-TL21-03 | 恢复指定 stash | 准确目标；冲突如实报告 |
| TUI-TL21-04 | 空仓库/无变化/不存在 stash | 区分空操作与错误；无假恢复 |
| TUI-TL21-05 | 取消或拒绝破坏性动作 | 不删除 stash 或覆盖工作树 |

## TL22 Git remote

入口：提问→git remote。优先级：P1。执行通道：`L`。夹具：`disposable-git`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/git.rs](../../../core/src/tools/builtin/git.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL22-01 | 查看固定 remote 集合 | 名称和脱敏地址正确 |
| TUI-TL22-02 | 对 schema 未开放的修改动作发起请求 | 明确拒绝；不借 bash 自动执行 |
| TUI-TL22-03 | 含 URL 凭证 remote 出错 | 界面和证据不泄露凭证 canary |
| TUI-TL22-04 | 无 remote 仓库查询 | 真实空态；不虚构 origin |
| TUI-TL22-05 | 并发改变 remote 后再查询 | 最新结果来自真实仓库；不缓存旧值冒充 |

## TL23 Git worktree

入口：提问→git worktree list/create/remove；欢迎页 New worktree / Ctrl+W。优先级：P0。执行通道：`L`。夹具：`disposable-git`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/git.rs](../../../core/src/tools/builtin/git.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/welcome/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/welcome/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/router.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/router.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/lifecycle.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/lifecycle.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/new_worktree_dialog.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/new_worktree_dialog.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL23-01 | 列出两个工作树 | 路径、分支和 HEAD 正确 |
| TUI-TL23-02 | 在允许目录创建分支工作树 | 目录与 Git 注册一致；不创建新仓库 |
| TUI-TL23-03 | 删除测试自己创建且干净的工作树 | 目录和注册同时消失 |
| TUI-TL23-04 | 请求删除脏或他人工作树并拒绝权限 | 内容和注册不变 |
| TUI-TL23-05 | 创建越界路径或重复分支 | 明确拒绝；无孤立目录残留 |
| TUI-TL23-06 | 欢迎页 Ctrl+W 打开对话框，输入草稿后取消 | 对话框真实出现；取消恢复焦点与先前输入；Git refs、目录不变；新增模型请求=0 |
| TUI-TL23-07 | 独占仓库 HEAD 固定到已有提交后，在欢迎页输入合法名称并创建 | 实际 worktree 路径、HEAD、分支和新 ACP session 一致；原工作树字节不变；无假 Ready |
| TUI-TL23-08 | 依固定名称 schema 分别尝试重复、路径式、中文及超限名称 | 接受或拒绝与预先固定 schema 一致；无越界路径、半建目录、错误分支或丢失资源身份 |
| TUI-TL23-09 | 在非 Git、只读及有脏文件的独占工作区操作欢迎页创建 | 错误或成功状态准确；不丢弃脏文件、不认领他人 worktree；正常输入仍可使用 |
| TUI-TL23-10 | Git 创建后使本次 ACP 初始化失败，再经真实入口重试 | 展示已创建资源和失败阶段；补偿仅清理本次拥有对象，或保留可恢复身份；不重复创建、不显示假成功 |

## TL24 网页获取

入口：提问→web_fetch。优先级：P0。执行通道：`L`。夹具：`controlled-public-http`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/web_fetch.rs](../../../core/src/tools/builtin/web_fetch.rs)；[apps/code/core/src/tools/builtin/web_search.rs](../../../core/src/tools/builtin/web_search.rs)；[apps/code/core/src/tools/builtin/download/mod.rs](../../../core/src/tools/builtin/download/mod.rs)；[apps/code/core/src/tools/builtin/safe_http.rs](../../../core/src/tools/builtin/safe_http.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL24-01 | 获取有随机标记的受控公开页面 | 真实 HTTP 命中；正文和引用准确 |
| TUI-TL24-02 | 重定向到另一个允许公网页面 | 每跳安全检查；最终来源正确 |
| TUI-TL24-03 | 重定向或 DNS rebinding 到私网/元数据地址 | 拒绝；禁区服务命中数为零 |
| TUI-TL24-04 | 超大、缓慢、压缩炸弹响应 | 按限制结束；TUI 可取消 |
| TUI-TL24-05 | 404/500/证书错误 | 保留真实错误；不声称已读取页面 |

## TL25 网页搜索

入口：提问→web_search。优先级：P1。执行通道：`L`。夹具：`controlled-search`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/web_fetch.rs](../../../core/src/tools/builtin/web_fetch.rs)；[apps/code/core/src/tools/builtin/web_search.rs](../../../core/src/tools/builtin/web_search.rs)；[apps/code/core/src/tools/builtin/download/mod.rs](../../../core/src/tools/builtin/download/mod.rs)；[apps/code/core/src/tools/builtin/safe_http.rs](../../../core/src/tools/builtin/safe_http.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL25-01 | 查唯一公开随机术语 | 真实搜索请求；结果 URL 与来源相符 |
| TUI-TL25-02 | 返回无结果 | 明确零命中；不杜撰引用 |
| TUI-TL25-03 | 搜索服务限流/鉴权失败 | 错误和重试有界；密钥不泄露 |
| TUI-TL25-04 | 点击/继续抓取搜索结果 | 实际抓取使用相同网络安全边界 |
| TUI-TL25-05 | 大量重复或恶意 URL 结果 | 去重和限量；拒绝不安全目标 |

## TL26 下载文件

入口：提问→download。优先级：P0。执行通道：`L`。夹具：`controlled-public-http`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/web_fetch.rs](../../../core/src/tools/builtin/web_fetch.rs)；[apps/code/core/src/tools/builtin/web_search.rs](../../../core/src/tools/builtin/web_search.rs)；[apps/code/core/src/tools/builtin/download/mod.rs](../../../core/src/tools/builtin/download/mod.rs)；[apps/code/core/src/tools/builtin/safe_http.rs](../../../core/src/tools/builtin/safe_http.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL26-01 | 下载固定摘要文件 | 落盘字节、长度和摘要一致；效果入账 |
| TUI-TL26-02 | 下载覆盖已有文件并拒绝审批 | 旧文件不变；无临时内容残留 |
| TUI-TL26-03 | 下载被取消/断流 | 不把部分文件当成功；清理或标记部分状态 |
| TUI-TL26-04 | 下载指向私网或重定向越界 | 禁区零请求；无目标写入 |
| TUI-TL26-05 | 指定越界/链接路径 | 安全拒绝；外部 sentinel 不变 |

## TL27 批量工具

入口：提问→batch。优先级：P0。执行通道：`L`。夹具：`mixed-tool-batch`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/batch.rs](../../../core/src/tools/builtin/batch.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL27-01 | 批量只读多个文件 | 结果逐 invocation 绑定；无内容串线 |
| TUI-TL27-02 | 混合成功和失败子调用 | 逐项真实状态；不合并成全成功 |
| TUI-TL27-03 | 同批包含写入和读回 | 依赖符合合同；所有效果正确入账 |
| TUI-TL27-04 | 超数量、嵌套 batch 或未知工具 | 明确参数/能力拒绝；无部分越权执行 |
| TUI-TL27-05 | 批量运行中取消 | 已开始与未开始项可区分；拥有子任务清理 |

## TL28 计划清单

入口：提问→update_plan、Ctrl+T。优先级：P1。执行通道：`L`。夹具：`planning`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/update_plan.rs](../../../core/src/tools/builtin/update_plan.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

动作参数化：`ToggleTodos`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL28-01 | 创建三步计划 | 真实 PlanEntry 与任务 id 显示对应 |
| TUI-TL28-02 | 逐步更新 pending/in_progress/completed | 状态单调且与实际动作相符 |
| TUI-TL28-03 | 任务 Failed/Skipped/Cancelled | 保留 a3sStatus；UI 不只显示成功完成 |
| TUI-TL28-04 | 无任务时打开 todo 面板 | 明确空态；无虚构条目 |
| TUI-TL28-05 | 恢复长任务后打开计划 | 未完成步骤仍在；历史不重复 |

## TL29 结构化结果

入口：提问→generate_object。优先级：P1。执行通道：`L`。夹具：`object-schema`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/tools/builtin/generate_object.rs](../../../core/src/tools/builtin/generate_object.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-TL29-01 | 要求满足固定 schema 的对象 | 真实 LLM 生成；独立 schema 校验通过 |
| TUI-TL29-02 | 使用 auto/strict/json/tool/prompt 各可用模式 | 实际适配路径正确；不假装 provider 原生支持 |
| TUI-TL29-03 | 输出缺少必需字段或错误类型 | 明确失败或有界修复；无未验证成功 |
| TUI-TL29-04 | 给非法/过深/超大 schema | 有界拒绝；UI 保持交互 |
| TUI-TL29-05 | 取消嵌套模型调用 | 拥有的子请求结束；无挂起 Run |

## EX01 技能发现

入口：提问→search_skills。优先级：P1。执行通道：`C`。夹具：`fixture-skill-pack`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/skill.rs](../../../core/src/tools/skill.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX01-01 | 搜索测试技能唯一标记 | 结果来自已加载技能目录 |
| TUI-EX01-02 | 两个同名技能来源不同 | 返回规范身份；不按猜测选错 |
| TUI-EX01-03 | 没有匹配技能 | 真实零结果；继续基础工具而非伪造技能 |
| TUI-EX01-04 | 损坏/不可读技能 | 局部错误可见；其他技能可查 |
| TUI-EX01-05 | 禁用或移除技能后新轮次再查 | 目录更新遵守 generation；当前 Run 不被偷偷替换 |

## EX02 技能执行

入口：提问→Skill。优先级：P0。执行通道：`C`。夹具：`fixture-skill-pack`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/skill.rs](../../../core/src/tools/skill.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX02-01 | 执行只有指令的技能 | 正确指令参与当前轮；无无关工具授权 |
| TUI-EX02-02 | 执行 allowed-tools 限制技能 | 嵌套调用严格使用允许集合 |
| TUI-EX02-03 | 以非法参数别名代替 skill_name | schema 拒绝；不触发技能 |
| TUI-EX02-04 | 技能尝试越权写入或进程执行 | 仍受宿主权限和 sandbox 管理 |
| TUI-EX02-05 | 执行中更新/移除技能再取消 | 运行使用捕获版本；清理可证明；后轮使用新目录 |

## EX03 子代理委派

入口：提问→task。优先级：P0。执行通道：`C`。夹具：`fixture-agent-pack`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/task.rs](../../../core/src/tools/task.rs)；[apps/code/core/src/agent_api/capabilities.rs](../../../core/src/agent_api/capabilities.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX03-01 | 委派单个只读子任务 | 真实子模型请求；parent/child Run 身份正确 |
| TUI-EX03-02 | 委派未知 agent 或超权限任务 | 拒绝；不提升父权限 |
| TUI-EX03-03 | 子任务产生文件修改 | 效果归属和验证报告传播父完成门 |
| TUI-EX03-04 | 子代理失败/超时 | 父可见真实错误；不伪造成功产物 |
| TUI-EX03-05 | 取消父轮次 | 所有拥有子代理终止/结算；无晚到写入 |

## EX04 并行委派能力门与隔离

入口：提问→task tasks / parallel_task。优先级：P0。执行通道：`C`。夹具：`fixture-agent-pack`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/task.rs](../../../core/src/tools/task.rs)；[apps/code/core/src/agent_api/capabilities.rs](../../../core/src/agent_api/capabilities.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX04-01 | 默认 local-code 询问并行专用工具 | 未广告 parallel_task 不得虚构调用 |
| TUI-EX04-02 | 支持配置下并行两个独立子任务 | 请求实际并发且不超 cap；结果有独立身份 |
| TUI-EX04-03 | 一子任务成功一失败 | 逐子任务状态和产物准确；不能全通过 |
| TUI-EX04-04 | 多个子任务争写同一路径 | 冲突按合同处理；不能静默互相覆盖 |
| TUI-EX04-05 | 并行期间切模型/取消 | 子任务保留捕获能力和模型；清理拥有任务 |

## EX05 MCP 连接与工具目录

入口：启动已授权 MCP→自然语言。优先级：P0。执行通道：`C`。夹具：`fixture-mcp`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/mcp/manager.rs](../../../core/src/mcp/manager.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mcp.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mcp.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX05-01 | stdio/http/sse 各启动合法服务器 | 实际握手和工具 schema 可核对 |
| TUI-EX05-02 | 服务器不可达或握手不兼容 | 只禁用该来源；错误可诊断 |
| TUI-EX05-03 | 同名工具来自两个服务器 | 规范绑定无歧义；不调用错服务器 |
| TUI-EX05-04 | server tools/list_changed 后发新轮次 | 目录刷新；旧 Run 仍用捕获身份 |
| TUI-EX05-05 | 未授权外部 stdio 或恶意工作区 MCP | 不启动外部程序；没有权限扩张 |

## EX06 MCP 调用与回传

入口：自然语言→MCP tool。优先级：P0。执行通道：`C`。夹具：`fixture-mcp`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/mcp/manager.rs](../../../core/src/mcp/manager.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mcp.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mcp.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX06-01 | 调用返回随机标记的工具 | 真实请求 id 与成功结果绑定 |
| TUI-EX06-02 | 返回文本和结构化/二进制内容 | 有界转换；类型与字节正确 |
| TUI-EX06-03 | 服务端返回协议/工具错误 | 保留错误分类；不展示为正常结果 |
| TUI-EX06-04 | 超时、断连或取消调用 | 有界失败；无悬空响应和重复重试副作用 |
| TUI-EX06-05 | 修改工具版本后恢复历史调用 | 不把新版本当旧身份重新结算 |

## EX07 MCP progress 和目录通知

入口：MCP 通知→TUI。优先级：P1。执行通道：`C`。夹具：`fixture-mcp`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/mcp/manager.rs](../../../core/src/mcp/manager.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mcp.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mcp.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX07-01 | 调用中多次 progress | 所属工具进度有序可见；不提前完成 |
| TUI-EX07-02 | 同时发 progress 和 list_changed | 原通知 FIFO 保留；目录刷新不吞进度 |
| TUI-EX07-03 | 发重复或未知请求 token | 不绑定其他调用；诊断有界 |
| TUI-EX07-04 | 空闲服务器断开/关闭 | 通知刷新 worker 被清理；无忙循环 |
| TUI-EX07-05 | 大通知突发并取消 | 保留有界前缀/缺口标记；最终清理完成 |

## EX08 MCP 用户提问、资源和采样

入口：MCP→TUI consent。优先级：P0。执行通道：`C`。夹具：`fixture-mcp`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/mcp/manager.rs](../../../core/src/mcp/manager.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mcp.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/acp_handler/mcp.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX08-01 | 服务器请求 elicitation 并填写 | 用户答案绑定准确请求和服务器 |
| TUI-EX08-02 | 用户拒绝资源/采样授权 | 无模型采样或资源读取副作用 |
| TUI-EX08-03 | YOLO 下服务器提问/采样 | 仍要求真实用户同意；无自动代答 |
| TUI-EX08-04 | 采样获批后使用允许模型与预算 | 真实采样请求有上限；凭证不回传服务器 |
| TUI-EX08-05 | 请求等待期间断连/取消 | 无永久挂起；未回答不当批准 |

## EX09 代码符号

入口：提问→code_symbols。优先级：P1。执行通道：`C`。夹具：`code-intelligence-host`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/builtin/code_intelligence/mod.rs](../../../core/src/tools/builtin/code_intelligence/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX09-01 | 列出已保存文件符号 | 来自真实语言服务；名称和范围正确 |
| TUI-EX09-02 | Unicode 标识符及嵌套符号 | 层级、位置与源文件相符 |
| TUI-EX09-03 | 不存在/越界文件 | 拒绝；不猜符号 |
| TUI-EX09-04 | 语言服务不可用 | 工具不广告或明确 unavailable；不以正则冒充 |
| TUI-EX09-05 | 编辑并保存后查询 | 返回最新文档版本或明确 stale |

## EX10 语义导航

入口：提问→code_navigation。优先级：P1。执行通道：`C`。夹具：`code-intelligence-host`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/builtin/code_intelligence/mod.rs](../../../core/src/tools/builtin/code_intelligence/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX10-01 | 请求 definition/declaration | 真实目标 URI 和 UTF-16 位置准确 |
| TUI-EX10-02 | 请求 references/implementations | 集合与固定工程基准一致 |
| TUI-EX10-03 | emoji 前后位置定位 | 正确使用 UTF-16；不跳错字符 |
| TUI-EX10-04 | 非法位置/跨工作区引用 | 有类型错误或安全拒绝 |
| TUI-EX10-05 | 服务超时或取消 | 有界返回；无后台挂起请求 |

## EX11 代码诊断

入口：提问→code_diagnostics。优先级：P1。执行通道：`C`。夹具：`code-intelligence-host`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/tools/builtin/code_intelligence/mod.rs](../../../core/src/tools/builtin/code_intelligence/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX11-01 | 查询含固定类型错误文件 | 真实诊断码、位置和严重性准确 |
| TUI-EX11-02 | 修复后保存再查 | 旧诊断消失或标记版本；无假清零 |
| TUI-EX11-03 | 多文件诊断限量 | 范围与截断合同正确 |
| TUI-EX11-04 | 服务不可用/不支持语言 | 明确 unavailable/unsupported |
| TUI-EX11-05 | 大工程查询中取消 | 响应有界；TUI 保持可用 |

## EX12 持久记忆的间接入口

入口：自然语言记忆请求。优先级：P0。执行通道：`C`。夹具：`memory-profile`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/core/src/agent_api/conversation_runtime.rs](../../../core/src/agent_api/conversation_runtime.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-EX12-01 | 明确请求记住测试偏好后重启追问 | 仅在实际启用的存储配置下验证模型条目、提交确认和检索 |
| TUI-EX12-02 | 普通叙述含 remember 字样但无授权语义 | 不得靠关键词持久化；无越权记忆 |
| TUI-EX12-03 | 请求忘记指定记录 | 验证真实存储/索引删除或 tombstone；不能只说忘记 |
| TUI-EX12-04 | 写入失败或存储 disabled | 明确未保存；End 不能当落盘证明 |
| TUI-EX12-05 | 跨项目/会话使用记忆 | 按 scope 和 Active-only 合同隔离；不能召回敏感 canary |

## UI01 设置面板交互

入口：/settings、/prefs、/preferences。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

动作参数化：`OpenSettings`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI01-01 | 打开并按类别浏览所有有效设置 | 目录与 inventory、后端过滤一致 |
| TUI-UI01-02 | 搜索规范名、标签和 yolo 别名 | 只展示正确设置；隐藏项不被搜索复活 |
| TUI-UI01-03 | 修改一个设置后 Esc 取消预览 | 原值恢复；无持久化写入 |
| TUI-UI01-04 | 保存失败后再次打开 | 回滚到已确认值；失败提示不丢 |
| TUI-UI01-05 | 小窗口中翻页、搜索、退出 | 选中项可达；不误修改邻行 |

## UI02 主题与系统外观

入口：/theme、/t、主题设置。优先级：P2。执行通道：`U`。夹具：`terminal-themes`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI02-01 | 依次选择已广告主题 | 文本可读；选择值与实际配色一致 |
| TUI-UI02-02 | auto 下切系统明暗状态 | 使用正确 auto_dark/auto_light 主题 |
| TUI-UI02-03 | 未知主题参数 | 明确拒绝；已接受主题不变 |
| TUI-UI02-04 | 切主题时有模态框和流式工具输出 | 全部视图更新；无残影和焦点丢失 |
| TUI-UI02-05 | 保存后重启以及只读配置失败 | 持久化/回滚遵守设置合同 |

## UI03 全屏与 minimal 切换

入口：/minimal、/fullscreen、/full。优先级：P1。执行通道：`L`。夹具：`two-sessions`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/mode_switch.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/mode_switch.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/screen_mode_switch.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/screen_mode_switch.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI03-01 | 全屏会话切 minimal 并继续追问 | 同 session；历史和模型连续 |
| TUI-UI03-02 | minimal 切回全屏 | 终端屏幕模式正确；没有重复工具执行 |
| TUI-UI03-03 | 运行中切屏 | 执行/取消规则明确；旧轮次终态不丢 |
| TUI-UI03-04 | 带草稿、审批卡片切屏 | 保留或明确要求先处理；不能默许审批 |
| TUI-UI03-05 | 在已处于目标模式或无会话时执行 | 提示原因；不重复启动后端 |

## UI04 终端尺寸和复合字符

入口：terminal_resize。优先级：P1。执行通道：`U`。夹具：`rich-transcript`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI04-01 | 120x40→80x24→40x12 缩放 | 文字/输入/底栏有界且仍可退出 |
| TUI-UI04-02 | 流式输出期间反复缩放 | 无 panic、闪回或重复内容 |
| TUI-UI04-03 | 中文、emoji、组合字符跨行 | 宽度和光标正确；无半字符截断 |
| TUI-UI04-04 | 长代码行、表格和差异块缩放 | 布局可阅读；复制仍是原内容 |
| TUI-UI04-05 | 恢复原尺寸 | 历史锚点和模态框状态正确 |

## UI05 鼠标和选区

入口：/toggle-mouse-reporting、滚轮、拖选。优先级：P1。执行通道：`U`。夹具：`terminal-mouse`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/actions/defaults.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/actions/defaults.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/actions.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/actions.rs)。

动作参数化：`ToggleMouseCapture`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI05-01 | 开启/关闭鼠标报告 | 符合终端模式；关闭后宿主选择可用 |
| TUI-UI05-02 | 滚轮和触控板上下滚动 | 方向/速度按设置；不误选或执行块 |
| TUI-UI05-03 | 双击字词、拖动跨行选区 | 选区与复制内容精确 |
| TUI-UI05-04 | 按 Shift 使用终端原生选择 | 不被 TUI 吞掉；无重复拷贝 |
| TUI-UI05-05 | 选区期间新输出/缩放 | 选区规则一致；不泄露其他会话内容 |

## UI06 Markdown、代码和差异渲染

入口：助手正文与 edit 卡片。优先级：P1。执行通道：`U`。夹具：`rich-transcript`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI06-01 | 显示标题、列表、表格和代码块 | 结构可读；不丢原文本 |
| TUI-UI06-02 | 代码高亮含中文/长行 | 无转义执行；复制准确 |
| TUI-UI06-03 | 实际 edit/patch 产生差异 | 路径、加减行数与真实文件 diff 一致 |
| TUI-UI06-04 | 输出含伪造 ANSI/OSC 控制文本 | 不得改变标题、注入输入或剪贴板 |
| TUI-UI06-05 | 渲染失败或未知语言 | 回退可读文本；会话不崩溃 |

## UI07 Mermaid 展示与打开

入口：render_mermaid 设置、块查看器。优先级：P2。执行通道：`U`。夹具：`fake-open-process`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI07-01 | 显示合法 Mermaid 块并打开 | 只打开该图；源码可追溯 |
| TUI-UI07-02 | 设置 off 后再输出图 | 仅源码；无外部打开副作用 |
| TUI-UI07-03 | 损坏/超大图 | 错误有界或源码回退；不锁死 TUI |
| TUI-UI07-04 | 图含外部链接或危险内容 | 按打开策略处理；不执行任意脚本命令 |
| TUI-UI07-05 | 缺少图形终端支持 | 文字可读；不声称图已渲染 |

## UI08 调试状态和滚动 HUD

入口：/debug、/debug scroll、/scroll-debug。优先级：P2。执行通道：`U`。夹具：`long-transcript`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/debug.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/debug.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/scroll_debug.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/scroll_debug.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/gboom.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/gboom.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI08-01 | 调用 /debug | 真实开关状态显示；不启动模型 |
| TUI-UI08-02 | 两种 scroll 拼写开关 | 作用于同一 HUD；再次调用关闭 |
| TUI-UI08-03 | release 构建打开命令面板 | /debug 不列出；已声明可手输诊断仍有效 |
| TUI-UI08-04 | 传未知子命令 | 用法错误；无状态变更 |
| TUI-UI08-05 | 显示 HUD 时滚动/缩放/退出 | 不卡住；不污染导出文本 |

## UI09 FPS 调试

入口：/debug fps。优先级：P2。执行通道：`U`。夹具：`long-transcript`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/debug.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/debug.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/scroll_debug.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/scroll_debug.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/gboom.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/gboom.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI09-01 | 开关 FPS | 仅本地浮层出现/消失 |
| TUI-UI09-02 | 空闲时观察 | 刷新有界；无持续满核忙循环 |
| TUI-UI09-03 | 流式+滚动时观察 | 数值可更新；不阻塞交互 |
| TUI-UI09-04 | minimal 或不支持视图中开启 | 安全提示或兼容渲染；无损终端 |
| TUI-UI09-05 | 退出重启 | 按合同清理；无后台测量 worker 残留 |

## UI10 滚动诊断记录

入口：/debug log。优先级：P2。执行通道：`U`。夹具：`long-transcript`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/debug.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/debug.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/scroll_debug.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/scroll_debug.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/gboom.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/gboom.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI10-01 | 开启记录并滚动后关闭 | 产生本测试独有文件；事件与动作对应 |
| TUI-UI10-02 | 重复开关 | 新文件/关闭状态符合合同；无 fd 泄漏 |
| TUI-UI10-03 | 记录目录不可写 | 明确错误；正常交互继续 |
| TUI-UI10-04 | 输入 secret canary 后检查留存 | 按声明日志政策脱敏/限域；证据不外传原秘密 |
| TUI-UI10-05 | 输出达到保留上限 | 截断/轮转有界；无无限磁盘增长 |

## UI11 隐藏游戏入口

入口：手输 /gboom。优先级：P2。执行通道：`U`。夹具：`kitty-terminal`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/debug.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/debug.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/scroll_debug.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/scroll_debug.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/gboom.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/gboom.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI11-01 | 活跃会话执行裸命令 | 支持终端打开游戏；无模型调用 |
| TUI-UI11-02 | 菜单搜索 gboom | 始终不列出 |
| TUI-UI11-03 | 无 kitty 图形支持终端执行 | 提示或文字回退；不得破坏终端 |
| TUI-UI11-04 | 进入游戏后退出 | 回到原会话、草稿与滚动位置 |
| TUI-UI11-05 | 带参数调用 | 按当前 PassThrough 合同记录行为；不得把隐藏游戏当无副作用安全命令 |

## UI12 终端诊断

入口：/doctor 及三个别名。优先级：P1。执行通道：`U`。夹具：`terminal-variants`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/doctor.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/doctor.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI12-01 | 标准终端执行诊断 | 报告真实键码/能力；无模型请求 |
| TUI-UI12-02 | SSH/tmux/VS Code 环境执行 | 区分环境；建议对应实际能力 |
| TUI-UI12-03 | 输入不支持的子参数 | 用法可诊断；不修改用户 shell |
| TUI-UI12-04 | 诊断后发送普通任务 | TUI 保持正确输入/屏幕模式 |
| TUI-UI12-05 | 导出诊断 | 不包含密钥；版本、路径和能力足以复现 |

## UI13 工作目录入口限制

入口：手输 /cd [path]。优先级：P1。执行通道：`U`。夹具：`two-workspaces`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/session/mod.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI13-01 | Code 会话打开菜单搜索 cd | dashboard_only 生效；不宣称可切 cwd |
| TUI-UI13-02 | 手输 /cd 测试子目录 | 提示需要 dashboard；当前 cwd 不变 |
| TUI-UI13-03 | 手输空 /cd | 不打开不可用 dashboard 流程 |
| TUI-UI13-04 | 用带空格或越界参数调用 | 同样不执行目录切换；无外部路径访问 |
| TUI-UI13-05 | 拒绝后读取当前工作区随机文件 | 仍在原 cwd；会话可继续 |

## UI14 语音快捷键限制

入口：Ctrl+Space、F8、语音设置。优先级：P0。执行通道：`U`。夹具：`fake-audio-host`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/voice.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/voice.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)。

动作参数化：`EnableVoiceMode`、`VoiceToggle`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI14-01 | Code 后端按两种语音键 | 禁用状态下不得打开麦克风或外部 STT |
| TUI-UI14-02 | 从首页、活跃输入和模态框按语音键 | 能力门一致；不旁路 /voice 禁用 |
| TUI-UI14-03 | 切换 voice_keybind_enabled 后再次触发 | 设置不能擅自恢复未授权服务 |
| TUI-UI14-04 | 按住/释放或重复快速按语音键 | 无遗留录音 worker；终端正常 |
| TUI-UI14-05 | 故障/不支持音频平台触发 | 明确不可用；无假识别文本或秘密网络请求 |

## UI15 退出与进程清理

入口：欢迎页 Quit / Ctrl+Q；/quit、/exit、EOF。优先级：P0。执行通道：`U`。夹具：`owned-process`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/actions/defaults.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/actions/defaults.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/actions.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/actions.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/welcome/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/welcome/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/router.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/router.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI15-01 | 空闲正常退出 | 退出码正常；终端 echo/鼠标/alternate-screen 恢复 |
| TUI-UI15-02 | 流式任务中退出 | 拥有 ACP 和 worker 按合同结束；无晚到写入 |
| TUI-UI15-03 | 审批/问题等待中退出 | 所有等待者释放；不能退出后执行批准 |
| TUI-UI15-04 | 第一次 Ctrl+C 与第二次中断 | 按声明取消/强制清理语义；只杀本测试树 |
| TUI-UI15-05 | 关闭 PTY/测试 host 意外退出 | 平台拥有树回收；不杀旁边 sentinel 进程 |

## SC01 项目指令与常驻上下文

入口：AGENTS.md 链、启动上下文。优先级：P0。执行通道：`L`。夹具：`instruction-chain`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/acp/src/config.rs](../../../acp/src/config.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SC01-01 | 项目嵌套 AGENTS 指定随机规则后提问 | 真实模型输入含正确链；执行符合适用层级 |
| TUI-SC01-02 | 个人与项目指令冲突 | 按声明优先级；不根据文件出现顺序猜权威 |
| TUI-SC01-03 | 指令文件含过大文本/链接/FIFO | 有界读取拒绝；不无限阻塞 |
| TUI-SC01-04 | 压缩、切模型并恢复会话 | 现行指令和约束保留；旧目标不复活 |
| TUI-SC01-05 | 工具读取的恶意提示要求泄密 | 不取得宿主授权；外部 canary 无泄漏 |

## SC02 sandbox 网络与写入边界

入口：自然语言工具任务。优先级：P0。执行通道：`C`。夹具：`sandbox-policy`。

可达性：CONDITIONAL: 仅产品实际启动路径广告并可执行时运行正向；否则记录入口拒绝与正向 BLOCKED。

源码：[apps/code/acp/src/config.rs](../../../acp/src/config.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SC02-01 | deny 网络姿态下发公网命令 | 实际网络零命中；没有 process-host 自动回退 |
| TUI-SC02-02 | mediated 只允许目标域名 | 允许目标可达；其他目标拒绝 |
| TUI-SC02-03 | YOLO 下访问私网/外部文件 | 权限和网络边界仍生效 |
| TUI-SC02-04 | 子代理、技能和背景任务发相同请求 | 继承同一宿主 sandbox；无嵌套逃逸 |
| TUI-SC02-05 | 无法证明后代清理时尝试收尾 | fail closed；保留未结算所有权而非清零 |

## SC03 幂等、重放与身份绑定

入口：恢复、重试、晚到事件。优先级：P0。执行通道：`H`。夹具：`fault-ledger`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/fact_control/receipts.rs](../../../core/src/fact_control/receipts.rs)；[apps/code/core/src/sandbox/native.rs](../../../core/src/sandbox/native.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SC03-01 | 重放同一已落盘 tool result | 不重复落盘副作用；摘要和身份不变 |
| TUI-SC03-02 | 另一个 Run 同名工具/相同参数回执 | 不能结算本 Run 的 Unknown |
| TUI-SC03-03 | 旧 generation 终态晚到新轮次 | 不覆盖新状态；保留正确归属 |
| TUI-SC03-04 | 中途崩溃后重试同一请求 | 区分观察重放与真实再执行；无假幂等 |
| TUI-SC03-05 | 日志损坏或序号缺口时恢复 | 明确不完整；不把缺记录推断为零副作用 |

## SC04 性能、资源上限与耐久性

入口：长会话、重复任务、崩溃恢复。优先级：P1。执行通道：`L`。夹具：`soak-workspace`。

可达性：当前源码入口；实际行为待运行确认。

源码：[apps/code/core/src/agent_api/conversation_runtime.rs](../../../core/src/agent_api/conversation_runtime.rs)；[apps/code/acp/src/agent.rs](../../../acp/src/agent.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-SC04-01 | 真实 LLM 50 轮混合任务 | 记录成功率、p50/p95、内存/FD/进程；无未归属残留 |
| TUI-SC04-02 | 十万行工具输出加连续输入 | 本地输入延迟满足预设 SLO；截断明确 |
| TUI-SC04-03 | 连续打开/关闭 100 次设置、查看器和选择器 | 无单调 FD/worker/内存泄漏 |
| TUI-SC04-04 | 运行 2 小时后重启继续 | 历史和义务可恢复；资源仍在上限内 |
| TUI-SC04-05 | 同一时间运行两个 TUI 会话 | 共享目录隔离；取消/清理互不干扰 |

## UI16 欢迎页 Changelog

入口：欢迎页 Changelog 菜单；与禁用斜杠入口分别验收。优先级：P1。执行通道：`C`。夹具：`welcome-release-notes`。

可达性：实际启动视口已显示；点击行为、合法 A3S 数据来源和能力合同尚未资格核实。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/welcome/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/welcome/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/router.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/router.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/event_loop.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/event_loop.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-shell-base/src/util/changelog.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-shell-base/src/util/changelog.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-UI16-01 | 预先固定合法 A3S 发布源和版本，有发布记录时从欢迎页打开 Changelog | 记录实际目标、版本和内容摘要；展示对应 A3S 记录；新增模型请求=0；无合法 A3S 数据源则正向 BLOCKED |
| TUI-UI16-02 | 缓存为空，合法发布源分别慢、断连或错误时点击欢迎页 Changelog | 有界失败并说明状态；不是无反馈死入口；输入与退出仍可用；不能按失败结果改选限制 profile |
| TUI-UI16-03 | 离线重启后从欢迎页打开已有合法缓存 | 来源、版本和离线状态准确；旧记录不冒充当前；缓存不被半写；新增模型请求=0 |
| TUI-UI16-04 | 合法发布数据包含长 Markdown、控制字符和外链时打开 | 渲染有界；无终端控制注入、自动打开、上传或凭证转发副作用；输入与关闭可用 |
| TUI-UI16-05 | 在首页菜单、已声明快捷键和斜杠发布记录入口间切换 | 各入口遵守运行前固定的能力合同；不可用入口不能靠另一入口越权；关闭后焦点恢复 |

