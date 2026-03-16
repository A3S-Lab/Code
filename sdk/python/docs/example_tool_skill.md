---
name: scoring-video-adapter
description: "视频评分适配器 - 用于处理视频评分任务"
kind: tool
allowed-tools: "mcp_video-processor_(*), mcp_longvt__(*), Bash(*), Read(*), Write(*)"
---
# Scoring Video Adapter

这是一个视频评分适配器 skill，使用 `kind: tool` 类型。

## 功能

- 调用视频处理器 MCP 工具
- 使用 LongVT 进行视频分析
- 执行必要的文件操作

## Skill Kind: tool

`tool` 类型的 skill 会像 `instruction` 类型一样被注入到系统提示中，但语义上表示这是一个工具型的 skill，提供专门的功能。

## 使用方法

调用此 skill 时，请提供以下信息：
- 视频文件路径
- 评分标准
- 输出格式要求

## 示例

```
Call Skill('scoring-video-adapter', {
  "video_path": "/path/to/video.mp4",
  "criteria": "quality, content, engagement",
  "output_format": "json"
})
```

## 注意事项

- 确保 MCP 工具已正确配置
- 视频文件必须可访问
- 支持的视频格式：mp4, avi, mov
