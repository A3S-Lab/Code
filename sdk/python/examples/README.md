# A3S Code Python SDK Examples

This directory contains examples and test scripts for the A3S Code Python SDK.

## Categories

### Basic Examples
- `test_simple.py` - Basic agent usage
- `test_simple_fixed.py` - Fixed version of basic example
- `test_apis.py` - API usage examples

### Advanced Features
- `test_advanced_features.py` - Advanced SDK features
- `test_custom_skills_agents.py` - Custom skills and agents
- `test_external_task_handler.py` - External task handling
- `test_parallel_processing.py` - Parallel processing

### Agent Teams & Orchestration
- `test_agent_teams.py` - Basic agent teams
- `test_agent_teams_comprehensive.py` - Comprehensive team examples
- `test_orchestrator_external_lane_kimi.py` - Orchestrator with external lane
- `test_run_team_kimi.py` - Team execution with Kimi
- `test_team_runner_create_kimi.py` - Team runner creation

### Skills & Tools
- `test_tool_kind.py` - Tool-type skills
- `skill_tool_example.py` - Skill tool usage
- `test_subagent_config.py` - SubAgent configuration

### Integration Tests (Require Real LLM API)
- `test_real_with_kimi.py` - Real integration test with Kimi
- `test_real_integration.py` - Full integration test
- `test_real_kimi.py` - Kimi model integration

### Unit Tests (Mock-based, No API Required)
- `test_all_attributes.py` - Attribute testing
- `test_orchestrator_integration.py` - Orchestrator integration
- `test_final_verification.py` - Final verification
- `test_end_to_end.py` - End-to-end testing
- `test_rust_layer.py` - Rust layer testing

## Running Examples

### Basic Examples (No API Key Required)

```bash
cd examples
python test_simple.py
python test_subagent_config.py
python test_tool_kind.py
```

### Integration Tests (Requires API Key)

Integration tests require environment variables:

```bash
export KIMI_API_KEY="your-api-key"
export KIMI_BASE_URL="https://api.moonshot.cn/v1"

cd examples
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
