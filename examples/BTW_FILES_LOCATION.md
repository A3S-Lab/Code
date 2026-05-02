# BTW 功能测试文件位置

测试文件已成功移动到正确的位置。

## 文件位置

### Node.js SDK 示例
- **配置文件**: `crates/code/sdk/node/examples/agent_btw_test.acl`
- **完整测试**: `crates/code/sdk/node/examples/test_btw_feature.ts`
- **简单测试**: `crates/code/sdk/node/examples/test_btw_simple.ts`
- **方法检查**: `crates/code/sdk/node/examples/check_btw.cjs`

### 文档
- **测试指南**: `crates/code/examples/BTW_TEST_GUIDE.md`
- **测试报告**: `crates/code/examples/BTW_TEST_REPORT.md`

## 运行测试

### 方法 1: 使用环境变量

```bash
cd crates/code/sdk/node/examples

# 设置环境变量
export KIMI_API_KEY="your-api-key"
export KIMI_BASE_URL="your-base-url"

# 运行简单测试
npx ts-node test_btw_simple.ts

# 运行完整测试
npx ts-node test_btw_feature.ts
```

### 方法 2: 从配置文件自动读取

```bash
cd crates/code/sdk/node/examples

# 从 a3s 配置文件读取并设置环境变量
export KIMI_API_KEY=$(grep -o '"apiKey"\s*=\s*"sk-[^"]*"' ../../../../../.a3s/config.acl | head -1 | sed 's/.*"\(sk-[^"]*\)".*/\1/')
export KIMI_BASE_URL=$(grep -o '"baseUrl"\s*=\s*"[^"]*"' ../../../../../.a3s/config.acl | head -1 | sed 's/.*"\([^"]*\)".*/\1/')

# 运行测试
npx ts-node test_btw_simple.ts
```

### 方法 3: 检查 btw 方法是否存在

```bash
cd crates/code/sdk/node/examples
export KIMI_API_KEY="..." && export KIMI_BASE_URL="..."
node check_btw.cjs
```

## 验证结果

btw 方法已成功实现并导出：

```
btw method exists: function
btw is function: true
Session methods: [
  ...,
  'btw',
  ...
]
```

## 配置文件示例

`agent_btw_test.acl` 使用 `env()` 函数从环境变量读取敏感信息：

```acl
default_model = "openai/kimi-k2.5"

providers "openai" {
  api_key  = env("KIMI_API_KEY")      # 从环境变量读取
  base_url = env("KIMI_BASE_URL")     # 从环境变量读取

  models "kimi-k2.5" {
    name      = "KIMI K2.5"
    family    = "kimi"
    tool_call = true

    limit {
      context = 256000
      output  = 8192
    }
  }
}

storage_backend = "memory"
max_tool_rounds = 20
```

## 最佳实践

1. ✅ 使用 `env()` 函数在配置文件中引用环境变量
2. ✅ 不要在代码或配置文件中硬编码 API key
3. ✅ 测试文件放在 SDK 的 examples 目录中
4. ✅ 文档放在 crates/code/examples 目录中

## 清理

以下文件已从根目录删除：
- ❌ `test_btw.ts`
- ❌ `test_btw.js`
- ❌ `test_btw_env.ts`
- ❌ `test_config.acl`
- ❌ `setup_test_env.sh`
- ❌ `tsconfig.json`
- ❌ `BTW_TEST_GUIDE.md`
- ❌ `BTW_TEST_REPORT.md`
