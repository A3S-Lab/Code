# A3S Code · 真实 TUI · 15 秒

这是对 `../20261012-15s/` 的修订。上一版错误地使用了通用终端界面。
本版使用实际 A3S Code TUI 画面：欢迎页、会话页眉、用户消息、工具记录、
回答、底部输入框、provider/model 栏、快捷键与 `/` 命令菜单。

播放 `preview.html` 或 `a3s-code-tui-launch-15s.mp4`。
`tui-preview.png` 是成片中实际 TUI 的一帧；`poster.png` 是品牌封面。

## 画面来源

- 本地真实 pager：`apps/code/vendor/a3s-code-ui/target/debug/a3s-code-tui`，
  版本 `9.1.3 (439cc740f252)`。
- 真实 Agent：`apps/code/target/debug/a3s-code-acp`，版本 `9.1.3`。
- 使用项目已有 ACL，通过既有 `acl_env_config` 帮助程序在子进程中注入凭据；
  凭据没有进入视频、参数或素材文件。
- 在本目录自有示例工作区中读取 `title.rs`，实际模型为
  `boyue/DeepSeek V4 Flash (high)`。
- 实际任务：「读取 title.rs，用三条要点说明空输入逻辑，每条不超过20字。」
- 原始记录：`capture/01-ready.vt`、`02-draft.vt`、`03-response.vt`、
  `04-palette.vt`、`all-output.vt`。
- `vt_cells.rs` 用现有 vt100 解析器生成单元格数据，`tui-data.js` 保留实际
  110×35 网格。动画只操作镜头、逐字显现、引导线与结尾品牌。
  字体是嵌入的 Geist Mono、Noto Sans SC 与 DejaVu 符号子集；
  私用区分支符号按终端 glyph 画在原单元格内。
- `capture/native-result.json` 保留实际会话耗时、字节数和输出时间索引。
  画面里的 `Worked for 8.2s` 来自该会话；视频压缩了等待时间。

## 验证与复现

`capture.acl` 使用 a3s-test 重跑同一条真实 TUI 流程。首次采集时对欢迎页
可见文字的假设错误，导致等候超时；按观察到的实际文案修正后，最终报告为
`passed`，所有 11 个步骤通过。见 `capture/report.json`。
视频采用另一次同版本真实会话的完整原始 VT，以避免截取未完成的一次绘制。

`composition-check.json` 检查 451 个时间样本与重复 seek 的像素一致性。
`probe/` 和 `verification.log` 检查连续承接、快门、节奏、音量与完整取景。
`media-info.json` 是实际 MP4 的编码信息。

最终成片检查结果：PASS。15.000 秒，1920×1080，30 fps，450 帧；
音频峰值 −4.9 dBFS，0 个削波采样。声明的 TUI、输入框、菜单、字标
与标语在各自取景窗口内均完整可见。连续性探针检出的 1 个边界有承接物；
其余镜头另按编码后的关键帧检查。

在本目录运行：

```sh
/Users/roylin/.local/share/onetake/venv/bin/python prepare.py
/Users/roylin/.local/share/onetake/venv/bin/python score.py
/Users/roylin/.local/share/onetake/venv/bin/python /Users/roylin/.agents/skills/onetake/scripts/render.py film.html --out a3s-code-tui-launch-15s.mp4 --sfx score.wav --workers 8 --gap 6 --samples-min 4 --samples-max 48
/Users/roylin/.local/share/onetake/venv/bin/python /Users/roylin/.agents/skills/onetake/scripts/verify_promo.py a3s-code-tui-launch-15s.mp4 --comp film.html --shots 0,0.8,1.45,3.5,4.25,4.8,5.35,7.9,9.35,10.45,12
```

所有播放素材均可离线使用。源代码与字体许可随附。
本版是 1080p30 审阅视频，没有提交、推送、对外发布或渲染 4K60。

## 明确不宣称

本片没有演示修复代码或运行测试，也不宣称完整产品测试通过。
界面来源是本地真实 9.1.3 构建，不是对所有发行渠道二进制的一致性证明。
镜头引导线和品牌变形是视频动效，不是 TUI 新增功能。
