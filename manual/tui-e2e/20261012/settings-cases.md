# 逐项设置端到端用例

状态：**规划，未执行**。每行五条独立用例；共同前置、夹具和证据见 [计划](README.md)。机器可读版本见 [catalog.json](catalog.json)。

本文件 52 项，260 条。`C` 必须先证明产品可达，`G` 仅证明入口限制，不代表正向功能可用。

## ST01 设置：Compact mode

入口：/settings → Compact mode (compact_mode)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST01-01 | 开关后检查消息边距和可见行数 | 仅排版密度变化；内容、焦点和复制字节不变 |
| TUI-ST01-02 | 流式输出时切换 | 锚点稳定；无丢字和重复事件 |
| TUI-ST01-03 | 与 /compact-mode 双向切换 | 面板、状态和实际排版一致 |
| TUI-ST01-04 | 确认合法 compact_mode 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST01-05 | 预览新 compact_mode 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST02 设置：Default screen mode

入口：/settings → Default screen mode (screen_mode)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST02-01 | 保存 fullscreen/minimal 默认并重新启动 | 下次采用选定模式；当前 session 不被误重建 |
| TUI-ST02-02 | 用显式启动参数覆盖默认 | 参数优先；持久值不被静默改写 |
| TUI-ST02-03 | 未知值或存储失败 | 保持已确认模式；不能猜默认 |
| TUI-ST02-04 | 确认合法 screen_mode 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST02-05 | 预览新 screen_mode 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST03 设置：Show timestamps

入口：/settings → Show timestamps (show_timestamps)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST03-01 | 开关后检查用户与助手消息 | 真实时钟标签出现/消失；正文不变 |
| TUI-ST03-02 | 跨午夜/时区夹具观察 | 时间对应事件；不乱序 |
| TUI-ST03-03 | 用 /timestamps 双向切换 | 设置值与显示一致 |
| TUI-ST03-04 | 确认合法 show_timestamps 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST03-05 | 预览新 show_timestamps 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST04 设置：Timeline sidebar

入口：/settings → Timeline sidebar (show_timeline)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST04-01 | 开关时间线并定位历史轮 | 轨道与轮次映射准确 |
| TUI-ST04-02 | 空会话和超多轮次 | 空态明确、渲染有界 |
| TUI-ST04-03 | 用 /timeline 切换 | 面板值与轨道状态一致 |
| TUI-ST04-04 | 确认合法 show_timeline 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST04-05 | 预览新 show_timeline 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST05 设置：Dashboard preview

入口：/settings → Dashboard preview (dashboard_preview)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST05-01 | 首页和会话内浏览类别查找 dashboard_preview | 该设置行隐藏；不能暗示已支持 Dashboard preview |
| TUI-ST05-02 | 搜索完整 key 和展示名 Dashboard preview | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST05-03 | 从同组子项/旧滚动选中位置尝试进入 dashboard_preview | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST05-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST05-05 | 切屏、重连与 settings refresh 后重查 dashboard_preview | 仍隐藏；无宿主权限或账号请求变化 |

## ST06 设置：Snap prompt to top on send

入口：/settings → Snap prompt to top on send (page_flip_on_send)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST06-01 | 开启后发送 | 用户提示按规则对齐顶部 |
| TUI-ST06-02 | 关闭后发送 | 阅读位置按关闭规则保留 |
| TUI-ST06-03 | 阅读历史期间流式/发送 | 不抢焦点、不改变草稿 |
| TUI-ST06-04 | 确认合法 page_flip_on_send 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST06-05 | 预览新 page_flip_on_send 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST07 设置：Combine queued prompts

入口：/settings → Combine queued prompts (combine_queued_prompts)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST07-01 | 首页和会话内浏览类别查找 combine_queued_prompts | 该设置行隐藏；不能暗示已支持 Combine queued prompts |
| TUI-ST07-02 | 搜索完整 key 和展示名 Combine queued prompts | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST07-03 | 从同组子项/旧滚动选中位置尝试进入 combine_queued_prompts | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST07-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST07-05 | 切屏、重连与 settings refresh 后重查 combine_queued_prompts | 仍隐藏；无宿主权限或账号请求变化 |

## ST08 设置：Follow-up behavior

入口：/settings → Follow-up behavior (follow_up_behavior)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST08-01 | 首页和会话内浏览类别查找 follow_up_behavior | 该设置行隐藏；不能暗示已支持 Follow-up behavior |
| TUI-ST08-02 | 搜索完整 key 和展示名 Follow-up behavior | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST08-03 | 从同组子项/旧滚动选中位置尝试进入 follow_up_behavior | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST08-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST08-05 | 切屏、重连与 settings refresh 后重查 follow_up_behavior | 仍隐藏；无宿主权限或账号请求变化 |

## ST09 设置：Confirm before rewind

入口：/settings → Confirm before rewind (confirm_before_rewind)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST09-01 | 首页和会话内浏览类别查找 confirm_before_rewind | 该设置行隐藏；不能暗示已支持 Confirm before rewind |
| TUI-ST09-02 | 搜索完整 key 和展示名 Confirm before rewind | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST09-03 | 从同组子项/旧滚动选中位置尝试进入 confirm_before_rewind | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST09-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST09-05 | 切屏、重连与 settings refresh 后重查 confirm_before_rewind | 仍隐藏；无宿主权限或账号请求变化 |

## ST10 设置：Disable vim input mode

入口：/settings → Disable vim input mode (simple_mode)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST10-01 | 开启 plain 输入后输入 hjkl 和 Escape | 按声明普通编辑规则工作 |
| TUI-ST10-02 | 关闭后使用 Vim 输入状态 | 模式和按键语义一致；区别于 scrollback vim_mode |
| TUI-ST10-03 | 带多行/中文草稿切换 | 原字节和光标保留 |
| TUI-ST10-04 | 确认合法 simple_mode 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST10-05 | 预览新 simple_mode 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST11 设置：Vim scrollback navigation

入口：/settings → Vim scrollback navigation (vim_mode)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST11-01 | 开启后滚动区 j/k/gg/G | 导航正确 |
| TUI-ST11-02 | 关闭后按同键 | 不触发 Vim 滚动动作 |
| TUI-ST11-03 | 用 /vim-mode 切换并返回输入区 | 两入口一致；不改变输入编辑模式 |
| TUI-ST11-04 | 确认合法 vim_mode 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST11-05 | 预览新 vim_mode 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST12 设置：Theme

入口：/settings → Theme (theme)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST12-01 | 依次选已广告主题 | 全部界面配色更新且可读 |
| TUI-ST12-02 | 选 auto 后切系统明暗 | 自动主题按子项生效 |
| TUI-ST12-03 | 选未知值或取消预览 | 不更改已接受主题 |
| TUI-ST12-04 | 确认合法 theme 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST12-05 | 预览新 theme 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST13 设置：Auto dark theme

入口：/settings → Auto dark theme (auto_dark_theme)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST13-01 | theme=auto 时选两种 dark 子主题 | 暗模式跟随所选值 |
| TUI-ST13-02 | theme 非 auto 时修改 | 不误改变当前固定主题 |
| TUI-ST13-03 | 切亮再切暗 | 恢复配置的暗主题；无循环选择 auto |
| TUI-ST13-04 | 确认合法 auto_dark_theme 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST13-05 | 预览新 auto_dark_theme 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST14 设置：Auto light theme

入口：/settings → Auto light theme (auto_light_theme)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST14-01 | theme=auto 时选两种 light 子主题 | 亮模式跟随所选值 |
| TUI-ST14-02 | theme 非 auto 时修改 | 不误改变当前固定主题 |
| TUI-ST14-03 | 切暗再切亮 | 恢复配置的亮主题；无循环选择 auto |
| TUI-ST14-04 | 确认合法 auto_light_theme 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST14-05 | 预览新 auto_light_theme 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST15 设置：Render Mermaid diagrams

入口：/settings → Render Mermaid diagrams (render_mermaid)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST15-01 | 遍历 auto/on/off 并展示图 | 点击行/源码按各模式合同 |
| TUI-ST15-02 | 缺少外部打开能力 | 回退可读源码；不声称已打开 |
| TUI-ST15-03 | 非法图或超大图 | 有界错误；输入仍可使用 |
| TUI-ST15-04 | 确认合法 render_mermaid 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST15-05 | 预览新 render_mermaid 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST16 设置：Permission mode

入口：/settings → Permission mode (permission_mode)。优先级：P0。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST16-01 | 选择每个当前后端支持姿态 | ACP 确认值、状态栏和实际审批一致 |
| TUI-ST16-02 | 切 YOLO 后请求宿主禁止操作 | 仍然拒绝；无权限扩张 |
| TUI-ST16-03 | 取消预览/远端拒绝 | 不排空待审批队列；回滚到接受值 |
| TUI-ST16-04 | 确认合法 permission_mode 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST16-05 | 预览新 permission_mode 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST17 设置：Remember tool approvals

入口：/settings → Remember tool approvals (remember_tool_approvals)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST17-01 | 首页和会话内浏览类别查找 remember_tool_approvals | 该设置行隐藏；不能暗示已支持 Remember tool approvals |
| TUI-ST17-02 | 搜索完整 key 和展示名 Remember tool approvals | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST17-03 | 从同组子项/旧滚动选中位置尝试进入 remember_tool_approvals | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST17-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST17-05 | 切屏、重连与 settings refresh 后重查 remember_tool_approvals | 仍隐藏；无宿主权限或账号请求变化 |

## ST18 设置：Multiline

入口：/settings → Multiline (multiline_mode)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST18-01 | 开启后 Enter 换行、发送键提交 | 准确多行输入 |
| TUI-ST18-02 | 关闭后 Enter | 单次发送 |
| TUI-ST18-03 | 新会话重查 | 会话局部重置；不错误持久化 |
| TUI-ST18-04 | 确认合法 multiline_mode 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST18-05 | 预览新 multiline_mode 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST19 设置：Default model

入口：/settings → Default model (default_model)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST19-01 | 首页和会话内浏览类别查找 default_model | 该设置行隐藏；不能暗示已支持 Default model |
| TUI-ST19-02 | 搜索完整 key 和展示名 Default model | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST19-03 | 从同组子项/旧滚动选中位置尝试进入 default_model | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST19-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST19-05 | 切屏、重连与 settings refresh 后重查 default_model | 仍隐藏；无宿主权限或账号请求变化 |

## ST20 设置：Subagent model inheritance

入口：/settings → Subagent model inheritance (subagent_model_inheritance)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST20-01 | 首页和会话内浏览类别查找 subagent_model_inheritance | 该设置行隐藏；不能暗示已支持 Subagent model inheritance |
| TUI-ST20-02 | 搜索完整 key 和展示名 Subagent model inheritance | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST20-03 | 从同组子项/旧滚动选中位置尝试进入 subagent_model_inheritance | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST20-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST20-05 | 切屏、重连与 settings refresh 后重查 subagent_model_inheritance | 仍隐藏；无宿主权限或账号请求变化 |

## ST21 设置：Max thoughts width

入口：/settings → Max thoughts width (max_thoughts_width)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST21-01 | 设置 40、120、500 | 显示宽度预算符合值 |
| TUI-ST21-02 | 尝试 39、501 及非法数 | 按明确范围拒绝/钳制；不溢出 |
| TUI-ST21-03 | 流式和缩放时调宽 | 布局有界；未显示的正文不丢 |
| TUI-ST21-04 | 确认合法 max_thoughts_width 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST21-05 | 预览新 max_thoughts_width 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST22 设置：Show thinking blocks

入口：/settings → Show thinking blocks (show_thinking_blocks)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST22-01 | 开启真实思考块流 | 有该事件才显示；不制造思考文本 |
| TUI-ST22-02 | 关闭后继续流 | 隐藏思考块；最终正文保留 |
| TUI-ST22-03 | 切换与展开全部组合 | 已有思考可按合同查看；不会变成正文 |
| TUI-ST22-04 | 确认合法 show_thinking_blocks 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST22-05 | 预览新 show_thinking_blocks 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST23 设置：Prompt suggestions

入口：/settings → Prompt suggestions (prompt_suggestions)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST23-01 | 首页和会话内浏览类别查找 prompt_suggestions | 该设置行隐藏；不能暗示已支持 Prompt suggestions |
| TUI-ST23-02 | 搜索完整 key 和展示名 Prompt suggestions | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST23-03 | 从同组子项/旧滚动选中位置尝试进入 prompt_suggestions | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST23-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST23-05 | 切屏、重连与 settings refresh 后重查 prompt_suggestions | 仍隐藏；无宿主权限或账号请求变化 |

## ST24 设置：Respect manual folds

入口：/settings → Respect manual folds (respect_manual_folds)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST24-01 | 开启并手动折叠流式块 | 保持人工折叠 |
| TUI-ST24-02 | 关闭后继续流 | 按自动折叠合同更新 |
| TUI-ST24-03 | 切换、缩放和换会话 | 只影响正确视图；无工具重放 |
| TUI-ST24-04 | 确认合法 respect_manual_folds 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST24-05 | 预览新 respect_manual_folds 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST25 设置：Group tool calls

入口：/settings → Group tool calls (group_tool_verbs)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST25-01 | 开启并连续 read/search/ls | 分组有正确子项和计数 |
| TUI-ST25-02 | 关闭再查看 | 独立工具卡片身份完整 |
| TUI-ST25-03 | 同组一失败一成功 | 真实状态可见；不能组折叠隐藏失败 |
| TUI-ST25-04 | 确认合法 group_tool_verbs 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST25-05 | 预览新 group_tool_verbs 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST26 设置：Collapsed edit blocks

入口：/settings → Collapsed edit blocks (collapsed_edit_blocks)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST26-01 | 开启后实际 edit/patch | 一行 diffstat 与真实加减行相符 |
| TUI-ST26-02 | 关闭并展开 | 完整 diff 可读且可复制 |
| TUI-ST26-03 | 同文件多次编辑或不同文件 | 分组不混淆路径/摘要 |
| TUI-ST26-04 | 确认合法 collapsed_edit_blocks 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST26-05 | 预览新 collapsed_edit_blocks 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST27 设置：Match display refresh rate

入口：/settings → Match display refresh rate (display_refresh_auto_cadence)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST27-01 | 开启高刷新终端 | 刷新节拍受声明上限约束；输入响应正常 |
| TUI-ST27-02 | 关闭后流式滚动 | 采用固定节拍；输出内容相同 |
| TUI-ST27-03 | 无刷新率探测环境 | 有界默认；无忙循环 |
| TUI-ST27-04 | 确认合法 display_refresh_auto_cadence 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST27-05 | 预览新 display_refresh_auto_cadence 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST28 设置：Scroll speed

入口：/settings → Scroll speed (scroll_speed)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST28-01 | 设置 1 和 100 后固定滚轮输入 | 移动幅度符合乘数且有界 |
| TUI-ST28-02 | 尝试 0、101 | 范围明确；无溢出 |
| TUI-ST28-03 | 长列表和模态框滚动 | 只影响正确焦点 |
| TUI-ST28-04 | 确认合法 scroll_speed 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST28-05 | 预览新 scroll_speed 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST29 设置：Scroll input

入口：/settings → Scroll input (scroll_mode)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST29-01 | 遍历 auto/wheel/trackpad | 按实际选项解释输入 |
| TUI-ST29-02 | 混合滚轮/触控板连续事件 | 无方向反转或过量跳跃 |
| TUI-ST29-03 | 改变模式时保留选区 | 选区和消息锚点符合合同 |
| TUI-ST29-04 | 确认合法 scroll_mode 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST29-05 | 预览新 scroll_mode 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST30 设置：Scroll lines

入口：/settings → Scroll lines (scroll_lines)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST30-01 | 设置 1 与 10 并输入同一 tick | 移动行数精确 |
| TUI-ST30-02 | 尝试 0、11 | 范围校验有效 |
| TUI-ST30-03 | 与速度乘数组合 | 位移有界；无数值溢出 |
| TUI-ST30-04 | 确认合法 scroll_lines 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST30-05 | 预览新 scroll_lines 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST31 设置：Invert scroll

入口：/settings → Invert scroll (invert_scroll)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST31-01 | 开关后相同滚轮事件 | 移动方向相反且幅度相同 |
| TUI-ST31-02 | 滚到首尾继续 | 位置有界 |
| TUI-ST31-03 | 鼠标与键盘翻页组合 | 只改变声明的输入类型 |
| TUI-ST31-04 | 确认合法 invert_scroll 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST31-05 | 预览新 invert_scroll 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST32 设置：Text selection

入口：/settings → Text selection (keep_text_selection)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST32-01 | 遍历当前枚举选项 | 选区保留与双击语义符合各项 |
| TUI-ST32-02 | 新输出、焦点改变和滚动 | 保留/清除时机一致 |
| TUI-ST32-03 | 跨行中文选择后复制 | 字节精确；不带 ANSI |
| TUI-ST32-04 | 确认合法 keep_text_selection 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST32-05 | 预览新 keep_text_selection 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST33 设置：Coding data, retention, and training

入口：/settings → Coding data, retention, and training (coding_data_sharing)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST33-01 | 首页和会话内浏览类别查找 coding_data_sharing | 该设置行隐藏；不能暗示已支持 Coding data, retention, and training |
| TUI-ST33-02 | 搜索完整 key 和展示名 Coding data, retention, and training | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST33-03 | 从同组子项/旧滚动选中位置尝试进入 coding_data_sharing | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST33-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST33-05 | 切屏、重连与 settings refresh 后重查 coding_data_sharing | 仍隐藏；无宿主权限或账号请求变化 |

## ST34 设置：Default selected permission

入口：/settings → Default selected permission (default_selected_permission)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST34-01 | 首页和会话内浏览类别查找 default_selected_permission | 该设置行隐藏；不能暗示已支持 Default selected permission |
| TUI-ST34-02 | 搜索完整 key 和展示名 Default selected permission | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST34-03 | 从同组子项/旧滚动选中位置尝试进入 default_selected_permission | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST34-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST34-05 | 切屏、重连与 settings refresh 后重查 default_selected_permission | 仍隐藏；无宿主权限或账号请求变化 |

## ST35 设置：Ask-Question timeout

入口：/settings → Ask-Question timeout (toolset.ask_user_question.timeout_enabled)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST35-01 | 首页和会话内浏览类别查找 toolset.ask_user_question.timeout_enabled | 该设置行隐藏；不能暗示已支持 Ask-Question timeout |
| TUI-ST35-02 | 搜索完整 key 和展示名 Ask-Question timeout | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST35-03 | 从同组子项/旧滚动选中位置尝试进入 toolset.ask_user_question.timeout_enabled | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST35-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST35-05 | 切屏、重连与 settings refresh 后重查 toolset.ask_user_question.timeout_enabled | 仍隐藏；无宿主权限或账号请求变化 |

## ST36 设置：Plan mode

入口：/settings → Plan mode (plan_mode)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST36-01 | 首页和会话内浏览类别查找 plan_mode | 该设置行隐藏；不能暗示已支持 Plan mode |
| TUI-ST36-02 | 搜索完整 key 和展示名 Plan mode | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST36-03 | 从同组子项/旧滚动选中位置尝试进入 plan_mode | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST36-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST36-05 | 切屏、重连与 settings refresh 后重查 plan_mode | 仍隐藏；无宿主权限或账号请求变化 |

## ST37 设置：Show tips

入口：/settings → Show tips (show_tips)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST37-01 | 首页和会话内浏览类别查找 show_tips | 该设置行隐藏；不能暗示已支持 Show tips |
| TUI-ST37-02 | 搜索完整 key 和展示名 Show tips | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST37-03 | 从同组子项/旧滚动选中位置尝试进入 show_tips | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST37-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST37-05 | 切屏、重连与 settings refresh 后重查 show_tips | 仍隐藏；无宿主权限或账号请求变化 |

## ST38 设置：Show contextual hints

入口：/settings → Show contextual hints (contextual_hints)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST38-01 | 关闭总开关再触发所有子提示 | 全部提示隐藏 |
| TUI-ST38-02 | 开启总开关但关闭单项 | 只有该项不显示 |
| TUI-ST38-03 | 展开分组、搜索子项 | 结构正确；隐藏子项不复活 |
| TUI-ST38-04 | 确认合法 contextual_hints 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST38-05 | 预览新 contextual_hints 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST39 设置：Auto-update

入口：/settings → Auto-update (auto_update)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST39-01 | 首页和会话内浏览类别查找 auto_update | 该设置行隐藏；不能暗示已支持 Auto-update |
| TUI-ST39-02 | 搜索完整 key 和展示名 Auto-update | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST39-03 | 从同组子项/旧滚动选中位置尝试进入 auto_update | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST39-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST39-05 | 切屏、重连与 settings refresh 后重查 auto_update | 仍隐藏；无宿主权限或账号请求变化 |

## ST40 设置：Hunk tracker

入口：/settings → Hunk tracker (hunk_tracker_mode)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST40-01 | 首页和会话内浏览类别查找 hunk_tracker_mode | 该设置行隐藏；不能暗示已支持 Hunk tracker |
| TUI-ST40-02 | 搜索完整 key 和展示名 Hunk tracker | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST40-03 | 从同组子项/旧滚动选中位置尝试进入 hunk_tracker_mode | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST40-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST40-05 | 切屏、重连与 settings refresh 后重查 hunk_tracker_mode | 仍隐藏；无宿主权限或账号请求变化 |

## ST41 设置：Voice shortcut

入口：/settings → Voice shortcut (voice_keybind_enabled)。优先级：P0。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST41-01 | 关闭后按 Ctrl+Space/F8 | 零录音、零 STT |
| TUI-ST41-02 | 开启后在 Code 禁用语音环境按键 | 仍零录音；不得恢复服务 |
| TUI-ST41-03 | 快速开关后退出 | 无音频 worker 残留 |
| TUI-ST41-04 | 确认合法 voice_keybind_enabled 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST41-05 | 预览新 voice_keybind_enabled 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST42 设置：Voice capture

入口：/settings → Voice capture (voice_capture_mode)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST42-01 | 选 toggle/hold 后在禁用 Code 后端触发 | 仍不录音；设置不越过服务授权 |
| TUI-ST42-02 | 选择未知枚举 | 拒绝；原值不变 |
| TUI-ST42-03 | 模态框/首页重复按住释放 | 无假识别文本和后台请求 |
| TUI-ST42-04 | 确认合法 voice_capture_mode 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST42-05 | 预览新 voice_capture_mode 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST43 设置：Voice language

入口：/settings → Voice language (voice_stt_language)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST43-01 | 遍历 advertised 语言及 auto | 合法枚举显示一致；不因设置启动网络 |
| TUI-ST43-02 | 选择不支持语言 | 拒绝；不按字符串猜映射 |
| TUI-ST43-03 | Code 禁用语音时切语言并触发 | 仍无麦克风/STT；不可伪装识别成功 |
| TUI-ST43-04 | 确认合法 voice_stt_language 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST43-05 | 预览新 voice_stt_language 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST44 设置：Undo

入口：/settings → Undo (contextual_hints.undo)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST44-01 | 开启后清空草稿 | 出现正确撤销提示 |
| TUI-ST44-02 | 关闭后重复清空 | 不出现提示；Ctrl+Z 仍有效 |
| TUI-ST44-03 | 撤销提示可见时发送 | 提示不混入用户消息 |
| TUI-ST44-04 | 确认合法 contextual_hints.undo 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST44-05 | 预览新 contextual_hints.undo 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST45 设置：Plan mode

入口：/settings → Plan mode (contextual_hints.plan_mode)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST45-01 | 首页和会话内浏览类别查找 contextual_hints.plan_mode | 该设置行隐藏；不能暗示已支持 Plan mode |
| TUI-ST45-02 | 搜索完整 key 和展示名 Plan mode | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST45-03 | 从同组子项/旧滚动选中位置尝试进入 contextual_hints.plan_mode | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST45-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST45-05 | 切屏、重连与 settings refresh 后重查 contextual_hints.plan_mode | 仍隐藏；无宿主权限或账号请求变化 |

## ST46 设置：Image input

入口：/settings → Image input (contextual_hints.image_input)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST46-01 | 首页和会话内浏览类别查找 contextual_hints.image_input | 该设置行隐藏；不能暗示已支持 Image input |
| TUI-ST46-02 | 搜索完整 key 和展示名 Image input | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST46-03 | 从同组子项/旧滚动选中位置尝试进入 contextual_hints.image_input | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST46-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST46-05 | 切屏、重连与 settings refresh 后重查 contextual_hints.image_input | 仍隐藏；无宿主权限或账号请求变化 |

## ST47 设置：Send now

入口：/settings → Send now (contextual_hints.send_now)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST47-01 | 开启后运行中提交补充 | 仅在实际支持行为时出现正确提示 |
| TUI-ST47-02 | 关闭后相同操作 | 提示隐藏；提交能力门仍有效 |
| TUI-ST47-03 | 取消旧轮次后提示 | 及时清除；不误指向新轮次 |
| TUI-ST47-04 | 确认合法 contextual_hints.send_now 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST47-05 | 预览新 contextual_hints.send_now 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST48 设置：Small screen

入口：/settings → Small screen (contextual_hints.small_screen)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST48-01 | 开启后小窗口启动 | 提示 compact-mode 的触发次数符合合同 |
| TUI-ST48-02 | 关闭后同尺寸 | 不出现提示 |
| TUI-ST48-03 | 恢复尺寸并重新缩小 | 不重复刷屏；输入可用 |
| TUI-ST48-04 | 确认合法 contextual_hints.small_screen 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST48-05 | 预览新 contextual_hints.small_screen 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST49 设置：Word select

入口：/settings → Word select (contextual_hints.word_select)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST49-01 | 开启并双击可选择字词 | 提示对应真实选区语义 |
| TUI-ST49-02 | 关闭后双击 | 无提示；复制动作仍准确 |
| TUI-ST49-03 | 焦点不在会话文本双击 | 无错误提示和工具动作 |
| TUI-ST49-04 | 确认合法 contextual_hints.word_select 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST49-05 | 预览新 contextual_hints.word_select 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST50 设置：Copy and export

入口：/settings → Copy and export (contextual_hints.export_copy)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST50-01 | 开启并达到声明复制触发数 | 显示正确 export/copy 提示 |
| TUI-ST50-02 | 关闭后重复复制 | 不显示提示；剪贴板正确 |
| TUI-ST50-03 | 换会话后再复制 | 计数作用域符合合同；无跨会话文本 |
| TUI-ST50-04 | 确认合法 contextual_hints.export_copy 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST50-05 | 预览新 contextual_hints.export_copy 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST51 设置：SSH wrap

入口：/settings → SSH wrap (contextual_hints.ssh_wrap)。优先级：P1。执行通道：`U`。夹具：`settings-fixtures`。

可达性：当前可见设置；其效果必须实测，可能暴露无 backing 缺陷；语音项正向不代表可开启语音服务。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST51-01 | 开启 SSH 且无 wrap 环境 | 仅出现适用诊断提示 |
| TUI-ST51-02 | 关闭后同环境 | 无提示 |
| TUI-ST51-03 | 本地非 SSH 或已 wrap | 不误提示；不自动更改 shell |
| TUI-ST51-04 | 确认合法 contextual_hints.ssh_wrap 值后关闭面板并重启/新会话 | 持久值按元数据恢复；Pager-local/session-local 按合同重置；实际行为与显示一致 |
| TUI-ST51-05 | 预览新 contextual_hints.ssh_wrap 值后取消；另一次令持久化失败 | 旧已确认值恢复；ACL 未被半写；无无关设置或权限变化 |

## ST52 设置：Fork secondary model

入口：/settings → Fork secondary model (fork_secondary_model)。优先级：P1。执行通道：`G`。夹具：`settings-fixtures`。

可达性：CODE BACKEND HIDDEN：仅入口限制验收，不声称该设置功能通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/settings/defs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/settings_modal/state.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/settings/setters.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-ST52-01 | 首页和会话内浏览类别查找 fork_secondary_model | 该设置行隐藏；不能暗示已支持 Fork secondary model |
| TUI-ST52-02 | 搜索完整 key 和展示名 Fork secondary model | 搜索不能恢复隐藏项；无假可编辑结果 |
| TUI-ST52-03 | 从同组子项/旧滚动选中位置尝试进入 fork_secondary_model | 不能误改隐藏值或邻行；存储摘要不变 |
| TUI-ST52-04 | 在持久 ACL 带有该旧键的隔离 home 启动并打开设置 | 兼容数据不得放开 Code 不支持能力；旧值不能触发外部服务 |
| TUI-ST52-05 | 切屏、重连与 settings refresh 后重查 fork_secondary_model | 仍隐藏；无宿主权限或账号请求变化 |
