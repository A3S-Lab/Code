# A3S Code Tests

This directory contains integration tests for the a3s-code project.

## Test Files

- `test_ahp_safety.py` - AHP safety harness integration tests
- `test_issue7.py` - Tests for issue #7 (dynamic agent registration and MCP timeout)

## Running Tests

```bash
# Run AHP safety tests
python3 tests/test_ahp_safety.py

# Run issue #7 tests
python3 tests/test_issue7.py
```

## Configuration

Tests require an agent configuration file. Create `agent.test.hcl` in the project root:

```hcl
default_model = "anthropic/claude-sonnet-4-20250514"

providers {
  name    = "anthropic"
  api_key = env("ANTHROPIC_API_KEY")
}
```

Or use the example: `cp agent.example.hcl agent.test.hcl` and add your API key.

## SDK Tests

SDK-specific tests are located in their respective directories:
- Node.js SDK: `sdk/node/examples/test_*.ts`
- Python SDK: `sdk/python/examples/test_*.py`

These are kept in the examples directories as they also serve as usage examples.
