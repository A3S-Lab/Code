# A3S Code Python SDK Tests

## Directory Structure

```
tests/
├── integration/          # Integration tests (require real LLM API)
│   ├── test_real_with_kimi.py
│   └── test_real_integration.py
└── unit/                 # Unit tests (mock-based, no API required)
    ├── test_subagent_config.py
    ├── test_all_attributes.py
    ├── test_tool_kind.py
    ├── test_orchestrator_integration.py
    ├── test_final_verification.py
    ├── test_end_to_end.py
    └── test_rust_layer.py
```

## Running Tests

### Unit Tests (No API Key Required)

```bash
cd tests/unit
python test_subagent_config.py
python test_all_attributes.py
python test_tool_kind.py
```

### Integration Tests (Requires API Key)

Integration tests require environment variables:

```bash
export KIMI_API_KEY="your-api-key"
export KIMI_BASE_URL="https://api.moonshot.cn/v1"

cd tests/integration
python test_real_with_kimi.py
```

## Security Notes

- **NEVER commit API keys** to the repository
- Integration tests use environment variables for credentials
- Use `.env` files locally (add to `.gitignore`)
- CI/CD should use GitHub Secrets for API keys

## Test Coverage

- **SubAgentConfig**: skill_dirs parameter passing
- **Skill System**: kind: tool, kind: instruction support
- **Orchestrator**: Sub-agent execution and skill loading
- **End-to-End**: Full workflow from Python SDK → Rust core → LLM
