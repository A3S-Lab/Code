# A3S Code · 60 秒真实 TUI 发布视频

沿用用户已选的「光标贯穿」。末尾使用 A3S Desktop 官方黑白图标，最后四秒静止。

## 播放

- `a3s-code-tui-launch-60s.mp4`：1920×1080、30 fps、60 秒、AAC 48 kHz 双声道。
- `preview.html`：本地播放器，含视频、片尾与界面截图入口。
- `film.html?play`：可重复渲染的动画源稿。
- `poster.png`：黑白 Logo 片尾。
- `tui-preview.png`、`encoded-contact-sheet.png`：从最终编码视频提取。

## 内容与来源

真实流程：欢迎页 → 输入只读任务 → 读取实现及测试 → 解释空白输入和五类测试覆盖 → 连续追问边界条件 → `/` 命令菜单 → `/model` 模型列表 → `/settings` 设置面板 → Desktop 黑白 Logo。无配音，含中文编辑字幕和原创音乐音效。

使用本机 A3S Code 9.1.3 原生 TUI / ACP 实际运行。`capture/`、`capture-ui/` 保留原生 VT 输出与转换后的终端单元；`vt_cells.rs` 按真实 110×35 网格转换为 `tui-data.js`。HTML 逐格绘制原始字符、前景和背景颜色，保留输入框、模型栏、工具记录、正文与菜单。输入逐字揭示，等待时间经过压缩。设置面板来自第二次真实原生捕获，按原始范围覆盖在第一会话的真实对话底板上。没有新增产品控件或编造输出。

片尾的 `brand/a3s-desktop-icon.png` 逐字节复制自 `apps/work/icons/app/icon.png`，同样被 Desktop bundle 配置引用。SHA-256：`3b443c5adf6014615b2d42ddf866d84df3e420512ef8813f61b7561e4d9525a3`。

## 验证

- A3S Test：`capture.acl`，2 个场景、20 个步骤通过，覆盖读代码结果、命令菜单、模型列表、设置面板；见 `capture/report.json`。
- 两次原生只读回合分别显示 `Worked for 7.0s` 与 `Worked for 10s`；见 `capture/03-response.json`、`capture/05-followup.json`。
- 1801 个 seek 采样，无页面错误；重复 seek 像素相同：`composition-check.json`。
- 10 个声明的可视范围全部无裁切：`probe.log`、`probe/tracks.json`。
- 真实 180° 快门，多子帧在线性光空间合成；见 `.render.json`。
- 最终编码、节奏、音量、连续性和曲线检查：`verification.log`。
- 字体及工具许可在 `fonts/`、`ONETAKE-LICENSE.txt`；声音说明见 `SOUND-CREDITS.md`。

## 留存的问题与未采用素材

最初尝试修改并执行测试：默认原生沙箱拒绝执行；精确测试命令在宿主路径返回通过后，回合仍因变更没有绑定 Passed 验证或宿主豁免而失败。证据在 `capture-attempt-without-host-contract/`。本片没有把这个失败回合包装成成功交付，没有修改生产代码、沙箱边界或完成门禁。

首次只读捕获中模型尝试列目录，需要执行确认；该捕获被取消，证据在 `capture-attempt-read-shell/`。改为直接使用 read 工具后，两个只读回合正常完成。主捕获最初退出模型列表后错误拼接 `/settings`，脚本因此超时；设置面板随后在独立原生会话成功捕获。首次 A3S Test 把可变措辞 `Unicode` 当成固定断言，改为文件中的固定值 `Untitled` 后全部通过；原失败报告留存为 `capture/report-spec-failure.json`。

## Explicitly not claimed

不声称修复了完成校验问题；不声称本片展示了通过门禁的修改/测试交付；不声称覆盖所有 TUI 功能或模型；不声称 4K60、真人旁白或已发布到外部平台。没有提交、推送或发布代码。

## 再生成

Python 环境：`/Users/roylin/.local/share/onetake/venv/bin/python`。

1. `capture_native.py`、`capture_ui.py` 获取参考，通过项目既有 ACL 凭据入口启动，密钥不进入日志或 argv。
2. 用 `vt_cells.rs` 转换 VT，更新 `tui-data.js`；改文本后运行 onetake `look.py apply` 并重新生成符号字体子集。
3. `prepare.py` 导出事件与检查纯函数；`score.py` 生成 60 秒音轨。
4. 用 onetake `stills.py`、`probe.py` 检查；`render.py film.html --out a3s-code-tui-launch-60s.mp4 --sfx score.wav --workers 8 --gap 6 --samples-min 4 --samples-max 48`。
5. `verify_promo.py ... --comp film.html`，并检查最终编码截图。

私有运行状态在被忽略的 `runtime/`、`workspace/.a3s/`、`.a3s-test/`。没有触碰其他并行任务的源码修改。
