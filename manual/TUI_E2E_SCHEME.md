# A3S Code TUI 端到端测试方案

本文件是交互式 `a3s code` 的可执行检查表。它不代替
[FULL_FEATURE_TEST_PLAN.md](FULL_FEATURE_TEST_PLAN.md) 或
[FIRST_PRINCIPLES_E2E.md](FIRST_PRINCIPLES_E2E.md)，也不运行那两份方案里的矩阵。

## 1. 被测产品

交互式 `a3s code` 启动的是分页器进程，不是进程内 UI。

| 角色 | 名称 |
| --- | --- |
| 启动器 | `apps/cli/src/code_pager.rs` 的 `code_tui_binary()`。`LAUNCH_VERSION = "9.1.2"`，过期针是 `9.1.1` |
| 分页器包 | `a3s-code-pager-bin`（`apps/code/vendor/a3s-code-ui/crates/codegen/a3s-code-pager-bin`） |
| 分页器二进制 | 该包的 `[[bin]] name = "a3s-code-tui"`，调试产物在 `apps/code/vendor/a3s-code-ui/target/debug/a3s-code-tui` |
| 会话代理 | 旁边的 `a3s-code-acp`（包 `a3s-code-acp`，版本 `9.1.2`）。ACP 走 stdio，驱动 `a3s-code-core` |

`apps/code/acp/tests/shipped_tui_e2e.rs` 的 `tui_binary()` 使用 `A3S_CODE_TUI_BIN`，否则使用上面的分页器调试二进制，代理使用 `CARGO_BIN_EXE_a3s-code-acp`。这就是本方案的 PTY 启动路径。

薄 Hybrid 脚手架是 Cargo 包 `apps/code/tui`（包名也叫 `a3s-code-tui`）。`apps/code/tui/src/session.rs:34` 用 `with_session_id("tui-turn")` 固定它的会话号，会话号是 `tui-turn`。字符串 `tui-session` 只出现在 `assert_ne!` 里：`acp/src/agent.rs`、`acp/tests/v912_seam.rs`、`core/src/fact_control.rs`、`core/src/agent_api/tests/fact_log.rs`。本方案不编译这个包、不启动它、不用它的测试当通过证据。产品二进制只来自 `a3s-code-pager-bin` 的 `a3s-code-tui`。

2026-10-04 那次会话状态栏仍显示 `always-approve`，是因为当时的二进制早于这次改名。PTY 检查对着本树编出的分页器，不对着那个旧进程。

## 2. 和现有方案怎么叠

| 父方案 | 本方案怎么用它 |
| --- | --- |
| [FULL_FEATURE_TEST_PLAN.md](FULL_FEATURE_TEST_PLAN.md) | 训练顺序、L0–L8、证据规则保持原样。§6 把 `a3s code` TUI 的面板和斜杠交给 CLI。本文件补上那一列的分页器检查，不改 §6 的宿主边界 |
| [FIRST_PRINCIPLES_E2E.md](FIRST_PRINCIPLES_E2E.md) | Layer A–C 仍是 harness 矩阵。本方案不重跑 `just harness-convergence-check` 或 `just layer-c-live-e2e`。Layer D 仍是外部资格 |
| `acp/tests/v912_seam.rs` | 9.1.2 会话接缝。新断言写进这个文件 |
| `acp/tests/shipped_tui_e2e.rs` | 已交付的 PTY。新的屏幕观察写进这个文件 |
| `acp/tests/live_tui_prompt.rs` | 可选的真实模型针，默认 `#[ignore]` |
| `core/src/tools/builtin/bash/tests.rs` 与 `core/src/porcelain.rs` | 变更归属。新的 git 子命令断言写进这两个文件 |

不另建一套产品套件，不另起一个 TUI 包。

## 3. 层

每条检查只放在能看见该内核效果的最便宜一层。活模型轮次不能用来证明一层已经能在本地看见的不变量。通过观察是姿态字符串、路径列表、摘要、停止原因或屏幕上的状态/错误文本。模型正文不是通过观察。

| 层 | 进程 | 目标目录 | 何时跑 |
| --- | --- | --- | --- |
| hermetic-core | `cargo test -p a3s-code-core --lib`，工作目录 `apps/code` | `apps/code/target` | 第一 |
| hermetic-pager | 分页器与 shell 的库测试，工作目录 `apps/code/vendor/a3s-code-ui` | `apps/code/vendor/a3s-code-ui/target` | 第二。与上一层串行，两个 target 不要同时 cargo |
| acp-mock | `a3s-code-acp` 进程加本地 HTTP mock，`v912_seam.rs` | `apps/code/target` | 第三。等 hermetic-core 的 cargo 退出 |
| pty | 本树 `a3s-code-tui` 加 `a3s-code-acp`，`shipped_tui_e2e.rs` | 二进制来自分页器 target，测试 cargo 用 `apps/code/target` | 第四。先 `cargo build -p a3s-code-pager-bin --bin a3s-code-tui`，且该 build 不与 hermetic-pager 重叠 |
| live-flash | `live_tui_prompt.rs`，`#[ignore]` | `apps/code/target` | 可选，本方案的通过条件不含这一层 |

工具链是各树 `rust-toolchain.toml` 的 Rust 1.98.1：`rustup run 1.98.1 cargo …`。不加 `--locked`。同一 target 上只留一个 cargo。

## 4. 检查表

每一行都有入口、层和通过观察。通过观察是姿态字符串、路径列表、摘要、停止原因或屏幕上的状态/错误文本。

### 4.0 启动路径

| ID | 层 | 入口 | 通过观察 |
| --- | --- | --- | --- |
| L0 | hermetic-core | `select_launch_binary` 与 `bundled_acp_executable`（`apps/cli/src/code_pager.rs`），测试 `v912_resolver_skips_a_9_1_1_binary`、`bundled_acp_uses_the_pager_sibling`。cargo 包名 `a3s`，工作目录 `apps/cli`，target 是 `apps/cli/target` | 候选里版本号为 `9.1.1` 的二进制被跳过，选中的是打印 `9.1.2` 的那个。工作区 `apps/code/target/debug/a3s-code-acp` 存在时优先于分页器旁边的发布副本；工作区文件不存在时，代理路径等于分页器同目录的 `a3s-code-acp`。CLI 的 `Cargo.toml` 不含 `a3s-code-pager`，交互式 `a3s code` 启动外部进程。本方案的 PTY 把 `A3S_CODE_TUI_BIN` 定在包 `a3s-code-pager-bin` 的产物 `apps/code/vendor/a3s-code-ui/target/debug/a3s-code-tui`，代理是 `a3s-code-acp`。`code_tui_candidates()` 的最后一项 `apps/code/target/debug/a3s-code-tui` 也是薄包 `apps/code/tui` 的默认产物路径；本方案不编译、不启动那个包，也不把它的二进制当作被测产品 |

### 4.1 自动批准只叫 yolo，而且不豁免完成门

| ID | 层 | 入口 | 通过观察 |
| --- | --- | --- | --- |
| H1 | hermetic-core | `policy_for_posture`（`core/src/permissions/policy.rs`），测试 `seam_admission_permission_posture_denies_under_yolo` | `"yolo"` 对未拒绝的 write 是 Allow；`"always-approve"` 和 `"force"` 对 bash 是 Ask；`policy_for_posture("yolo").deny("bash(*)")` 对 bash 仍是 Deny |
| H2 | hermetic-core | `decide_with_observations`（`core/src/harness_loop.rs`），测试 `unbound_mutation_does_not_spend_a_continuation_the_model_cannot_close` | 非空账本、无 Passed 报告、无宿主豁免时，即使 `allow_continuation` 为真，结果仍是 Incomplete，不是 Allow，也不是 Verified。助手正文不关闭完成门。消息以 `completion gate:` 开头，含变更路径 `src/lib.rs`，含 `Assistant text does not count`。该函数没有姿态参数，yolo 不能把它选成 Allow |
| P1 | hermetic-pager | `PermissionModeKind::as_canonical` / `from_canonical`（`a3s-code-pager/src/app/actions.rs`）；`parse_permission_mode_canonical`（`a3s-code-shell/src/util/config/permissions.rs`），测试 `parse_permission_mode_canonical_covers_all_canonicals_plus_fallback` | `AlwaysApprove` 的规范串是 `yolo`。`from_canonical("always-approve")` 与 `from_canonical("force")` 为 None。`parse_permission_mode_canonical("always-approve")` 与 `("force")` 为 Ask。在现有 fallback 测试里补上这两个字符串 |
| P2 | hermetic-pager | `PERMISSION_MODE_CHOICES`（`a3s-code-pager/src/settings/defs.rs`），测试 `pr6_search_yolo_matches_permission_mode`、`pr6_permission_mode_picker_enter_dispatches_set_permission_mode_commit`。这两个函数在 `a3s-code-pager/tests/settings_e2e.rs`，cargo 用 `--test settings_e2e`，不在 `--lib` 里 | 目录里自动批准的 canonical 只有 `yolo`，display 是 `Yolo`。`search("yolo")` 只命中 `permission_mode`。没有 canonical `always-approve` |
| P3 | hermetic-pager | `BuiltinCommand` `name: "yolo"`、`aliases: &[]`（`a3s-code-shell/src/session/slash_commands.rs`），测试 `always_approve_parses_on_off`、`yolo_alias_resolves_to_always_approve` | `/yolo` 解析为 `SetYolo`。`/always-approve` 与 `/force` 不是这条命令。在同一测试文件里断言后两者没有 `SetYolo` |
| A1 | acp-mock | `canonical_posture` 与 `set_session_mode`（`acp/src/agent.rs`），测试 `v912_session_posture_and_deny` | 模式名单只有 `default`、`plan`、`auto`、`yolo`。`set_session_mode` 之后 `x.ai/session/info` 的 `permissionPosture` 等于该字符串。新会话停在 `default` 时，再 `set_session_mode("always-approve")` 和 `set_session_mode("force")`，`permissionPosture` 仍是 `default`。`x.ai/permission_posture` 传入这两个字符串时，错误数据含 `unknown posture`。这两个名字落到 Ask：`policy_for_posture("always-approve")` 与 `policy_for_posture("force")` 对 bash 是 Ask（H1）。它们不会变成自动批准。yolo 加 `deny: bash(*)` 时标记文件不存在 |
| A2 | acp-mock | `v912_completion_gate_keeps_path`（`acp/tests/v912_seam.rs`）。先 `set_mode(..., "yolo")`，再发 `WRITE-GATE gate-target.ts` | 发提示之前 `permissionPosture` 是 `yolo`。提示结果是 Err。`error.data` 含 `completion gate:`、变更路径 `gate-target.ts`、`Assistant text does not count`，不含 `Try sending again`。yolo 不豁免完成门 |
| P4 | hermetic-pager | `format_request_failure`（`a3s-code-pager/src/app/error_display.rs`），测试 `completion_gate_keeps_the_kernel_sentence` | `formatted.message()` 等于传入的原始内核句。该句含 `completion gate:`、路径 `apps/docs/components/home/project-links.ts`、`Assistant text does not count`、`has no bound Passed verification and no host waiver`，不含 `Try sending again` |
| P5 | pty | `shipped_tui_launch_resume_and_visible_commands` 里现有的 `/yolo` 段，屏幕来自 `PromptWidget::render_info_line`，旗标用 `PermissionLabel::as_canonical` | 泵出的帧含旗标文本 `yolo`，不含 `always-approve`。既有文件观察保持：`side-effect.txt` 内容是 `wrote-marker`。二进制是本树 `a3s-code-pager-bin` 的 `a3s-code-tui` |

`InteractiveApprovalMode::from_name`（`core/src/permissions/interactive.rs`）把 `"force"` 和 `"yolo"` 都映成 `Force`。这是 CLI 审批模式。H1 的通过观察只看 `policy_for_posture` 返回的 `PermissionDecision`：`"yolo"` 对未拒绝的 write 是 Allow，`"always-approve"` 和 `"force"` 对 bash 是 Ask。

### 4.2 只读 git 不领取并行提交；自己的写入和移动 HEAD 的 git 仍入账

内核句在 `decide_with_observations` 里，非空账本且没有绑定的 Passed 报告、也没有宿主豁免时固定为：

`completion gate: workspace mutation {digest}{paths} has no bound Passed verification and no host waiver. Assistant text does not count. Bind a verification_report.effect_digest to this digest with required checks Passed, or obtain a host waiver for this digest.`

路径最多列出 4 个，其余写成 `and N more`。

| ID | 层 | 入口 | 通过观察 |
| --- | --- | --- | --- |
| H3 | hermetic-core | `BashTool::execute`，测试 `bash_readonly_git_does_not_take_a_parallel_commit`、`a_parallel_commit_during_bash_is_not_this_commands_mutation` | 命令只跑只读 git（`status`、`ls-files`、`log`、`check-ignore`，含 `git -c … status`）时，并行提交产生的 `guest.txt` 不在 `changed_paths`。本命令写下的 `proof.txt` 仍在。`sleep` 期间的并行提交同样不在 |
| H4 | hermetic-core | `BashTool::execute`，测试 `bash_that_invokes_git_keeps_its_own_commit`（`core/src/tools/builtin/bash/tests.rs`） | `git -C . -c user.email=test@example.com commit` 之后 `changed_paths` 含 `owned.txt`。见证只在这些子命令上写入：`commit`、`checkout`、`switch`、`reset`、`merge`、`rebase`、`pull`、`cherry-pick`、`revert`、`am`（全局选项 `-c`、`-C`、`--git-dir`、`--work-tree`、`--namespace`、`--exec-path`、`--super-prefix`、`--config-env` 先跳过）。同一文件再加一条 `git checkout`：检出把 `switched.txt` 带入工作区后，`changed_paths` 含 `switched.txt` |
| H5 | hermetic-core | `BashTool::execute`，测试 `sandbox_bash_records_changed_paths_from_the_workspace_not_the_command` | 命令文本是 `printf done`，工作区在执行期间出现 `guest.txt` 时，`changed_paths` 含 `guest.txt`。断言不解析命令字符串 |
| H6 | hermetic-core | `Watch::finish_with_revision` / `command_delta`（`core/src/porcelain.rs`），测试 `a_foreign_commit_is_not_a_command_mutation_unless_this_command_invoked_git`、`a_clean_revision_path_is_not_kept_when_git_was_not_invoked`、`a_revision_path_left_dirty_stays_on_the_command`、`a_failed_reread_is_not_an_empty_mutation` | `attribute_revision = false` 时，已干净的 `guest.txt` 离开命令增量，未提交的 `local.txt` 留下；`guest.txt` 仍 porcelain-dirty 时留下。`attribute_revision = true` 时，本窗口提交的 `owned.txt` 留下。`a_failed_reread_is_not_an_empty_mutation`：after 读缺失时 `incomplete` 为真，路径列表为空，这个空列表不当成空变更 |
| A3 | acp-mock | `decide`、`open_session`、`say`、`effect_log`（`acp/tests/v912_seam.rs`）。提示正文含 `READONLY-GIT` 时，`decide` 返回一次 bash 工具调用，命令是 `git -c core.quotepath=false status`、`git ls-files`、`git log`、`git check-ignore`；同工作区另一个进程在命令期间提交 `guest.txt` | `say` 返回 `Ok(PromptResponse)`。`effect_log` 文本不含 `completion gate:`，变更观察的路径列表不含 `guest.txt`。这一轮的停止结果是成功的提示响应 |
| A2 | acp-mock | `v912_completion_gate_keeps_path`（`acp/tests/v912_seam.rs`）。先 `set_mode(..., "yolo")`，再发 `WRITE-GATE gate-target.ts`。助手在同一轮里可以有正文 | 发提示之前 `permissionPosture` 是 `yolo`。提示结果是 Err。`error.data` 含完整内核句 `completion gate: workspace mutation`、变更路径 `gate-target.ts`、`Assistant text does not count`，不含 `Try sending again`。yolo 不豁免完成门，助手正文不把 Err 变成成功 |
| P4 | hermetic-pager | `format_request_failure`（`a3s-code-pager/src/app/error_display.rs`），测试 `completion_gate_keeps_the_kernel_sentence` | 用户可见字符串等于原始内核句：`formatted.message()` 与传入串逐字相同，含 `completion gate: workspace mutation`、路径 `apps/docs/components/home/project-links.ts`、`Assistant text does not count`、`has no bound Passed verification and no host waiver`，不含 `Try sending again` |

H3 是 2026-10-04 那次 `Turn failed in 18m57s` 的回归：审查期间并行提交被记到只读 `git status` / `git ls-files` 上，账本里出现 `crates/flow/Cargo.toml` 等 11 个路径。A3 把同一归属放到 `a3s-code-acp` 的一轮上，确认这一轮不再因那些路径失败。

### 4.3 9.1.2 会话机制

| ID | 机制 | 层 | 入口 | 通过观察 |
| --- | --- | --- | --- | --- |
| A4 | 常驻上下文在第一次模型调用前入账 | acp-mock | `v912_standing_context_before_model` | 效果日志有 `a3s.standing-context`，`digest` 为 `sha256:` 加 64 位小写十六进制，且载荷不含指令正文。该 fact 的 `seq` 小于第一条 `user.message`。mock 请求体含指令行 |
| A5 | 压缩保留义务 | acp-mock | `v912_compact_keeps_obligations`，方法 `x.ai/compact_conversation` | 最后一条 `compaction.done` 的 `summary` 含 `# Instructions (personal + project AGENTS.md chain)`、`CARRY-PERSONAL-7f3a`、`CARRY-PROJECT-7f3a`、现行目标里的 JSON exporter、`Must not: add a dependency to parse CSV.`、`## Open steps`、`write the JSON exporter`。`## Goal` 一节不含已被替换的 CSV parser 任务 |
| H7 | 用户图片字节穿过压缩 | hermetic-core | `user_attachment_plan`（`core/src/fact_control.rs`），测试 `user_attachment_plan_restores_png_bytes_after_compaction`。代理孪生是 `v912_image_bytes_reach_the_model`：mock 请求体含 `PNG_BASE64` | `compaction.done` 之后恢复出的用户消息，首个内容块是 `ContentBlock::Image`，解码字节等于原来的 PNG，`media_type` 是 `image/png`。不另开活模型图片轮次 |
| A6 | 旧 remember 保证，待迁移 | acp-mock | 历史 `v912_remember_before_stream_close` | 原用例要求提示返回前存在 token + `explicit-preference` 文件；该关键词授权保证已撤销，历史通过不能证明现行行为。现行自动写入必须有通过校验的模型条目和存储确认，End 不代表落盘；此 ACP 用例尚待迁移，当前真实模型正向证据见 `core/tests/test_memory_store_real_llm.rs` |
| H8 | 相同 run-event 游标先重放，再写新链摘要 | hermetic-core | `CoreEventLog::append_run_event`，测试 `v912_cursor_replays_before_new_chain_digest`。函数在 `acp/tests/v912_seam.rs`，不启代理、不调模型。它随第 4 步 `cargo test -p a3s-code-acp --test v912_seam` 跑，不在 `a3s-code-core --lib` 的过滤器里 | 同一条记录第二次追加：`replayed` 为真、`appended` 为假，`entry_digest` 不变。下一条记录 `appended` 为真，`previous_digest` 等于那条摘要，`entry_digest` 与它不同 |

### 4.4 已经在 PTY 上驱动的分页器结果

入口都在 `shipped_tui_e2e.rs`。代理是 mock，不是活模型。

| ID | 结果 | 层 | 入口 | 通过观察 |
| --- | --- | --- | --- | --- |
| P6 | 启动 | pty | `launch_once(false)`，测试 `shipped_tui_launch_resume_and_visible_commands` | 首帧非空，且含 ACL 模型 id |
| P7 | 恢复 / 继续 | pty | 同测试的 `launch_once(true)` 与 `launch_continued` | `--continue` 的会话 id 等于第一次的 id。旁边发现代理、不设 `A3S_ACP_AGENT_BIN` 时同样相等。oracle 名 `continue-same-session`、`continue-without-agent-env` |
| P8 | 可见命令面板 | pty | 同测试，`palette_offers` 对照 `FULLSCREEN_PALETTE` | 面板上有 `yolo`，描述含 `skip all permission prompts`。隐藏名不在面板上 |
| P9 | 只有 xAI key 时模型字段保持 ACL 默认 | pty | `shipped_tui_xai_key_keeps_acl_model` | 信息行模型字段等于 `acl_default_model(config)`。环境里凭证只有 `XAI_API_KEY`。home 下没有 `cc-switch.db` |

### 4.5 留在外部资格、不进入上面四层

| ID | 层 | 入口 | 通过观察 |
| --- | --- | --- | --- |
| X1 | 不在 hermetic-core、hermetic-pager、acp-mock、pty、live-flash | [FIRST_PRINCIPLES_E2E.md](FIRST_PRINCIPLES_E2E.md) Layer D1 `TB-QUAL1`、Layer D2 `DM-PROD1`；[FULL_FEATURE_TEST_PLAN.md](FULL_FEATURE_TEST_PLAN.md) 的 L7 与 enterprise GA 定义 | 第 5 节的 cargo 命令不调用 Harbor、TB-QUAL1、DM-PROD1，也不把 v9.1.2 或本文件记成 enterprise GA。这三项保持延后。本文件是检查方案，不是那次矩阵的一次运行 |

## 5. 执行顺序

在 `apps/code` 下，一条 cargo 结束再开下一条。过滤器用测试名子串。

1. hermetic 启动路径。target 是 `apps/cli/target`，与 `apps/code/target` 不是同一个目录，仍然等这一条结束再开下一条。

```bash
cd apps/cli
rustup run 1.98.1 cargo test -p a3s --lib -- --test-threads=8 \
  v912_resolver_skips_a_9_1_1_binary \
  bundled_acp_uses_the_pager_sibling
```

2. hermetic-core。H8 不在这条过滤器里，它在第 4 步的 `v912_seam` 里。

```bash
cd apps/code
rustup run 1.98.1 cargo test -p a3s-code-core --lib -- --test-threads=8 \
  seam_admission_permission_posture_denies_under_yolo \
  unbound_mutation_does_not_spend_a_continuation \
  bash_readonly_git_does_not_take_a_parallel_commit \
  a_parallel_commit_during_bash \
  bash_that_invokes_git_keeps \
  sandbox_bash_records_changed_paths \
  a_foreign_commit_is_not_a_command_mutation \
  a_clean_revision_path_is_not_kept \
  a_revision_path_left_dirty \
  a_failed_reread_is_not_an_empty_mutation \
  user_attachment_plan_restores_png_bytes
```

3. hermetic-pager，然后编出 PTY 要用的二进制。工作目录换成 vendor 树，target 是 `apps/code/vendor/a3s-code-ui/target`。等第 2 步退出后再开，避免两个 cargo 打在 `apps/code/target` 上；这一步用的是另一个 target。下面四条 cargo 按书写顺序串行，打在同一个 vendor target 上。`a3s-code-pager --lib` 只覆盖 P1 的 `permission_mode_kind_tests` 和 P4 的 `completion_gate_keeps_the_kernel_sentence`。P2 的两个 `pr6_*` 在集成测试 `settings_e2e`，不在 `--lib` 里。

```bash
cd apps/code/vendor/a3s-code-ui
rustup run 1.98.1 cargo test -p a3s-code-pager --lib -- --test-threads=8 \
  completion_gate_keeps_the_kernel_sentence \
  permission_mode_kind
rustup run 1.98.1 cargo test -p a3s-code-pager --test settings_e2e -- --test-threads=8 \
  pr6_search_yolo_matches_permission_mode \
  pr6_permission_mode_picker_enter
rustup run 1.98.1 cargo test -p a3s-code-shell --lib -- --test-threads=8 \
  always_approve_parses_on_off \
  yolo_alias_resolves_to_always_approve \
  parse_permission_mode_canonical_covers_all_canonicals_plus_fallback
rustup run 1.98.1 cargo build -p a3s-code-pager-bin --bin a3s-code-tui
```

4. acp-mock。回到 `apps/code`，此时第 2 步的 cargo 已经退出。这一步包含 H8 `v912_cursor_replays_before_new_chain_digest`，以及 A1–A6。

```bash
cd apps/code
rustup run 1.98.1 cargo test -p a3s-code-acp --test v912_seam -- --test-threads=1
```

5. pty。第 3 步的二进制已经在 `vendor/a3s-code-ui/target/debug/a3s-code-tui`。覆盖 P5–P9：启动、恢复 / 继续、可见命令面板、只有 xAI key 时的 ACL 模型字段、信息行旗标 `yolo`。

```bash
cd apps/code
rustup run 1.98.1 cargo test -p a3s-code-acp --test shipped_tui_e2e -- --test-threads=1 \
  shipped_tui_launch_resume_and_visible_commands \
  shipped_tui_xai_key_keeps_acl_model
```

6. live-flash，可选。不作为第 1–5 步的通过条件，也不证明 H1–H8 里已经能在本地看见的不变量。`A3S_TEST_MODEL=boyue/bailian/deepseek-v4-flash` 时，`pin_test_model` 把它改写成 ACL 里的 `boyue/bailian/deepseek-v4.1-flash`。

```bash
cd apps/code
A3S_CONFIG_FILE=/abs/path/.a3s/config.acl \
  rustup run 1.98.1 cargo test -p a3s-code-acp --test live_tui_prompt -- \
  --ignored --test-threads=1 --nocapture
```

通过观察在 `live_prompt_copies_the_marker_through_the_tui_agent`：`copied.txt` 含 `tui-acp-marker-6c1d`，`marker.txt` 仍是该记号；若提示返回 Err，`error.data` 含 `completion gate:`。

## 6. 本方案明确不做

- 不跑 Terminal-Bench / TB-QUAL1。`FIRST_PRINCIPLES_E2E.md` Layer D1 仍由 Harbor 与 Code runner 持有。产品豁免日期是 2026-09-26：`V9_0_0_COMPLETION_ROADMAP.md:52` 写明 `TB-QUAL1 waived by product decision (2026-09-26)`，`HARNESS_CONVERGENCE.md:71` 的 ROADMAP link date 写明完整 `-k 5` 矩阵 deferred by product。
- 不把本方案或 v9.1.2 渠道标签当成 enterprise GA。GA 仍要同一提交上的 L0–L6、L8、DM-PROD1 引用和完整 TB-QUAL1。
- 不跑 DM-PROD1。Layer D2 仍是宿主报告。`HARNESS_CONVERGENCE.md:77` 把该报告针在 tip Code `be467457`（RC `b91462d3` stack）。本方案不把那份报告当作 9.1.2 的一次运行。
- 本文件是检查方案，不是上述矩阵的一次运行。
- 不把助手正文计为 Passed 验证，也不规划“yolo 跳过完成门”的测试。
- 不改这些名字：权限提示选项 id `enable-always-approve`；协议记号 `bypassPermissions`（`permission_mode_label` 在 yolo 时仍返回它，它不是产品姿态名）；OS CLI 的 `a3s code --force`（`apps/cli` 里的执行旗标，可见别名仍是 yolo）。

## 7. 后来的实现者补断言的位置

只改第 2 节列出的四个套件，外加 P1–P3 所在的分页器/shell 测试。L0 的断言已经在 `apps/cli/src/code_pager.rs`。

| 缺口 | 写到 |
| --- | --- |
| `from_canonical("always-approve")` 与 `from_canonical("force")` 为 None | `permission_mode_kind_tests`（`a3s-code-pager/src/app/actions.rs`） |
| `always-approve`、`force` 解析为 Ask | `parse_permission_mode_canonical_covers_all_canonicals_plus_fallback` |
| `/always-approve`、`/force` 不是 `SetYolo` | `slash_commands_tests.rs`，挨着 `yolo_alias_resolves_to_always_approve` |
| 未知姿态被 ACP 拒绝，且不改已接受的 `permissionPosture` | `v912_session_posture_and_deny` |
| yolo 下的 write 仍失败并保留内核句与路径 | `v912_completion_gate_keeps_path` |
| `READONLY-GIT` 的 bash 期间，并行提交的 `guest.txt` 不让 `say` 失败 | `v912_seam.rs` 的 `decide`，复用 `open_session` / `say` / `effect_log` |
| `checkout`（或集合中另一个非 commit 成员）仍入账 | `bash/tests.rs`，复用 `bash_that_invokes_git_keeps_its_own_commit` 的仓库夹具 |
| 信息行旗标是 `yolo` | `shipped_tui_launch_resume_and_visible_commands` 的 `/yolo` 段 |
