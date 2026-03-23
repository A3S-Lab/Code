# Running a3s-code on SkillsBench

This document explains how the current `a3s-code` SkillsBench integration works and how to run it locally with Harbor.

## Architecture

The current integration uses Harbor's installed-agent path rather than a long-running external agent service.

Execution flow:

1. Build the control image from `crates/code`.
2. Harbor imports `skillsbench.harbor_a3s_agent:A3SCode`.
3. `A3SCode` uploads an install script into the benchmark environment.
4. The install script creates `/opt/a3s-code-venv`, installs `a3s-code`, and writes `a3s_code_runner.py`.
5. Harbor executes the runner inside the task environment.
6. The runner loads `.a3s/config.hcl`, creates an `a3s_code.Agent`, opens a session, and sends the SkillsBench instruction.
7. The runner emits a machine-readable JSON payload to stdout.
8. `A3SCode.populate_context_post_run()` reads that payload and maps token counts and metadata back into Harbor.

Key files:

- [`harbor_a3s_agent.py`](/Users/roylin/Desktop/code/a3s/crates/code/integrations/skillsbench/harbor_a3s_agent.py)
- [`a3s_code_runner.py`](/Users/roylin/Desktop/code/a3s/crates/code/integrations/skillsbench/a3s_code_runner.py)
- [`install_agent.sh.j2`](/Users/roylin/Desktop/code/a3s/crates/code/integrations/skillsbench/install_agent.sh.j2)
- [`Dockerfile`](/Users/roylin/Desktop/code/a3s/crates/code/integrations/skillsbench/Dockerfile)

## What Harbor Does

`A3SCode` is a Harbor `BaseInstalledAgent`.

At setup/run time it:

- sets default env vars such as `A3S_CODE_CONFIG=/workspace/.a3s/config.hcl`
- passes the benchmark instruction through `A3S_CODE_INSTRUCTION`
- uploads `skills_dir` into `/workspace/.skillsbench-skills` when Harbor provides a local skills directory
- forwards Harbor MCP server definitions through `A3S_CODE_MCP_SERVERS_JSON`
- runs:

```bash
. /opt/a3s-code-venv/bin/activate && python /installed-agent/a3s_code_runner.py
```

## What the Runner Does

`a3s_code_runner.py` executes one SkillsBench task inside the benchmark container.

It:

- reads `A3S_CODE_INSTRUCTION`
- reads `A3S_CODE_CONFIG` and `A3S_CODE_WORKSPACE`
- creates an `a3s_code.Agent`
- opens a session with:

```text
builtin_skills = true
planning = true
permissive = true
```

- applies `skill_dirs` if present
- adds MCP servers if present
- calls `session.send(instruction)`
- prints a JSON result payload wrapped with:

```text
A3S_CODE_RESULT_BEGIN
A3S_CODE_RESULT_END
```

## Local Prerequisites

You need:

- local Docker
- Harbor CLI or Harbor Python environment
- a valid `.a3s/config.hcl`
- model credentials in environment variables

For the Kimi setup currently used in this repo, the config typically reads:

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

By default the benchmark container expects the config at:

```text
/workspace/.a3s/config.hcl
```

Override with:

```bash
export A3S_CODE_CONFIG=/path/to/config.hcl
```

## Build the Image

Run from `crates/code`:

```bash
docker build -f integrations/skillsbench/Dockerfile -t a3s-code-skillsbench .
```

## Minimal Smoke Checks

Verify the integration module loads:

```bash
docker run --rm a3s-code-skillsbench python -c "from skillsbench.harbor_a3s_agent import A3SCode; print(A3SCode.name())"
```

Verify command generation:

```bash
docker run --rm a3s-code-skillsbench python -c "from pathlib import Path; from skillsbench.harbor_a3s_agent import A3SCode; agent=A3SCode(logs_dir=Path('/tmp/logs')); cmd=agent.create_run_agent_commands('ping')[0]; print(cmd.command); print(cmd.cwd)"
```

## Run SkillsBench with Harbor

Make the integration importable:

```bash
export PYTHONPATH=/opt/a3s-code-skillsbench:$PYTHONPATH
```

If you are running Harbor from the container image, make sure the same module path is available in that runtime.

Set model credentials:

```bash
export KIMI_API_KEY=your_key
export KIMI_BASE_URL=your_base_url
```

Run the full SkillsBench dataset:

```bash
harbor run \
  -d skillsbench \
  --agent-import-path skillsbench.harbor_a3s_agent:A3SCode \
  -m openai/kimi-k2.5
```

Run a single SkillsBench task:

```bash
harbor run \
  -p datasets/skillsbench/<task_id> \
  --agent-import-path skillsbench.harbor_a3s_agent:A3SCode \
  -m openai/kimi-k2.5
```

## Useful Environment Variables

Runtime variables passed into the benchmark task:

- `A3S_CODE_CONFIG`
- `A3S_CODE_WORKSPACE`
- `A3S_CODE_BUILTIN_SKILLS`
- `A3S_CODE_PLANNING`
- `A3S_CODE_PERMISSIVE`
- `A3S_CODE_SKILL_DIRS_JSON`
- `A3S_CODE_MCP_SERVERS_JSON`

Installation variables:

- `A3S_CODE_VERSION`
- `A3S_CODE_PIP_SPEC`

Examples:

```bash
export A3S_CODE_VERSION=1.5.3
```

```bash
export A3S_CODE_PIP_SPEC='git+https://github.com/A3S-Lab/Code.git@main#subdirectory=crates/code/sdk/python'
```

## Important Notes

- The benchmark install script uses `rustup` with the stable toolchain to avoid failures from older distro-packaged Rust versions.
- If Harbor provides a local `skills_dir`, it is uploaded into `/workspace/.skillsbench-skills` before task execution.
- MCP server definitions are forwarded into `a3s-code` and then registered with `session.add_mcp_server(...)`.
- The current integration returns final metadata and token counts, but does not yet implement ATIF trajectory export.
