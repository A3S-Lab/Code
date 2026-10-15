# 可达入口与覆盖映射

这是 TUI 测试入口清单，不是第二份 SDK capability id 清单。源快照见 [inventory.json](inventory.json)。所有映射均表示已规划，**不是运行通过**。

## 命令

| 规范命令 | 别名 | 用例族 | 状态 |
| --- | --- | --- | --- |
| /announcements |  | GT37 | 后端禁用 |
| /auto |  | GT42 | 后端禁用 |
| /btw |  | GT09 | 后端禁用 |
| /cd |  | UI13 | 本地/条件可达；待实测 |
| /compact |  | RT14 | 本地/条件可达；待实测 |
| /compact-mode |  | ST01 | 本地/条件可达；待实测 |
| /config |  | MO03, MO04, MO05, MO06 | 本地/条件可达；待实测 |
| /config-agents | /agents | GT21 | 后端禁用 |
| /context |  | RT13 | 本地/条件可达；待实测 |
| /copy |  | SE08 | 本地/条件可达；待实测 |
| /dashboard | /agents-dashboard, /sessions | GT33 | 后端禁用 |
| /debug |  | UI08, UI09, UI10 | 本地/条件可达；待实测 |
| /delete |  | SE05 | 本地/条件可达；待实测 |
| /docs | /howto, /guides | GT05 | 后端禁用 |
| /doctor | /terminal-setup, /terminal-check, /terminal-info | UI12 | 本地/条件可达；待实测 |
| /dream |  | GT38 | 后端禁用 |
| /edit-prompt |  | IN05 | 本地/条件可达；待实测 |
| /effort |  | MO02 | 本地/条件可达；待实测 |
| /expand |  | SE12 | 本地/条件可达；待实测 |
| /export |  | SE07 | 本地/条件可达；待实测 |
| /feedback |  | GT10 | 后端禁用 |
| /find |  | SE09 | 本地/条件可达；待实测 |
| /flush |  | GT39 | 后端禁用 |
| /fork |  | GT12 | 后端禁用 |
| /fullscreen | /full | UI03 | 本地/条件可达；待实测 |
| /gboom |  | UI11 | 本地/条件可达；待实测 |
| /help |  | IN07 | 本地/条件可达；待实测 |
| /history |  | GT15 | 后端禁用 |
| /home | /welcome | SE04 | 本地/条件可达；待实测 |
| /hooks |  | GT25 | 后端禁用 |
| /imagine |  | GT30 | 后端禁用 |
| /imagine-video |  | GT31 | 后端禁用 |
| /import-claude |  | GT27 | 后端禁用 |
| /jump |  | SE10 | 本地/条件可达；待实测 |
| /login |  | GT07 | 后端禁用 |
| /logout |  | GT08 | 后端禁用 |
| /loop |  | GT41 | 后端禁用 |
| /marketplace |  | GT26 | 后端禁用 |
| /mcps |  | GT24 | 后端禁用 |
| /memory | /mem | GT40 | 后端禁用 |
| /minimal |  | UI03 | 本地/条件可达；待实测 |
| /model | /m | MO01 | 本地/条件可达；待实测 |
| /multiline | /ml | IN02 | 本地/条件可达；待实测 |
| /new | /clear | SE01 | 本地/条件可达；待实测 |
| /personas |  | GT20 | 后端禁用 |
| /plan |  | GT03 | 后端禁用 |
| /plugins | /plugin | GT22 | 后端禁用 |
| /privacy |  | GT06 | 后端禁用 |
| /queue |  | GT11 | 后端禁用 |
| /quit | /exit | UI15 | 本地/条件可达；待实测 |
| /recap | /summarize | GT34 | 后端禁用 |
| /release-notes | /changelog | GT36 | 后端禁用 |
| /remember |  | GT28 | 后端禁用 |
| /rename | /title | GT14 | 后端禁用 |
| /resume |  | GT13 | 后端禁用 |
| /rewind | /undo | GT17 | 后端禁用 |
| /scroll-debug |  | UI08 | 本地/条件可达；待实测 |
| /session-info |  | SE06 | 本地/条件可达；待实测 |
| /settings | /preferences, /prefs | UI01 | 本地/条件可达；待实测 |
| /share |  | GT35 | 后端禁用 |
| /skills |  | GT23 | 后端禁用 |
| /tasks |  | GT29 | 后端禁用 |
| /theme | /t | UI02 | 本地/条件可达；待实测 |
| /timeline |  | SE10 | 本地/条件可达；待实测 |
| /timestamps |  | ST03 | 本地/条件可达；待实测 |
| /toggle-mouse-reporting |  | UI05 | 本地/条件可达；待实测 |
| /transcript | /log | GT16 | 后端禁用 |
| /tutorial | /tour, /onboarding | GT04 | 后端禁用 |
| /usage | /cost | GT01 | 后端禁用 |
| /view-plan | /show-plan, /plan-view | GT02 | 后端禁用 |
| /vim-mode |  | SE11 | 本地/条件可达；待实测 |
| /voice |  | GT32 | 后端禁用 |
| /workflow |  | GT19 | 后端禁用 |
| /workflows |  | GT18 | 后端禁用 |
| /yolo |  | RT04 | 本地/条件可达；待实测 |
| /goal | ACP 动态广告 | RT12 | 后端控制命令 |

## 设置

| key | 类型 | 用例族 | 状态 |
| --- | --- | --- | --- |
| compact_mode | Bool | ST01 | 可见；效果待实测 |
| screen_mode | Enum | ST02 | 可见；效果待实测 |
| show_timestamps | Bool | ST03 | 可见；效果待实测 |
| show_timeline | Bool | ST04 | 可见；效果待实测 |
| dashboard_preview | Bool | ST05 | 隐藏限制 |
| page_flip_on_send | Bool | ST06 | 可见；效果待实测 |
| combine_queued_prompts | Bool | ST07 | 隐藏限制 |
| follow_up_behavior | Enum | ST08 | 隐藏限制 |
| confirm_before_rewind | Bool | ST09 | 隐藏限制 |
| simple_mode | Bool | ST10 | 可见；效果待实测 |
| vim_mode | Bool | ST11 | 可见；效果待实测 |
| theme | Enum | ST12 | 可见；效果待实测 |
| auto_dark_theme | Enum | ST13 | 可见；效果待实测 |
| auto_light_theme | Enum | ST14 | 可见；效果待实测 |
| render_mermaid | Enum | ST15 | 可见；效果待实测 |
| permission_mode | Enum | ST16 | 可见；效果待实测 |
| remember_tool_approvals | Bool | ST17 | 隐藏限制 |
| multiline_mode | Bool | ST18 | 可见；效果待实测 |
| default_model | DynamicEnum | ST19 | 隐藏限制 |
| subagent_model_inheritance | Bool | ST20 | 隐藏限制 |
| max_thoughts_width | Int | ST21 | 可见；效果待实测 |
| show_thinking_blocks | Bool | ST22 | 可见；效果待实测 |
| prompt_suggestions | Bool | ST23 | 隐藏限制 |
| respect_manual_folds | Bool | ST24 | 可见；效果待实测 |
| group_tool_verbs | Bool | ST25 | 可见；效果待实测 |
| collapsed_edit_blocks | Bool | ST26 | 可见；效果待实测 |
| display_refresh_auto_cadence | Bool | ST27 | 可见；效果待实测 |
| scroll_speed | Int | ST28 | 可见；效果待实测 |
| scroll_mode | Enum | ST29 | 可见；效果待实测 |
| scroll_lines | Int | ST30 | 可见；效果待实测 |
| invert_scroll | Bool | ST31 | 可见；效果待实测 |
| keep_text_selection | Enum | ST32 | 可见；效果待实测 |
| coding_data_sharing | Enum | ST33 | 隐藏限制 |
| default_selected_permission | Enum | ST34 | 隐藏限制 |
| toolset.ask_user_question.timeout_enabled | Bool | ST35 | 隐藏限制 |
| plan_mode | Enum | ST36 | 隐藏限制 |
| show_tips | Bool | ST37 | 隐藏限制 |
| contextual_hints | Group | ST38 | 可见；效果待实测 |
| auto_update | Bool | ST39 | 隐藏限制 |
| hunk_tracker_mode | Enum | ST40 | 隐藏限制 |
| voice_keybind_enabled | Bool | ST41 | 可见；效果待实测 |
| voice_capture_mode | Enum | ST42 | 可见；效果待实测 |
| voice_stt_language | Enum | ST43 | 可见；效果待实测 |
| contextual_hints.undo | Bool | ST44 | 可见；效果待实测 |
| contextual_hints.plan_mode | Bool | ST45 | 隐藏限制 |
| contextual_hints.image_input | Bool | ST46 | 隐藏限制 |
| contextual_hints.send_now | Bool | ST47 | 可见；效果待实测 |
| contextual_hints.small_screen | Bool | ST48 | 可见；效果待实测 |
| contextual_hints.word_select | Bool | ST49 | 可见；效果待实测 |
| contextual_hints.export_copy | Bool | ST50 | 可见；效果待实测 |
| contextual_hints.ssh_wrap | Bool | ST51 | 可见；效果待实测 |
| fork_secondary_model | DynamicEnum | ST52 | 隐藏限制 |

## 动作和快捷键

ActionId 是覆盖身份；实际按键必须从本候选帮助/默认绑定确认，不能把平台差异写死。Dashboard 子上下文当前不能由 Code 产品入口创建，只记录其入口门及不可达原因，不能人为构造视图后声称 E2E。

| ActionId | 上下文 | 用例族 |
| --- | --- | --- |
| EditPromptExternal | AgentScreen | IN05 |
| ToggleTasks | AgentScreen | GT29 |
| SelectNext | ScrollbackFocused | SE11 |
| SelectPrev | ScrollbackFocused | SE11 |
| NextTurn | ScrollbackFocused | SE10 |
| PrevTurn | ScrollbackFocused | SE10 |
| NextResponse | ScrollbackFocused | SE10 |
| PrevResponse | ScrollbackFocused | SE10 |
| GotoTop | ScrollbackFocused | SE11 |
| GotoBottom | ScrollbackFocused | SE11 |
| ScrollUp | ScrollbackFocused | SE11 |
| ScrollDown | ScrollbackFocused | SE11 |
| HalfPageUp | ScrollbackFocused | SE11 |
| HalfPageDown | ScrollbackFocused | SE11 |
| PageUp | ScrollbackFocused | SE11 |
| PageDown | ScrollbackFocused | SE11 |
| Collapse | ScrollbackFocused | SE12 |
| Expand | ScrollbackFocused | SE12 |
| ToggleFold | ScrollbackFocused | SE12 |
| ToggleExpandAll | ScrollbackFocused | SE12 |
| ExpandAllThinking | ScrollbackFocused | SE12 |
| ToggleRaw | ScrollbackFocused | SE12 |
| CopyBlockContent | ScrollbackFocused | SE08 |
| CopyBlockMeta | ScrollbackFocused | SE08 |
| OpenBlockViewer | ScrollbackFocused | SE13 |
| OpenNextLink | ScrollbackFocused | SE13 |
| OpenPrevLink | ScrollbackFocused | SE13 |
| Rewind | ScrollbackFocused | GT17 |
| KillBgTask | ScrollbackFocused | TL15 |
| SendPrompt | PromptFocused | IN01 |
| FocusPrompt | ScrollbackFocused | IN08 |
| FocusScrollback | PromptFocused | IN08 |
| CancelTurn | AgentScreen | RT07 |
| CycleMode | PromptFocused | RT04 |
| ToggleTodos | AgentScreen | TL28 |
| ToggleQueue | AgentScreen | GT11 |
| OpenSessions | AgentScreen | SE03 |
| OpenExtensions | AgentScreen | GT22, GT24, GT23, GT25 |
| SendToBackground | AgentScreen | TL15 |
| InterjectPrompt | PromptFocused | RT08 |
| EnableVoiceMode | AgentScreen | UI14 |
| VoiceToggle | Always | UI14 |
| ToggleMultiline | PromptFocused | IN02 |
| StashPrompt | PromptFocused | IN04 |
| BashMode | PromptFocused | IN09 |
| ToggleYolo | AgentScreen | RT04 |
| NewSession | Always | SE01 |
| Quit | Always | UI15 |
| CommandPalette | AgentScreen | IN07 |
| ShortcutsHelp | AgentScreen | IN08 |
| ModelPicker | AgentScreen | MO01 |
| OpenSettings | AgentScreen | UI01 |
| ToggleMouseCapture | ScrollbackFocused | UI05 |
| OpenDashboard | Always | GT33 |
| DashboardSelectNext | DashboardFocused | GT33 |
| DashboardSelectPrev | DashboardFocused | GT33 |
| DashboardTogglePin | DashboardFocused | GT33 |
| DashboardBeginRename | DashboardFocused | GT33 |
| DashboardStop | DashboardFocused | GT33 |
| DashboardCycleMode | DashboardFocused | GT33 |
| DashboardToggleGrouping | DashboardFocused | GT33 |
| DashboardReorderUp | DashboardFocused | GT33 |
| DashboardReorderDown | DashboardFocused | GT33 |
| DashboardShortcutsHelp | DashboardFocused | GT33 |
| DashboardExit | DashboardFocused | GT33 |
| DashboardToggleAutoApprove | DashboardFocused | GT33 |
| DashboardOpenLocationPicker | DashboardFocused | GT33 |
| DashboardToggleWorktree | DashboardFocused | GT33 |
| DashboardOverlayExit | DashboardOverlay | GT33 |
| DashboardOverlayPrev | DashboardOverlay | GT33 |
| DashboardOverlayNext | DashboardOverlay | GT33 |
| DashboardOverlayStop | DashboardOverlay | GT33 |

## 欢迎页独立入口

实际启动观察显示四个菜单项及反馈提示；旧设置导入仅有条件源码入口，尚未证明运行时可达。下表是规划映射，不是功能通过。

| 入口 | 触发 | 用例族 | 资格 |
| --- | --- | --- | --- |
| New worktree | Ctrl+W | TL23 | observed-display |
| Resume session | Ctrl+R | SE03 | observed-display |
| Changelog | menu | UI16 | observed-display |
| Quit | Ctrl+Q | UI15 | observed-display |
| Import Claude settings | conditional menu / declared shortcut | GT27 | source-conditional; runtime reachability unqualified |
| /feedback hint | welcome advertisement | GT10 | observed-advertisement; slash command disabled |
