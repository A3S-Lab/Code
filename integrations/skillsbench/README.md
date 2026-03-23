# a3s-code SkillsBench Docker Integration

This directory provides a Harbor-compatible `a3s-code` agent that can be used to run the SkillsBench benchmark.

The integration consists of:

- `harbor_a3s_agent.py`: a custom Harbor `BaseInstalledAgent` wrapper
- `a3s_code_runner.py`: the in-environment runner that executes one SkillsBench task with `a3s-code`
- `install_agent.sh.j2`: the install script uploaded by Harbor into the benchmark container
- `Dockerfile`: a control-plane image with Harbor, `a3s-code`, and the custom agent module installed

## What it does

Harbor loads `A3SCode`, uploads the install script into the benchmark environment, installs `a3s-code` in a Python virtualenv, writes the runner script, and executes the benchmark instruction with:

- `builtin_skills = true`
- `planning = true`
- `permissive = true`

The runner emits a JSON payload to stdout, and `populate_context_post_run()` maps token counts and final metadata back into Harbor.

## Build

Build from `crates/code`:

```bash
docker build -f integrations/skillsbench/Dockerfile -t a3s-code-skillsbench .
```

## Required runtime environment

The benchmark environment must have access to your A3S Code config and model credentials.

Default config path inside the task environment:

```text
/workspace/.a3s/config.hcl
```

This matches the existing repo convention. If your config lives elsewhere, set:

```bash
export A3S_CODE_CONFIG=/path/to/config.hcl
```

For the Kimi setup you have been using, your config can continue to read credentials from environment variables, for example:

```hcl
default_model = "openai/kimi-k2.5"

providers {
  name     = "openai"
  api_key  = env("KIMI_API_KEY")
  base_url = env("KIMI_BASE_URL")

  models {
    id   = "kimi-k2.5"
    name = "Kimi K2.5"
  }
}
```

## Harbor usage

Make the module importable:

```bash
export PYTHONPATH=/opt/a3s-code-skillsbench:$PYTHONPATH
```

Then instantiate the agent from Harbor code:

```python
from pathlib import Path

from skillsbench.harbor_a3s_agent import A3SCode

agent = A3SCode(logs_dir=Path("./logs"))
```

Or run it directly from Harbor CLI:

```bash
harbor run \
  -d skillsbench \
  --agent-import-path skillsbench.harbor_a3s_agent:A3SCode \
  -m openai/kimi-k2.5
```

For a single local task:

```bash
harbor run \
  -p datasets/skillsbench/<task_id> \
  --agent-import-path skillsbench.harbor_a3s_agent:A3SCode \
  -m openai/kimi-k2.5
```

## Important knobs

These env vars are passed into the benchmark run:

- `A3S_CODE_CONFIG`: config path inside the benchmark container. Default: `/workspace/.a3s/config.hcl`
- `A3S_CODE_WORKSPACE`: workspace path. Default: `/workspace`
- `A3S_CODE_BUILTIN_SKILLS`: default `true`
- `A3S_CODE_PLANNING`: default `true`
- `A3S_CODE_PERMISSIVE`: default `true`
- `A3S_CODE_SKILL_DIRS_JSON`: JSON array of skill directories passed through to `agent.session(..., skill_dirs=...)`
- `A3S_CODE_MCP_SERVERS_JSON`: JSON-serialized Harbor MCP server definitions mapped to `session.add_mcp_server(...)`

These env vars affect installation:

- `A3S_CODE_VERSION`: install `a3s-code==<version>`
- `A3S_CODE_PIP_SPEC`: override the pip install target completely

Examples:

```bash
export A3S_CODE_VERSION=1.5.3
```

```bash
export A3S_CODE_PIP_SPEC='git+https://github.com/A3S-Lab/Code.git@main#subdirectory=crates/code/sdk/python'
```

Use `A3S_CODE_PIP_SPEC` when you need the benchmark environment to install from a specific Git revision instead of PyPI.

## Notes

- The Docker image handles standalone `crates/code` builds by rewriting local path dependencies in `core/Cargo.toml` and `sdk/python/Cargo.toml` before installing the Python SDK.
- The current integration uses Harbor's installed-agent path, not an external long-running RPC service.
- The benchmark install script uses `rustup` with the stable toolchain instead of Debian's packaged Rust, which avoids source-build failures seen with older `rustc` versions.
- `skills_dir` is handled explicitly in `setup()`: if Harbor provides a local skills directory, the agent uploads it into the task container at `/workspace/.skillsbench-skills` and passes that path to `a3s-code`.
- `mcp_servers` are forwarded into `a3s-code` via `session.add_mcp_server(...)`.
- ATIF trajectory export is not implemented in this first version; Harbor still receives final metadata and token counts.
