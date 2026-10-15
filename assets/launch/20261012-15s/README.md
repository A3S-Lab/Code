# A3S Code · 光标贯穿

此版已作废：其中的通用终端界面不符合真实 A3S Code TUI。
请使用 [真实 TUI 修订版](../20261012-tui-15s/preview.html)。

用户选择的方案 1：同一个蓝色光标依次成为输入位置、终端边框、代码定位线、
测试下划线与 A3S 标志。15 秒，中文，16:9，1920×1080，30 fps。

打开 `preview.html` 播放，或直接使用 `a3s-code-launch-15s.mp4`。
`poster.png` 是封面，`film.html?play` 是可编辑动画源稿。
所有字体均已嵌入；播放与源稿均不依赖网络。

## 交付文件

- `a3s-code-launch-15s.mp4`：H.264 + AAC，48 kHz 双声道。
- `film.html`、`motion.js`、`look.js`：确定性动画及字体。
- `score.py`、`sfx_palette.py`、`events.json`、`score.wav`：原创音乐与动作音效。
- `CONCEPT.md`：概念、节奏与画面承接设计。
- `verification.log`、`composition-check.json`、`probe/`：检查记录。
- `encoded-contact-sheet.png`：从实际 MP4 提取的关键帧。
- `SOUND-CREDITS.md`、`ONETAKE-LICENSE.txt`、`fonts/`：来源与许可证。

## 验证

2026-10-12，onetake 的成片检查结果为 **PASS**：

| 检查 | 结果 |
|---|---|
| 时长与编码 | 15.000 秒，450 帧，1920×1080，30 fps，H.264 / AAC |
| 节奏差异 | CV 0.62 |
| 画面停顿 | 阈值测得静止占比 76.7%，最长低变化区间 5 秒 |
| 音频 | 峰值 −5.0 dBFS，0 个削波采样，静音区间占比 58.7% |
| 自动连续性 | 检出的 2 个场景边界均有承接物，得分 1.00 |
| 快速移动 | 180° 快门，在线性光空间平均子帧 |
| 构图 | 声明的光标、字标、标语在各自窗口内均未出框 |
| 确定性 | 451 个时间样本无 seek 错误；重复 seek 的像素完全相同 |
| 示例代码 | 空字符串、纯空白、普通标题，共 3 个测试通过 |

自动连续性只检查探针检出的边界。关键帧另外经过视觉检查。
参考片只用于事前节奏研究，没有要求复制其爆发段或故事。
本片以 1080p30 交付供审阅，未经确认不渲染 4K60。

## 内容边界

终端按用户提供的 A3S Code TUI 截图重建，界面显示「示例工作区」。
代码与三个测试来自 `demo.rs`，已经实际运行；这是视觉演示，
不构成完整产品测试结果，也不宣称为真实模型会话的录屏。
没有加入版本号、性能或稳定性承诺。

## 复现

本机 onetake 安装位置：`/Users/roylin/.agents/skills/onetake`。
使用其隔离 Python 环境执行以下命令，在本目录运行：

```sh
/Users/roylin/.local/share/onetake/venv/bin/python prepare.py
/Users/roylin/.local/share/onetake/venv/bin/python score.py
/Users/roylin/.local/share/onetake/venv/bin/python /Users/roylin/.agents/skills/onetake/scripts/render.py film.html --out a3s-code-launch-15s.mp4 --sfx score.wav --workers 8 --gap 6 --samples-min 4 --samples-max 48
/Users/roylin/.local/share/onetake/venv/bin/python /Users/roylin/.agents/skills/onetake/scripts/verify_promo.py a3s-code-launch-15s.mp4 --comp film.html --shots 0,0.4,2.25,3.05,3.7,4.35,6.65,7.8,9.35,10.4,10.65,12
```

更新文案后需要用 onetake `look.py apply` 重新生成字体子集。
许可证见随附文件；未提交、推送或对外发布本视频。
