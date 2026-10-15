# 模型工具入口映射

这是源码注册清单，尚未证明每个工具在本次 TUI profile 的实际广告和执行资格。基础用例计数保持 1,020；下列工具复用已单列的功能行，每行至少五例。

| 精确工具名 | 功能行 | 注册/执行条件 | 源码 |
| --- | --- | --- | --- |
| `read` | TL01 | read capability | [实现](../../../../../apps/code/core/src/tools/builtin/read.rs) |
| `ls` | TL02 | read capability | [实现](../../../../../apps/code/core/src/tools/builtin/ls.rs) |
| `write` | TL08 | write capability | [实现](../../../../../apps/code/core/src/tools/builtin/write.rs) |
| `download` | TL26 | write capability + local root | [实现](../../../../../apps/code/core/src/tools/builtin/download/mod.rs) |
| `edit` | TL09 | read + write | [实现](../../../../../apps/code/core/src/tools/builtin/edit.rs) |
| `patch` | TL10 | read + write | [实现](../../../../../apps/code/core/src/tools/builtin/patch.rs) |
| `bash` | TL11, TL12 | exec capability + execution qualification | [实现](../../../../../apps/code/core/src/tools/builtin/bash.rs) |
| `background` | TL13, TL14, TL15 | exec capability + execution qualification | [实现](../../../../../apps/code/core/src/tools/builtin/background.rs) |
| `search` | TL03, TL04, TL05, TL06, TL07 | search/read/index capabilities select modes | [实现](../../../../../apps/code/core/src/tools/builtin/search.rs) |
| `git` | TL16, TL17, TL18, TL19, TL20, TL21, TL22, TL23 | git capability | [实现](../../../../../apps/code/core/src/tools/builtin/git.rs) |
| `web_fetch` | TL24 | network policy | [实现](../../../../../apps/code/core/src/tools/builtin/web_fetch.rs) |
| `web_search` | TL25 | provider/network configuration | [实现](../../../../../apps/code/core/src/tools/builtin/web_search.rs) |
| `update_plan` | TL28 | baseline session | [实现](../../../../../apps/code/core/src/tools/builtin/update_plan.rs) |
| `ask_user` | RT06 | host question bridge | [实现](../../../../../apps/code/core/src/tools/builtin/ask_user.rs) |
| `ask_user_form` | RT06 | host form bridge | [实现](../../../../../apps/code/core/src/tools/builtin/ask_user_form.rs) |
| `batch` | TL27 | registered inner tools and their permissions | [实现](../../../../../apps/code/core/src/tools/builtin/batch.rs) |
| `generate_object` | TL29 | session LLM capabilities | [实现](../../../../../apps/code/core/src/tools/builtin/generate_object.rs) |
| `search_skills` | EX01 | effective skill catalog | [实现](../../../../../apps/code/core/src/tools/skill.rs) |
| `Skill` | EX02 | effective skill catalog | [实现](../../../../../apps/code/core/src/tools/skill.rs) |
| `task` | EX03, EX04 | admitted delegation + child capabilities | [实现](../../../../../apps/code/core/src/tools/task.rs) |
| `code_symbols` | EX09 | code intelligence host | [实现](../../../../../apps/code/core/src/tools/builtin/code_intelligence/symbols.rs) |
| `code_navigation` | EX10 | code intelligence host | [实现](../../../../../apps/code/core/src/tools/builtin/code_intelligence/navigation.rs) |
| `code_diagnostics` | EX11 | code intelligence host | [实现](../../../../../apps/code/core/src/tools/builtin/code_intelligence/diagnostics.rs) |

注册来源：[基础工具](../../../core/src/tools/builtin/mod.rs)、[会话能力](../../../core/src/agent_api/capabilities.rs)。

运行前保留真实工具名称及 schema 摘要，与本表逐项比较。缺失项标记条件不满足或 BLOCKED，不能算正向通过；新增项必须单独建功能行并补至少 5 例，再更新计数与源码快照。每个 MCP/插件工具须验证自己的参数、返回值、副作用、权限和取消语义，不能仅由五条通用连接用例替代。

`program` 未在该会话能力构建路径注册；`parallel_task` 不单独注册，fan-out 由 `task` 承担并映射 EX04。`open_local_asset`、`search_wiki` 的 Studio 宿主桥不能仅凭 ACP 中存在实现就算独立 TUI 可触达；实际正式启动链如广告这两个工具，同样先各补五例。
