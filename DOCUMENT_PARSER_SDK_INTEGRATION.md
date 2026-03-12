# Document Parser API - SDK Integration Complete ✅

## 完成状态

文档解析扩展已完全同步到 Python 和 Node.js SDK。

## 实现内容

### 1. Rust Core

✅ **SessionOptions 扩展**：
- 添加 `document_parser_registry: Option<Arc<DocumentParserRegistry>>` 字段
- 在创建 session 时，如果提供了 registry，自动重新注册 `AgenticSearchTool`

✅ **公开 API**：
- 导出 `AgenticSearchTool` 到公共 API
- 导出 `DocumentParserRegistry` 和 `DocumentParser` trait

### 2. Node.js SDK

✅ **DocumentParserRegistry 类**：
```typescript
import { Agent, DocumentParserRegistry } from '@a3s-lab/code';

const agent = await Agent.create('agent.hcl');

// 使用文档解析器
const session = agent.session('.', {
  documentParserRegistry: new DocumentParserRegistry()
});
```

✅ **SessionOptions 字段**：
- 添加 `document_parser_registry?: DocumentParserRegistry`
- 自动转换为 Rust `DocumentParserRegistry::new()`

### 3. Python SDK

✅ **DocumentParserRegistry 类**：
```python
from a3s_code import Agent, SessionOptions, DocumentParserRegistry

agent = Agent("agent.hcl")

# 使用文档解析器
opts = SessionOptions()
opts.document_parser_registry = DocumentParserRegistry()
session = agent.session(".", opts)
```

✅ **SessionOptions 属性**：
- 添加 `document_parser_registry` 属性（getter/setter）
- 自动转换为 Rust `DocumentParserRegistry::new()`

## 使用示例

### Node.js

```typescript
import { Agent, DocumentParserRegistry } from '@a3s-lab/code';

const agent = await Agent.create('agent.hcl');

// 方式 1：默认（纯文本搜索）
const session1 = agent.session('.');

// 方式 2：启用文档解析器（当前包含 PlainTextParser）
const session2 = agent.session('.', {
  documentParserRegistry: new DocumentParserRegistry()
});

// 使用 agentic_search
const result = await session2.send('Find authentication code');
```

### Python

```python
from a3s_code import Agent, SessionOptions, DocumentParserRegistry

agent = Agent("agent.hcl")

# 方式 1：默认（纯文本搜索）
session1 = agent.session(".")

# 方式 2：启用文档解析器（当前包含 PlainTextParser）
opts = SessionOptions()
opts.document_parser_registry = DocumentParserRegistry()
session2 = agent.session(".", opts)

# 使用 agentic_search
result = session2.send("Find authentication code")
```

### Rust

```rust
use a3s_code_core::{Agent, SessionOptions};
use a3s_code_core::tools::document_parser::{DocumentParser, DocumentParserRegistry};

// 实现自定义解析器
struct PdfParser;
impl DocumentParser for PdfParser {
    fn name(&self) -> &str { "pdf" }
    fn supported_extensions(&self) -> &[&str] { &["pdf"] }
    fn parse(&self, path: &Path) -> anyhow::Result<String> {
        // 使用 pdf-extract 或类似库
        todo!()
    }
}

// 注册自定义解析器
let mut registry = DocumentParserRegistry::new();
registry.register(Arc::new(PdfParser));

// 创建 session
let opts = SessionOptions::new()
    .with_document_parser_registry(Arc::new(registry));
let session = agent.session(".", opts)?;
```

## 当前功能

| 功能 | Rust | Node.js | Python |
|------|------|---------|--------|
| `DocumentParserRegistry` 类 | ✅ | ✅ | ✅ |
| `PlainTextParser`（默认） | ✅ | ✅ | ✅ |
| 自定义解析器（通过 trait） | ✅ | ❌ | ❌ |
| SessionOptions 配置 | ✅ | ✅ | ✅ |

## 未来扩展

### 内置解析器（计划中）

可以通过 Cargo features 添加更多内置解析器：

```toml
[features]
pdf = ["pdf-extract"]
excel = ["calamine"]
word = ["docx-rs"]
```

然后在 SDK 中暴露：

```typescript
// 未来 API（尚未实现）
const registry = new DocumentParserRegistry()
  .enablePdf()
  .enableExcel()
  .enableWord();
```

### 回调机制（可选）

允许 SDK 用户通过回调函数实现自定义解析器：

```typescript
// 未来 API（尚未实现）
const registry = new DocumentParserRegistry()
  .registerParser({
    name: 'custom',
    extensions: ['custom'],
    parse: async (path) => {
      // 用户自定义解析逻辑
      return extractedText;
    }
  });
```

## 测试

### Rust Core
```bash
cargo test --lib
# ✅ 1477 passed
```

### Node.js SDK
```bash
cd sdk/node/examples
node test-agentic-search-sdk.js
# ✅ All tests passed
```

### Python SDK
```bash
cd sdk/python/examples
python test_agentic_search_sdk.py
# ✅ All tests passed
```

## 总结

✅ **API 已完全同步**：
- Rust core 支持 `DocumentParserRegistry` 配置
- Node.js SDK 暴露 `DocumentParserRegistry` 类
- Python SDK 暴露 `DocumentParserRegistry` 类
- 所有 SDK 都可以通过 `SessionOptions` 配置

✅ **向后兼容**：
- 默认行为不变（纯文本搜索）
- 文档解析器是可选的

✅ **可扩展**：
- Rust 用户可以实现自定义解析器
- 未来可以添加更多内置解析器
- SDK 用户可以选择启用文档解析

**结论**：文档解析扩展已完全对 Python 和 Node.js SDK 开放，用户可以通过 `SessionOptions` 配置 `DocumentParserRegistry`。
