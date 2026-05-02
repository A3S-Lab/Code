# Chart Generator Skill - 快速入门

## 5 分钟上手指南

### 步骤 1: 确认文件已创建

确保以下文件存在：

```bash
# Skill 定义
crates/code/examples/skills/chart-generator.md

# Agent 配置示例
crates/code/examples/agent-chart-generator.acl

# 测试脚本
crates/code/examples/test_chart_generator.py
```

### 步骤 2: 使用 Python SDK

```python
from a3s_code import Agent

# 创建 agent（使用示例配置）
agent = Agent.create("examples/agent-chart-generator.acl")

# 创建 session
session = agent.session(".")

# 生成图表
result = session.send("""
Create a line chart showing monthly sales:
Jan: 45000, Feb: 52000, Mar: 48000, Apr: 61000
Use vis-chart format.
""")

print(result.text)
```

### 步骤 3: 查看输出

AI 会生成如下格式的输出：

````markdown
I'll create a line chart to visualize the monthly sales trend:

```vis-chart
{
  "type": "line",
  "data": [
    { "time": "Jan", "value": 45000 },
    { "time": "Feb", "value": 52000 },
    { "time": "Mar", "value": 48000 },
    { "time": "Apr", "value": 61000 }
  ]
}
```

The chart shows sales fluctuating between $45K-$61K, with a peak in February.
````

### 步骤 4: 在 SafeClaw 中查看

将上述输出复制到 SafeClaw 的聊天界面，图表会自动渲染为交互式可视化。

---

## 常见用例

### 用例 1: 从文件生成图表

```python
result = session.send("Read sales.json and create a bar chart of top products")
```

### 用例 2: 从命令输出生成图表

```python
result = session.send("""
Run 'git log --oneline --since="1 month ago" | wc -l' for each week
and show a trend chart of commit activity
""")
```

### 用例 3: 多图表仪表板

```python
result = session.send("""
Create a dashboard with:
1. Line chart: Monthly revenue trend
2. Pie chart: Product category distribution
3. Bar chart: Top 5 customers by sales
""")
```

### 用例 4: 自动选择图表类型

```python
result = session.send("""
Analyze the data in metrics.json and create the most
appropriate visualization to show key insights.
""")
```

---

## 支持的图表类型速查

| 图表类型 | 英文名 | 适用场景 | 数据格式 |
|---------|--------|---------|---------|
| 折线图 | line | 时间序列、趋势 | `{ "time": "...", "value": ... }` |
| 条形图 | bar | 分类对比、排名 | `{ "category": "...", "value": ... }` |
| 饼图 | pie | 占比、百分比 | `{ "category": "...", "value": ... }` |
| 面积图 | area | 数量变化、累积 | `{ "time": "...", "value": ... }` |
| 散点图 | scatter | 相关性、分布 | `{ "x": ..., "y": ..., "size": ... }` |
| 柱状图 | column | 分组对比 | `{ "category": "...", "series": "...", "value": ... }` |
| 热力图 | heatmap | 矩阵数据、密度 | `{ "x": "...", "y": "...", "value": ... }` |
| 雷达图 | radar | 多维对比 | `{ "dimension": "...", "value": ... }` |

---

## 提示词技巧

### ✅ 好的提示词

```
"Create a line chart showing quarterly revenue: Q1=100, Q2=120, Q3=150, Q4=180"
"Visualize the data in sales.json as a bar chart"
"Show me a pie chart of browser market share"
"Generate a trend chart for the last 6 months"
```

### ❌ 不好的提示词

```
"Make a chart"  # 太模糊，没有指定数据
"Show data"     # 没有说明图表类型
"Visualize"     # 缺少数据来源
```

### 💡 最佳实践

1. **明确数据来源**："from sales.json", "using this data: ..."
2. **指定图表类型**："line chart", "bar chart", "pie chart"
3. **提供上下文**："to show trends", "to compare products"
4. **要求格式**："use vis-chart format"（如果 AI 不自动使用）

---

## 故障排查

### 问题：AI 不生成 vis-chart 格式

**解决方案**：
```python
# 方法 1: 明确要求格式
result = session.send("Create a chart using vis-chart markdown format")

# 方法 2: 显式调用 skill
result = session.send("/chart-generator Show sales trend")

# 方法 3: 提供示例
result = session.send("""
Create a chart like this example:
\`\`\`vis-chart
{"type": "line", "data": [...]}
\`\`\`
""")
```

### 问题：生成的 JSON 格式错误

**解决方案**：
```python
# 要求验证 JSON
result = session.send("""
Create a vis-chart and validate the JSON syntax before outputting
""")
```

### 问题：图表类型选择不当

**解决方案**：
```python
# 明确指定图表类型
result = session.send("Create a LINE chart (not bar or pie) showing...")
```

---

## 下一步

1. **运行测试**：`python examples/test_chart_generator.py`
2. **查看文档**：`docs/a3s-code-vis-chart-integration.md`
3. **自定义 skill**：编辑 `examples/skills/chart-generator.md`
4. **集成到项目**：在你的 `agent.acl` 中启用 skill

---

## 相关资源

- **Skill 定义**：`crates/code/examples/skills/chart-generator.md`
- **完整文档**：`docs/a3s-code-vis-chart-integration.md`
- **SafeClaw 渲染器**：`apps/safeclaw/src/components/custom/memoized-markdown/vis-chart.tsx`
- **图表示例**：`apps/safeclaw/docs/vis-chart-examples.md`
- **GPT-Vis 官网**：https://gpt-vis.antv.vision
