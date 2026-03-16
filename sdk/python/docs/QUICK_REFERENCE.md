# Quick Reference: SubAgentConfig Fix

## Problem (Before Fix)

```python
from a3s_code import SubAgentConfig

cfg = SubAgentConfig(
    agent_type="my-agent",
    prompt="test",
    skill_dirs=["/tmp/skills"]  # ✓ Accepted by constructor
)

# ❌ But couldn't access it:
print(cfg.skill_dirs)           # AttributeError
hasattr(cfg, 'skill_dirs')      # False
```

## Solution (After Fix)

```python
from a3s_code import SubAgentConfig

cfg = SubAgentConfig(
    agent_type="my-agent",
    prompt="test",
    skill_dirs=["/tmp/skills"]
)

# ✅ Now works:
print(cfg.skill_dirs)           # ['/tmp/skills']
hasattr(cfg, 'skill_dirs')      # True
cfg.skill_dirs = ["/new/path"]  # Setter works too
```

## All Accessible Attributes

```python
cfg.agent_type          # str
cfg.prompt              # str
cfg.description         # str
cfg.permissive          # bool
cfg.permissive_deny     # list[str]
cfg.max_steps           # int | None
cfg.timeout_ms          # int | None
cfg.parent_id           # str | None
cfg.workspace           # str
cfg.agent_dirs          # list[str]
cfg.skill_dirs          # list[str]  ← The original bug
cfg.lane_config         # SessionQueueConfig | None
```

## Rebuild Instructions

```bash
cd crates/code/sdk/python
python3 -m pip install -e .
```

## Verify Fix

```bash
python3 test_final_verification.py
```

Expected output: `✅ ALL TESTS PASSED!`

## Technical Details

- **Root cause**: Missing `#[getter]` and `#[setter]` attributes in PyO3 bindings
- **Fix location**: `crates/code/sdk/python/src/lib.rs` line ~4162
- **Lines added**: ~130 lines (24 methods for 12 fields)
- **Backward compatible**: Yes (only adds functionality)
- **Runtime behavior change**: None (Rust layer was always correct)

## Version

- Fixed in: a3s-code 1.4.4 (with this patch)
- Affected versions: 1.4.3, 1.4.4 (before patch)
