# 当前禁用命令的入口限制用例

状态：**规划，未执行**。每项至少五条独立用例；共同前置、夹具和证据见 [计划](README.md)。机器可读版本见 [catalog.json](catalog.json)。

本文件 42 项，215 条。`C` 必须先证明产品可达，`G` 仅证明入口限制，不代表正向功能可用。

## GT01 禁用命令：/usage

入口：手输 /usage；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/usage.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/usage.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT01-01 | 在首页、活跃会话和帮助中搜索 /usage 及全部别名 | 不提供可执行入口；不能打开账单或发送上游账户请求 |
| TUI-GT01-02 | 空闲时直接手输 /usage 并 Enter | 本地明确不可用；新增模型请求=0；不能打开账单或发送上游账户请求 |
| TUI-GT01-03 | 通过 /cost 尝试“查看用量或 manage” | 同样拒绝；不绕过 canonical gate；不能打开账单或发送上游账户请求 |
| TUI-GT01-04 | 真实轮次运行/审批等待时提交 /usage | 不能排入未来执行；原轮次归属和审批不变；不能打开账单或发送上游账户请求 |
| TUI-GT01-05 | 重连、切屏、新建会话和刷新目录后再次手输 /usage | 禁用门仍生效；无外部请求/进程/文件变化；不能打开账单或发送上游账户请求 |

## GT02 禁用命令：/view-plan

入口：手输 /view-plan；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/view_plan.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/view_plan.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT02-01 | 在首页、活跃会话和帮助中搜索 /view-plan 及全部别名 | 不提供可执行入口；不能打开上游套餐页面或请求套餐 API |
| TUI-GT02-02 | 空闲时直接手输 /view-plan 并 Enter | 本地明确不可用；新增模型请求=0；不能打开上游套餐页面或请求套餐 API |
| TUI-GT02-03 | 通过 /show-plan, /plan-view 尝试“查看套餐” | 同样拒绝；不绕过 canonical gate；不能打开上游套餐页面或请求套餐 API |
| TUI-GT02-04 | 真实轮次运行/审批等待时提交 /view-plan | 不能排入未来执行；原轮次归属和审批不变；不能打开上游套餐页面或请求套餐 API |
| TUI-GT02-05 | 重连、切屏、新建会话和刷新目录后再次手输 /view-plan | 禁用门仍生效；无外部请求/进程/文件变化；不能打开上游套餐页面或请求套餐 API |

## GT03 禁用命令：/plan

入口：手输 /plan；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/plan.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/plan.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT03-01 | 在首页、活跃会话和帮助中搜索 /plan 及全部别名 | 不提供可执行入口；不能误当作当前 Code 的 Shift+Tab 计划姿态 |
| TUI-GT03-02 | 空闲时直接手输 /plan 并 Enter | 本地明确不可用；新增模型请求=0；不能误当作当前 Code 的 Shift+Tab 计划姿态 |
| TUI-GT03-03 | 通过 规范命令带有效参数的调用 尝试“修改套餐” | 同样拒绝；不绕过 canonical gate；不能误当作当前 Code 的 Shift+Tab 计划姿态 |
| TUI-GT03-04 | 真实轮次运行/审批等待时提交 /plan | 不能排入未来执行；原轮次归属和审批不变；不能误当作当前 Code 的 Shift+Tab 计划姿态 |
| TUI-GT03-05 | 重连、切屏、新建会话和刷新目录后再次手输 /plan | 禁用门仍生效；无外部请求/进程/文件变化；不能误当作当前 Code 的 Shift+Tab 计划姿态 |

## GT04 禁用命令：/tutorial

入口：手输 /tutorial；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/tutorial.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/tutorial.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT04-01 | 在首页、活跃会话和帮助中搜索 /tutorial 及全部别名 | 不提供可执行入口；不能启动未支持的上游 onboarding |
| TUI-GT04-02 | 空闲时直接手输 /tutorial 并 Enter | 本地明确不可用；新增模型请求=0；不能启动未支持的上游 onboarding |
| TUI-GT04-03 | 通过 /tour, /onboarding 尝试“启动教程” | 同样拒绝；不绕过 canonical gate；不能启动未支持的上游 onboarding |
| TUI-GT04-04 | 真实轮次运行/审批等待时提交 /tutorial | 不能排入未来执行；原轮次归属和审批不变；不能启动未支持的上游 onboarding |
| TUI-GT04-05 | 重连、切屏、新建会话和刷新目录后再次手输 /tutorial | 禁用门仍生效；无外部请求/进程/文件变化；不能启动未支持的上游 onboarding |

## GT05 禁用命令：/docs

入口：手输 /docs；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/docs.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/docs.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT05-01 | 在首页、活跃会话和帮助中搜索 /docs 及全部别名 | 不提供可执行入口；不能把上游产品文档冒充 Code 可用功能 |
| TUI-GT05-02 | 空闲时直接手输 /docs 并 Enter | 本地明确不可用；新增模型请求=0；不能把上游产品文档冒充 Code 可用功能 |
| TUI-GT05-03 | 通过 /howto, /guides 尝试“打开文档” | 同样拒绝；不绕过 canonical gate；不能把上游产品文档冒充 Code 可用功能 |
| TUI-GT05-04 | 真实轮次运行/审批等待时提交 /docs | 不能排入未来执行；原轮次归属和审批不变；不能把上游产品文档冒充 Code 可用功能 |
| TUI-GT05-05 | 重连、切屏、新建会话和刷新目录后再次手输 /docs | 禁用门仍生效；无外部请求/进程/文件变化；不能把上游产品文档冒充 Code 可用功能 |

## GT06 禁用命令：/privacy

入口：手输 /privacy；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/privacy.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/privacy.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT06-01 | 在首页、活跃会话和帮助中搜索 /privacy 及全部别名 | 不提供可执行入口；不能写上游账号隐私设置 |
| TUI-GT06-02 | 空闲时直接手输 /privacy 并 Enter | 本地明确不可用；新增模型请求=0；不能写上游账号隐私设置 |
| TUI-GT06-03 | 通过 规范命令带有效参数的调用 尝试“隐私控制” | 同样拒绝；不绕过 canonical gate；不能写上游账号隐私设置 |
| TUI-GT06-04 | 真实轮次运行/审批等待时提交 /privacy | 不能排入未来执行；原轮次归属和审批不变；不能写上游账号隐私设置 |
| TUI-GT06-05 | 重连、切屏、新建会话和刷新目录后再次手输 /privacy | 禁用门仍生效；无外部请求/进程/文件变化；不能写上游账号隐私设置 |

## GT07 禁用命令：/login

入口：手输 /login；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/login.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/login.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT07-01 | 在首页、活跃会话和帮助中搜索 /login 及全部别名 | 不提供可执行入口；不能索取账号凭证或启动上游授权 |
| TUI-GT07-02 | 空闲时直接手输 /login 并 Enter | 本地明确不可用；新增模型请求=0；不能索取账号凭证或启动上游授权 |
| TUI-GT07-03 | 通过 规范命令带有效参数的调用 尝试“登录” | 同样拒绝；不绕过 canonical gate；不能索取账号凭证或启动上游授权 |
| TUI-GT07-04 | 真实轮次运行/审批等待时提交 /login | 不能排入未来执行；原轮次归属和审批不变；不能索取账号凭证或启动上游授权 |
| TUI-GT07-05 | 重连、切屏、新建会话和刷新目录后再次手输 /login | 禁用门仍生效；无外部请求/进程/文件变化；不能索取账号凭证或启动上游授权 |

## GT08 禁用命令：/logout

入口：手输 /logout；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/logout.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/logout.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT08-01 | 在首页、活跃会话和帮助中搜索 /logout 及全部别名 | 不提供可执行入口；不能清除宿主 provider 凭证 |
| TUI-GT08-02 | 空闲时直接手输 /logout 并 Enter | 本地明确不可用；新增模型请求=0；不能清除宿主 provider 凭证 |
| TUI-GT08-03 | 通过 规范命令带有效参数的调用 尝试“登出” | 同样拒绝；不绕过 canonical gate；不能清除宿主 provider 凭证 |
| TUI-GT08-04 | 真实轮次运行/审批等待时提交 /logout | 不能排入未来执行；原轮次归属和审批不变；不能清除宿主 provider 凭证 |
| TUI-GT08-05 | 重连、切屏、新建会话和刷新目录后再次手输 /logout | 禁用门仍生效；无外部请求/进程/文件变化；不能清除宿主 provider 凭证 |

## GT09 禁用命令：/btw

入口：手输 /btw；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/btw.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/btw.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT09-01 | 在首页、活跃会话和帮助中搜索 /btw 及全部别名 | 不提供可执行入口；不能静默创建未支持旁路轮次 |
| TUI-GT09-02 | 空闲时直接手输 /btw 并 Enter | 本地明确不可用；新增模型请求=0；不能静默创建未支持旁路轮次 |
| TUI-GT09-03 | 通过 规范命令带有效参数的调用 尝试“旁路提问” | 同样拒绝；不绕过 canonical gate；不能静默创建未支持旁路轮次 |
| TUI-GT09-04 | 真实轮次运行/审批等待时提交 /btw | 不能排入未来执行；原轮次归属和审批不变；不能静默创建未支持旁路轮次 |
| TUI-GT09-05 | 重连、切屏、新建会话和刷新目录后再次手输 /btw | 禁用门仍生效；无外部请求/进程/文件变化；不能静默创建未支持旁路轮次 |

## GT10 禁用命令：/feedback

入口：手输 /feedback；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/feedback.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/feedback.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT10-01 | 在首页、活跃会话和帮助中搜索 /feedback 及全部别名 | 不提供可执行入口；不能外传用户会话或 trace |
| TUI-GT10-02 | 空闲时直接手输 /feedback 并 Enter | 本地明确不可用；新增模型请求=0；不能外传用户会话或 trace |
| TUI-GT10-03 | 通过 规范命令带有效参数的调用 尝试“发送反馈” | 同样拒绝；不绕过 canonical gate；不能外传用户会话或 trace |
| TUI-GT10-04 | 真实轮次运行/审批等待时提交 /feedback | 不能排入未来执行；原轮次归属和审批不变；不能外传用户会话或 trace |
| TUI-GT10-05 | 重连、切屏、新建会话和刷新目录后再次手输 /feedback | 禁用门仍生效；无外部请求/进程/文件变化；不能外传用户会话或 trace |

## GT11 禁用命令：/queue

入口：手输 /queue；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/queue.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/queue.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

动作参数化：`ToggleQueue`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT11-01 | 在首页、活跃会话和帮助中搜索 /queue 及全部别名 | 不提供可执行入口；不能调用无 backing 扩展后假成功 |
| TUI-GT11-02 | 空闲时直接手输 /queue 并 Enter | 本地明确不可用；新增模型请求=0；不能调用无 backing 扩展后假成功 |
| TUI-GT11-03 | 通过 规范命令带有效参数的调用 尝试“管理队列”；同时触发本项 action_ids 列出的实际快捷键，验证隐藏命令没有被键盘旁路 | 同样拒绝；不绕过 canonical gate；不能调用无 backing 扩展后假成功 |
| TUI-GT11-04 | 真实轮次运行/审批等待时提交 /queue | 不能排入未来执行；原轮次归属和审批不变；不能调用无 backing 扩展后假成功 |
| TUI-GT11-05 | 重连、切屏、新建会话和刷新目录后再次手输 /queue | 禁用门仍生效；无外部请求/进程/文件变化；不能调用无 backing 扩展后假成功 |

## GT12 禁用命令：/fork

入口：手输 /fork；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/fork.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/fork.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT12-01 | 在首页、活跃会话和帮助中搜索 /fork 及全部别名 | 不提供可执行入口；不能创建无证据的会话分支 |
| TUI-GT12-02 | 空闲时直接手输 /fork 并 Enter | 本地明确不可用；新增模型请求=0；不能创建无证据的会话分支 |
| TUI-GT12-03 | 通过 规范命令带有效参数的调用 尝试“分叉会话” | 同样拒绝；不绕过 canonical gate；不能创建无证据的会话分支 |
| TUI-GT12-04 | 真实轮次运行/审批等待时提交 /fork | 不能排入未来执行；原轮次归属和审批不变；不能创建无证据的会话分支 |
| TUI-GT12-05 | 重连、切屏、新建会话和刷新目录后再次手输 /fork | 禁用门仍生效；无外部请求/进程/文件变化；不能创建无证据的会话分支 |

## GT13 禁用命令：/resume

入口：手输 /resume；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/resume.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/resume.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT13-01 | 在首页、活跃会话和帮助中搜索 /resume 及全部别名 | 不提供可执行入口；不能把 slash 拒绝与启动 --resume 支持混淆 |
| TUI-GT13-02 | 空闲时直接手输 /resume 并 Enter | 本地明确不可用；新增模型请求=0；不能把 slash 拒绝与启动 --resume 支持混淆 |
| TUI-GT13-03 | 通过 规范命令带有效参数的调用 尝试“恢复会话” | 同样拒绝；不绕过 canonical gate；不能把 slash 拒绝与启动 --resume 支持混淆 |
| TUI-GT13-04 | 真实轮次运行/审批等待时提交 /resume | 不能排入未来执行；原轮次归属和审批不变；不能把 slash 拒绝与启动 --resume 支持混淆 |
| TUI-GT13-05 | 重连、切屏、新建会话和刷新目录后再次手输 /resume | 禁用门仍生效；无外部请求/进程/文件变化；不能把 slash 拒绝与启动 --resume 支持混淆 |

## GT14 禁用命令：/rename

入口：手输 /rename；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/rename.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/rename.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT14-01 | 在首页、活跃会话和帮助中搜索 /rename 及全部别名 | 不提供可执行入口；不能只改屏幕标题而假称持久化成功 |
| TUI-GT14-02 | 空闲时直接手输 /rename 并 Enter | 本地明确不可用；新增模型请求=0；不能只改屏幕标题而假称持久化成功 |
| TUI-GT14-03 | 通过 /title 尝试“重命名” | 同样拒绝；不绕过 canonical gate；不能只改屏幕标题而假称持久化成功 |
| TUI-GT14-04 | 真实轮次运行/审批等待时提交 /rename | 不能排入未来执行；原轮次归属和审批不变；不能只改屏幕标题而假称持久化成功 |
| TUI-GT14-05 | 重连、切屏、新建会话和刷新目录后再次手输 /rename | 禁用门仍生效；无外部请求/进程/文件变化；不能只改屏幕标题而假称持久化成功 |

## GT15 禁用命令：/history

入口：手输 /history；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/history.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/history.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT15-01 | 在首页、活跃会话和帮助中搜索 /history 及全部别名 | 不提供可执行入口；不能绕到其他产品历史存储 |
| TUI-GT15-02 | 空闲时直接手输 /history 并 Enter | 本地明确不可用；新增模型请求=0；不能绕到其他产品历史存储 |
| TUI-GT15-03 | 通过 规范命令带有效参数的调用 尝试“查看上游历史” | 同样拒绝；不绕过 canonical gate；不能绕到其他产品历史存储 |
| TUI-GT15-04 | 真实轮次运行/审批等待时提交 /history | 不能排入未来执行；原轮次归属和审批不变；不能绕到其他产品历史存储 |
| TUI-GT15-05 | 重连、切屏、新建会话和刷新目录后再次手输 /history | 禁用门仍生效；无外部请求/进程/文件变化；不能绕到其他产品历史存储 |

## GT16 禁用命令：/transcript

入口：手输 /transcript；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/transcript.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/transcript.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT16-01 | 在首页、活跃会话和帮助中搜索 /transcript 及全部别名 | 不提供可执行入口；不能把本地 /export 与缺失扩展混淆 |
| TUI-GT16-02 | 空闲时直接手输 /transcript 并 Enter | 本地明确不可用；新增模型请求=0；不能把本地 /export 与缺失扩展混淆 |
| TUI-GT16-03 | 通过 /log 尝试“打开上游转录” | 同样拒绝；不绕过 canonical gate；不能把本地 /export 与缺失扩展混淆 |
| TUI-GT16-04 | 真实轮次运行/审批等待时提交 /transcript | 不能排入未来执行；原轮次归属和审批不变；不能把本地 /export 与缺失扩展混淆 |
| TUI-GT16-05 | 重连、切屏、新建会话和刷新目录后再次手输 /transcript | 禁用门仍生效；无外部请求/进程/文件变化；不能把本地 /export 与缺失扩展混淆 |

## GT17 禁用命令：/rewind

入口：手输 /rewind；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/rewind.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/rewind.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

动作参数化：`Rewind`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT17-01 | 在首页、活跃会话和帮助中搜索 /rewind 及全部别名 | 不提供可执行入口；不能删除历史、撤销文件或清空效果义务 |
| TUI-GT17-02 | 空闲时直接手输 /rewind 并 Enter | 本地明确不可用；新增模型请求=0；不能删除历史、撤销文件或清空效果义务 |
| TUI-GT17-03 | 通过 /undo 尝试“撤销会话”；同时触发本项 action_ids 列出的实际快捷键，验证隐藏命令没有被键盘旁路 | 同样拒绝；不绕过 canonical gate；不能删除历史、撤销文件或清空效果义务 |
| TUI-GT17-04 | 真实轮次运行/审批等待时提交 /rewind | 不能排入未来执行；原轮次归属和审批不变；不能删除历史、撤销文件或清空效果义务 |
| TUI-GT17-05 | 重连、切屏、新建会话和刷新目录后再次手输 /rewind | 禁用门仍生效；无外部请求/进程/文件变化；不能删除历史、撤销文件或清空效果义务 |

## GT18 禁用命令：/workflows

入口：手输 /workflows；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/workflows.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/workflows.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT18-01 | 在首页、活跃会话和帮助中搜索 /workflows 及全部别名 | 不提供可执行入口；不能虚构已安装或可执行流程 |
| TUI-GT18-02 | 空闲时直接手输 /workflows 并 Enter | 本地明确不可用；新增模型请求=0；不能虚构已安装或可执行流程 |
| TUI-GT18-03 | 通过 规范命令带有效参数的调用 尝试“列出工作流” | 同样拒绝；不绕过 canonical gate；不能虚构已安装或可执行流程 |
| TUI-GT18-04 | 真实轮次运行/审批等待时提交 /workflows | 不能排入未来执行；原轮次归属和审批不变；不能虚构已安装或可执行流程 |
| TUI-GT18-05 | 重连、切屏、新建会话和刷新目录后再次手输 /workflows | 禁用门仍生效；无外部请求/进程/文件变化；不能虚构已安装或可执行流程 |

## GT19 禁用命令：/workflow

入口：手输 /workflow；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/workflow.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/workflow.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT19-01 | 在首页、活跃会话和帮助中搜索 /workflow 及全部别名 | 不提供可执行入口；不能启动缺失的 shell workflow 扩展 |
| TUI-GT19-02 | 空闲时直接手输 /workflow 并 Enter | 本地明确不可用；新增模型请求=0；不能启动缺失的 shell workflow 扩展 |
| TUI-GT19-03 | 通过 规范命令带有效参数的调用 尝试“运行保存流程” | 同样拒绝；不绕过 canonical gate；不能启动缺失的 shell workflow 扩展 |
| TUI-GT19-04 | 真实轮次运行/审批等待时提交 /workflow | 不能排入未来执行；原轮次归属和审批不变；不能启动缺失的 shell workflow 扩展 |
| TUI-GT19-05 | 重连、切屏、新建会话和刷新目录后再次手输 /workflow | 禁用门仍生效；无外部请求/进程/文件变化；不能启动缺失的 shell workflow 扩展 |

## GT20 禁用命令：/personas

入口：手输 /personas；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/personas.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/personas.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT20-01 | 在首页、活跃会话和帮助中搜索 /personas 及全部别名 | 不提供可执行入口；不能从上游 bundle 加载未授权人格 |
| TUI-GT20-02 | 空闲时直接手输 /personas 并 Enter | 本地明确不可用；新增模型请求=0；不能从上游 bundle 加载未授权人格 |
| TUI-GT20-03 | 通过 规范命令带有效参数的调用 尝试“选择角色” | 同样拒绝；不绕过 canonical gate；不能从上游 bundle 加载未授权人格 |
| TUI-GT20-04 | 真实轮次运行/审批等待时提交 /personas | 不能排入未来执行；原轮次归属和审批不变；不能从上游 bundle 加载未授权人格 |
| TUI-GT20-05 | 重连、切屏、新建会话和刷新目录后再次手输 /personas | 禁用门仍生效；无外部请求/进程/文件变化；不能从上游 bundle 加载未授权人格 |

## GT21 禁用命令：/config-agents

入口：手输 /config-agents；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/config_agents.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/config_agents.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT21-01 | 在首页、活跃会话和帮助中搜索 /config-agents 及全部别名 | 不提供可执行入口；不能改变未支持的上游代理设置 |
| TUI-GT21-02 | 空闲时直接手输 /config-agents 并 Enter | 本地明确不可用；新增模型请求=0；不能改变未支持的上游代理设置 |
| TUI-GT21-03 | 通过 /agents 尝试“配置代理” | 同样拒绝；不绕过 canonical gate；不能改变未支持的上游代理设置 |
| TUI-GT21-04 | 真实轮次运行/审批等待时提交 /config-agents | 不能排入未来执行；原轮次归属和审批不变；不能改变未支持的上游代理设置 |
| TUI-GT21-05 | 重连、切屏、新建会话和刷新目录后再次手输 /config-agents | 禁用门仍生效；无外部请求/进程/文件变化；不能改变未支持的上游代理设置 |

## GT22 禁用命令：/plugins

入口：手输 /plugins；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/plugin.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/plugin.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

动作参数化：`OpenExtensions`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT22-01 | 在首页、活跃会话和帮助中搜索 /plugins 及全部别名 | 不提供可执行入口；不能自动安装、启动或信任未授权插件 |
| TUI-GT22-02 | 空闲时直接手输 /plugins 并 Enter | 本地明确不可用；新增模型请求=0；不能自动安装、启动或信任未授权插件 |
| TUI-GT22-03 | 通过 /plugin 尝试“管理插件”；同时触发本项 action_ids 列出的实际快捷键，验证隐藏命令没有被键盘旁路 | 同样拒绝；不绕过 canonical gate；不能自动安装、启动或信任未授权插件 |
| TUI-GT22-04 | 真实轮次运行/审批等待时提交 /plugins | 不能排入未来执行；原轮次归属和审批不变；不能自动安装、启动或信任未授权插件 |
| TUI-GT22-05 | 重连、切屏、新建会话和刷新目录后再次手输 /plugins | 禁用门仍生效；无外部请求/进程/文件变化；不能自动安装、启动或信任未授权插件 |

## GT23 禁用命令：/skills

入口：手输 /skills；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/plugin.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/plugin.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

动作参数化：`OpenExtensions`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT23-01 | 在首页、活跃会话和帮助中搜索 /skills 及全部别名 | 不提供可执行入口；不能把隐藏管理面板等同于模型 search_skills/Skill 不可用 |
| TUI-GT23-02 | 空闲时直接手输 /skills 并 Enter | 本地明确不可用；新增模型请求=0；不能把隐藏管理面板等同于模型 search_skills/Skill 不可用 |
| TUI-GT23-03 | 通过 规范命令带有效参数的调用 尝试“管理技能” | 同样拒绝；不绕过 canonical gate；不能把隐藏管理面板等同于模型 search_skills/Skill 不可用 |
| TUI-GT23-04 | 真实轮次运行/审批等待时提交 /skills | 不能排入未来执行；原轮次归属和审批不变；不能把隐藏管理面板等同于模型 search_skills/Skill 不可用 |
| TUI-GT23-05 | 重连、切屏、新建会话和刷新目录后再次手输 /skills | 禁用门仍生效；无外部请求/进程/文件变化；不能把隐藏管理面板等同于模型 search_skills/Skill 不可用 |

## GT24 禁用命令：/mcps

入口：手输 /mcps；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mcps.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/mcps.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

动作参数化：`OpenExtensions`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT24-01 | 在首页、活跃会话和帮助中搜索 /mcps 及全部别名 | 不提供可执行入口；不能把隐藏管理面板等同于宿主预配置 MCP 工具不可用 |
| TUI-GT24-02 | 空闲时直接手输 /mcps 并 Enter | 本地明确不可用；新增模型请求=0；不能把隐藏管理面板等同于宿主预配置 MCP 工具不可用 |
| TUI-GT24-03 | 通过 规范命令带有效参数的调用 尝试“管理 MCP” | 同样拒绝；不绕过 canonical gate；不能把隐藏管理面板等同于宿主预配置 MCP 工具不可用 |
| TUI-GT24-04 | 真实轮次运行/审批等待时提交 /mcps | 不能排入未来执行；原轮次归属和审批不变；不能把隐藏管理面板等同于宿主预配置 MCP 工具不可用 |
| TUI-GT24-05 | 重连、切屏、新建会话和刷新目录后再次手输 /mcps | 禁用门仍生效；无外部请求/进程/文件变化；不能把隐藏管理面板等同于宿主预配置 MCP 工具不可用 |

## GT25 禁用命令：/hooks

入口：手输 /hooks；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/plugin.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/plugin.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

动作参数化：`OpenExtensions`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT25-01 | 在首页、活跃会话和帮助中搜索 /hooks 及全部别名 | 不提供可执行入口；不能创建/授信/执行未授权 hook |
| TUI-GT25-02 | 空闲时直接手输 /hooks 并 Enter | 本地明确不可用；新增模型请求=0；不能创建/授信/执行未授权 hook |
| TUI-GT25-03 | 通过 规范命令带有效参数的调用 尝试“管理 hooks” | 同样拒绝；不绕过 canonical gate；不能创建/授信/执行未授权 hook |
| TUI-GT25-04 | 真实轮次运行/审批等待时提交 /hooks | 不能排入未来执行；原轮次归属和审批不变；不能创建/授信/执行未授权 hook |
| TUI-GT25-05 | 重连、切屏、新建会话和刷新目录后再次手输 /hooks | 禁用门仍生效；无外部请求/进程/文件变化；不能创建/授信/执行未授权 hook |

## GT26 禁用命令：/marketplace

入口：手输 /marketplace；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/plugin.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/plugin.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT26-01 | 在首页、活跃会话和帮助中搜索 /marketplace 及全部别名 | 不提供可执行入口；不能发外部市场请求或下载包 |
| TUI-GT26-02 | 空闲时直接手输 /marketplace 并 Enter | 本地明确不可用；新增模型请求=0；不能发外部市场请求或下载包 |
| TUI-GT26-03 | 通过 规范命令带有效参数的调用 尝试“插件市场” | 同样拒绝；不绕过 canonical gate；不能发外部市场请求或下载包 |
| TUI-GT26-04 | 真实轮次运行/审批等待时提交 /marketplace | 不能排入未来执行；原轮次归属和审批不变；不能发外部市场请求或下载包 |
| TUI-GT26-05 | 重连、切屏、新建会话和刷新目录后再次手输 /marketplace | 禁用门仍生效；无外部请求/进程/文件变化；不能发外部市场请求或下载包 |

## GT27 禁用命令：/import-claude

入口：手输 /import-claude；补全/帮助/别名；欢迎页条件导入菜单和对应快捷键。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/import_claude.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/import_claude.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/welcome/mod.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/views/welcome/mod.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/router.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/router.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/import_claude.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/dispatch/import_claude.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT27-01 | 在首页、活跃会话和帮助中搜索 /import-claude 及全部别名 | 不提供可执行入口；不能读取未授权第三方会话目录 |
| TUI-GT27-02 | 空闲时直接手输 /import-claude 并 Enter | 本地明确不可用；新增模型请求=0；不能读取未授权第三方会话目录 |
| TUI-GT27-03 | 通过 规范命令带有效参数的调用 尝试“导入旧工具设置” | 同样拒绝；不绕过 canonical gate；不能读取未授权第三方会话目录 |
| TUI-GT27-04 | 真实轮次运行/审批等待时提交 /import-claude | 不能排入未来执行；原轮次归属和审批不变；不能读取未授权第三方会话目录 |
| TUI-GT27-05 | 重连、切屏、新建会话和刷新目录后再次手输 /import-claude | 禁用门仍生效；无外部请求/进程/文件变化；不能读取未授权第三方会话目录 |
| TUI-GT27-06 | 独占 HOME/工作区仅准备旧工具空目录和 ACL canary 后启动 | Code 不广告未支持导入；初始 ACL 与权限摘要不变；未支持配置不被创建或解析 |
| TUI-GT27-07 | 首页尝试源码已声明的导入快捷键，包括菜单隐藏时 | 明确限制或无允许动作；不扫描、写入或转换未支持配置；新增模型请求=0 |
| TUI-GT27-08 | 若真实菜单仍出现，记录出现条件并通过菜单尝试打开 | 先证明可达，再核对限制；无后端门旁路或假导入标记；入口不可达不得计为已测正向 |
| TUI-GT27-09 | 取消、关闭或重复触发真实可达的导入入口 | 无配置/权限半写、重复提示或新 ACP 会话；只允许 UI 状态恢复；不可达参数保留资格说明 |
| TUI-GT27-10 | 触发导入限制后，用真实 LLM 读取新生成的随机文件 | 新轮正常且引用未放入提示的随机内容；无 Unknown 债务；模型、凭证、权限 ACL 和外部 canary 不变 |

## GT28 禁用命令：/remember

入口：手输 /remember；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/remember.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/remember.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT28-01 | 在首页、活跃会话和帮助中搜索 /remember 及全部别名 | 不提供可执行入口；不能伪装已保存偏好；自然语言记忆另有条件用例 |
| TUI-GT28-02 | 空闲时直接手输 /remember 并 Enter | 本地明确不可用；新增模型请求=0；不能伪装已保存偏好；自然语言记忆另有条件用例 |
| TUI-GT28-03 | 通过 规范命令带有效参数的调用 尝试“上游记忆模式” | 同样拒绝；不绕过 canonical gate；不能伪装已保存偏好；自然语言记忆另有条件用例 |
| TUI-GT28-04 | 真实轮次运行/审批等待时提交 /remember | 不能排入未来执行；原轮次归属和审批不变；不能伪装已保存偏好；自然语言记忆另有条件用例 |
| TUI-GT28-05 | 重连、切屏、新建会话和刷新目录后再次手输 /remember | 禁用门仍生效；无外部请求/进程/文件变化；不能伪装已保存偏好；自然语言记忆另有条件用例 |

## GT29 禁用命令：/tasks

入口：手输 /tasks；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/tasks.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/tasks.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

动作参数化：`ToggleTasks`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT29-01 | 在首页、活跃会话和帮助中搜索 /tasks 及全部别名 | 不提供可执行入口；不能让 Ctrl+G 或其他入口绕过未支持任务 UI |
| TUI-GT29-02 | 空闲时直接手输 /tasks 并 Enter | 本地明确不可用；新增模型请求=0；不能让 Ctrl+G 或其他入口绕过未支持任务 UI |
| TUI-GT29-03 | 通过 规范命令带有效参数的调用 尝试“任务管理面板”；同时触发本项 action_ids 列出的实际快捷键，验证隐藏命令没有被键盘旁路 | 同样拒绝；不绕过 canonical gate；不能让 Ctrl+G 或其他入口绕过未支持任务 UI |
| TUI-GT29-04 | 真实轮次运行/审批等待时提交 /tasks | 不能排入未来执行；原轮次归属和审批不变；不能让 Ctrl+G 或其他入口绕过未支持任务 UI |
| TUI-GT29-05 | 重连、切屏、新建会话和刷新目录后再次手输 /tasks | 禁用门仍生效；无外部请求/进程/文件变化；不能让 Ctrl+G 或其他入口绕过未支持任务 UI |

## GT30 禁用命令：/imagine

入口：手输 /imagine；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/imagine.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/imagine.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT30-01 | 在首页、活跃会话和帮助中搜索 /imagine 及全部别名 | 不提供可执行入口；不能调用 image_gen 或发送媒体费用请求 |
| TUI-GT30-02 | 空闲时直接手输 /imagine 并 Enter | 本地明确不可用；新增模型请求=0；不能调用 image_gen 或发送媒体费用请求 |
| TUI-GT30-03 | 通过 规范命令带有效参数的调用 尝试“图片生成” | 同样拒绝；不绕过 canonical gate；不能调用 image_gen 或发送媒体费用请求 |
| TUI-GT30-04 | 真实轮次运行/审批等待时提交 /imagine | 不能排入未来执行；原轮次归属和审批不变；不能调用 image_gen 或发送媒体费用请求 |
| TUI-GT30-05 | 重连、切屏、新建会话和刷新目录后再次手输 /imagine | 禁用门仍生效；无外部请求/进程/文件变化；不能调用 image_gen 或发送媒体费用请求 |

## GT31 禁用命令：/imagine-video

入口：手输 /imagine-video；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/imagine_video.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/imagine_video.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT31-01 | 在首页、活跃会话和帮助中搜索 /imagine-video 及全部别名 | 不提供可执行入口；不能调用 image_to_video 或发送媒体费用请求 |
| TUI-GT31-02 | 空闲时直接手输 /imagine-video 并 Enter | 本地明确不可用；新增模型请求=0；不能调用 image_to_video 或发送媒体费用请求 |
| TUI-GT31-03 | 通过 规范命令带有效参数的调用 尝试“视频生成” | 同样拒绝；不绕过 canonical gate；不能调用 image_to_video 或发送媒体费用请求 |
| TUI-GT31-04 | 真实轮次运行/审批等待时提交 /imagine-video | 不能排入未来执行；原轮次归属和审批不变；不能调用 image_to_video 或发送媒体费用请求 |
| TUI-GT31-05 | 重连、切屏、新建会话和刷新目录后再次手输 /imagine-video | 禁用门仍生效；无外部请求/进程/文件变化；不能调用 image_to_video 或发送媒体费用请求 |

## GT32 禁用命令：/voice

入口：手输 /voice；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/voice.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/voice.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT32-01 | 在首页、活跃会话和帮助中搜索 /voice 及全部别名 | 不提供可执行入口；不能开启麦克风或 STT 请求 |
| TUI-GT32-02 | 空闲时直接手输 /voice 并 Enter | 本地明确不可用；新增模型请求=0；不能开启麦克风或 STT 请求 |
| TUI-GT32-03 | 通过 规范命令带有效参数的调用 尝试“语音” | 同样拒绝；不绕过 canonical gate；不能开启麦克风或 STT 请求 |
| TUI-GT32-04 | 真实轮次运行/审批等待时提交 /voice | 不能排入未来执行；原轮次归属和审批不变；不能开启麦克风或 STT 请求 |
| TUI-GT32-05 | 重连、切屏、新建会话和刷新目录后再次手输 /voice | 禁用门仍生效；无外部请求/进程/文件变化；不能开启麦克风或 STT 请求 |

## GT33 禁用命令：/dashboard

入口：手输 /dashboard；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/dashboard.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/dashboard.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

动作参数化：`OpenDashboard`、`DashboardSelectNext`、`DashboardSelectPrev`、`DashboardTogglePin`、`DashboardBeginRename`、`DashboardStop`、`DashboardCycleMode`、`DashboardToggleGrouping`、`DashboardReorderUp`、`DashboardReorderDown`、`DashboardShortcutsHelp`、`DashboardExit`、`DashboardToggleAutoApprove`、`DashboardOpenLocationPicker`、`DashboardToggleWorktree`、`DashboardOverlayExit`、`DashboardOverlayPrev`、`DashboardOverlayNext`、`DashboardOverlayStop`。每个当前可触发动作都要运行对应场景；不可达动作要记录原因。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT33-01 | 在首页、活跃会话和帮助中搜索 /dashboard 及全部别名 | 不提供可执行入口；不能让 Ctrl+反斜杠或 /sessions 别名绕过限制 |
| TUI-GT33-02 | 空闲时直接手输 /dashboard 并 Enter | 本地明确不可用；新增模型请求=0；不能让 Ctrl+反斜杠或 /sessions 别名绕过限制 |
| TUI-GT33-03 | 通过 /agents-dashboard, /sessions 尝试“多代理仪表盘”；在实际首页/会话上下文尝试 Ctrl+反斜杠；其余 Dashboard* 动作仅在真实可达上下文验证，当前不可达记 N/A(reason)，不注入伪造仪表盘 | 同样拒绝；不绕过 canonical gate；不能让 Ctrl+反斜杠或 /sessions 别名绕过限制 |
| TUI-GT33-04 | 真实轮次运行/审批等待时提交 /dashboard | 不能排入未来执行；原轮次归属和审批不变；不能让 Ctrl+反斜杠或 /sessions 别名绕过限制 |
| TUI-GT33-05 | 重连、切屏、新建会话和刷新目录后再次手输 /dashboard | 禁用门仍生效；无外部请求/进程/文件变化；不能让 Ctrl+反斜杠或 /sessions 别名绕过限制 |

## GT34 禁用命令：/recap

入口：手输 /recap；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/recap.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/recap.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT34-01 | 在首页、活跃会话和帮助中搜索 /recap 及全部别名 | 不提供可执行入口；不能调用缺失 sessionRecap 扩展并假成功 |
| TUI-GT34-02 | 空闲时直接手输 /recap 并 Enter | 本地明确不可用；新增模型请求=0；不能调用缺失 sessionRecap 扩展并假成功 |
| TUI-GT34-03 | 通过 /summarize 尝试“会话回顾” | 同样拒绝；不绕过 canonical gate；不能调用缺失 sessionRecap 扩展并假成功 |
| TUI-GT34-04 | 真实轮次运行/审批等待时提交 /recap | 不能排入未来执行；原轮次归属和审批不变；不能调用缺失 sessionRecap 扩展并假成功 |
| TUI-GT34-05 | 重连、切屏、新建会话和刷新目录后再次手输 /recap | 禁用门仍生效；无外部请求/进程/文件变化；不能调用缺失 sessionRecap 扩展并假成功 |

## GT35 禁用命令：/share

入口：手输 /share；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/share.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/share.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT35-01 | 在首页、活跃会话和帮助中搜索 /share 及全部别名 | 不提供可执行入口；不能上传对话或创建公开链接 |
| TUI-GT35-02 | 空闲时直接手输 /share 并 Enter | 本地明确不可用；新增模型请求=0；不能上传对话或创建公开链接 |
| TUI-GT35-03 | 通过 规范命令带有效参数的调用 尝试“分享会话” | 同样拒绝；不绕过 canonical gate；不能上传对话或创建公开链接 |
| TUI-GT35-04 | 真实轮次运行/审批等待时提交 /share | 不能排入未来执行；原轮次归属和审批不变；不能上传对话或创建公开链接 |
| TUI-GT35-05 | 重连、切屏、新建会话和刷新目录后再次手输 /share | 禁用门仍生效；无外部请求/进程/文件变化；不能上传对话或创建公开链接 |

## GT36 禁用命令：/release-notes

入口：手输 /release-notes；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/release_notes.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/release_notes.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT36-01 | 在首页、活跃会话和帮助中搜索 /release-notes 及全部别名 | 不提供可执行入口；不能把上游产品发布说明当成 Code 当前版本 |
| TUI-GT36-02 | 空闲时直接手输 /release-notes 并 Enter | 本地明确不可用；新增模型请求=0；不能把上游产品发布说明当成 Code 当前版本 |
| TUI-GT36-03 | 通过 /changelog 尝试“更新说明” | 同样拒绝；不绕过 canonical gate；不能把上游产品发布说明当成 Code 当前版本 |
| TUI-GT36-04 | 真实轮次运行/审批等待时提交 /release-notes | 不能排入未来执行；原轮次归属和审批不变；不能把上游产品发布说明当成 Code 当前版本 |
| TUI-GT36-05 | 重连、切屏、新建会话和刷新目录后再次手输 /release-notes | 禁用门仍生效；无外部请求/进程/文件变化；不能把上游产品发布说明当成 Code 当前版本 |

## GT37 禁用命令：/announcements

入口：手输 /announcements；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/announcements.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/announcements.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT37-01 | 在首页、活跃会话和帮助中搜索 /announcements 及全部别名 | 不提供可执行入口；不能调用缺失公告扩展或插入假通知 |
| TUI-GT37-02 | 空闲时直接手输 /announcements 并 Enter | 本地明确不可用；新增模型请求=0；不能调用缺失公告扩展或插入假通知 |
| TUI-GT37-03 | 通过 规范命令带有效参数的调用 尝试“公告” | 同样拒绝；不绕过 canonical gate；不能调用缺失公告扩展或插入假通知 |
| TUI-GT37-04 | 真实轮次运行/审批等待时提交 /announcements | 不能排入未来执行；原轮次归属和审批不变；不能调用缺失公告扩展或插入假通知 |
| TUI-GT37-05 | 重连、切屏、新建会话和刷新目录后再次手输 /announcements | 禁用门仍生效；无外部请求/进程/文件变化；不能调用缺失公告扩展或插入假通知 |

## GT38 禁用命令：/dream

入口：手输 /dream；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/memory_ops.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/memory_ops.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT38-01 | 在首页、活跃会话和帮助中搜索 /dream 及全部别名 | 不提供可执行入口；不能写入、合并或删除持久记忆 |
| TUI-GT38-02 | 空闲时直接手输 /dream 并 Enter | 本地明确不可用；新增模型请求=0；不能写入、合并或删除持久记忆 |
| TUI-GT38-03 | 通过 规范命令带有效参数的调用 尝试“记忆整理” | 同样拒绝；不绕过 canonical gate；不能写入、合并或删除持久记忆 |
| TUI-GT38-04 | 真实轮次运行/审批等待时提交 /dream | 不能排入未来执行；原轮次归属和审批不变；不能写入、合并或删除持久记忆 |
| TUI-GT38-05 | 重连、切屏、新建会话和刷新目录后再次手输 /dream | 禁用门仍生效；无外部请求/进程/文件变化；不能写入、合并或删除持久记忆 |

## GT39 禁用命令：/flush

入口：手输 /flush；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/memory_ops.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/memory_ops.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT39-01 | 在首页、活跃会话和帮助中搜索 /flush 及全部别名 | 不提供可执行入口；不能清空/重排记忆或产生假保存确认 |
| TUI-GT39-02 | 空闲时直接手输 /flush 并 Enter | 本地明确不可用；新增模型请求=0；不能清空/重排记忆或产生假保存确认 |
| TUI-GT39-03 | 通过 规范命令带有效参数的调用 尝试“刷新记忆” | 同样拒绝；不绕过 canonical gate；不能清空/重排记忆或产生假保存确认 |
| TUI-GT39-04 | 真实轮次运行/审批等待时提交 /flush | 不能排入未来执行；原轮次归属和审批不变；不能清空/重排记忆或产生假保存确认 |
| TUI-GT39-05 | 重连、切屏、新建会话和刷新目录后再次手输 /flush | 禁用门仍生效；无外部请求/进程/文件变化；不能清空/重排记忆或产生假保存确认 |

## GT40 禁用命令：/memory

入口：手输 /memory；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/memory.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/memory.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT40-01 | 在首页、活跃会话和帮助中搜索 /memory 及全部别名 | 不提供可执行入口；不能从 /mem 别名绕过缺失 memory 扩展 |
| TUI-GT40-02 | 空闲时直接手输 /memory 并 Enter | 本地明确不可用；新增模型请求=0；不能从 /mem 别名绕过缺失 memory 扩展 |
| TUI-GT40-03 | 通过 /mem 尝试“记忆管理” | 同样拒绝；不绕过 canonical gate；不能从 /mem 别名绕过缺失 memory 扩展 |
| TUI-GT40-04 | 真实轮次运行/审批等待时提交 /memory | 不能排入未来执行；原轮次归属和审批不变；不能从 /mem 别名绕过缺失 memory 扩展 |
| TUI-GT40-05 | 重连、切屏、新建会话和刷新目录后再次手输 /memory | 禁用门仍生效；无外部请求/进程/文件变化；不能从 /mem 别名绕过缺失 memory 扩展 |

## GT41 禁用命令：/loop

入口：手输 /loop；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/loop_cmd.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/loop_cmd.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT41-01 | 在首页、活跃会话和帮助中搜索 /loop 及全部别名 | 不提供可执行入口；不能创建未支持调度器或持续后台任务 |
| TUI-GT41-02 | 空闲时直接手输 /loop 并 Enter | 本地明确不可用；新增模型请求=0；不能创建未支持调度器或持续后台任务 |
| TUI-GT41-03 | 通过 规范命令带有效参数的调用 尝试“定时循环” | 同样拒绝；不绕过 canonical gate；不能创建未支持调度器或持续后台任务 |
| TUI-GT41-04 | 真实轮次运行/审批等待时提交 /loop | 不能排入未来执行；原轮次归属和审批不变；不能创建未支持调度器或持续后台任务 |
| TUI-GT41-05 | 重连、切屏、新建会话和刷新目录后再次手输 /loop | 禁用门仍生效；无外部请求/进程/文件变化；不能创建未支持调度器或持续后台任务 |

## GT42 禁用命令：/auto

入口：手输 /auto；补全/帮助/别名。优先级：P0。执行通道：`G`。夹具：`backend-gates`。

可达性：CODE BACKEND UNAVAILABLE：只有拒绝路径可测；不计算该功能正向通过。

源码：[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/auto.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/commands/auto.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/app/app_view.rs)；[apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs](../../../vendor/a3s-code-ui/crates/codegen/a3s-code-pager/src/slash/registry.rs)。

| 用例 | 操作 | 通过判据 |
| --- | --- | --- |
| TUI-GT42-01 | 在首页、活跃会话和帮助中搜索 /auto 及全部别名 | 不提供可执行入口；不能启用未声明的 auto 能力或放宽权限 |
| TUI-GT42-02 | 空闲时直接手输 /auto 并 Enter | 本地明确不可用；新增模型请求=0；不能启用未声明的 auto 能力或放宽权限 |
| TUI-GT42-03 | 通过 规范命令带有效参数的调用 尝试“自动模式” | 同样拒绝；不绕过 canonical gate；不能启用未声明的 auto 能力或放宽权限 |
| TUI-GT42-04 | 真实轮次运行/审批等待时提交 /auto | 不能排入未来执行；原轮次归属和审批不变；不能启用未声明的 auto 能力或放宽权限 |
| TUI-GT42-05 | 重连、切屏、新建会话和刷新目录后再次手输 /auto | 禁用门仍生效；无外部请求/进程/文件变化；不能启用未声明的 auto 能力或放宽权限 |
