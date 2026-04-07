# Configuration Files

Example `.hcl` configuration files for the A3S Code SDK.

## Files

- `test_config.hcl` - Minimal test configuration (uses env vars)
- `agent_kimi_k2.5.hcl` - Configuration for Kimi K2.5 model
- `agent_btw_test.hcl` - Configuration for BTW testing

## Usage

```bash
# Set environment variables
export OPENAI_API_KEY="your-api-key"
export OPENAI_BASE_URL="http://your-endpoint/v1/"

# Use in code
const agent = await Agent.create('configs/test_config.hcl');
```
