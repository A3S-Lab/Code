#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE="$(cd "$SCRIPT_DIR/.." && pwd)"
export TMPDIR="${A3S_CODE_TEST_TMPDIR:-/private/tmp}"
PYTHON_BIN="${A3S_CODE_PYTHON:-}"
if [ -z "$PYTHON_BIN" ]; then
  if [ -x "$WORKSPACE/sdk/python/.venv/bin/python" ]; then
    PYTHON_BIN="$WORKSPACE/sdk/python/.venv/bin/python"
  else
    PYTHON_BIN="python3"
  fi
fi
CONFIG_ROOT="$WORKSPACE"
for candidate in "$WORKSPACE/../.." "$WORKSPACE" "$WORKSPACE/../../.."; do
  candidate="$(cd "$candidate" 2>/dev/null && pwd || true)"
  if [ -n "$candidate" ] && [ -f "$candidate/.a3s/config.acl" ]; then
    CONFIG_ROOT="$candidate"
    break
  fi
done
CONFIG_FILE="${A3S_CONFIG_FILE:-$CONFIG_ROOT/.a3s/config.acl}"

if [ ! -f "$CONFIG_FILE" ]; then
  echo "config file not found: $CONFIG_FILE" >&2
  exit 1
fi

if [ "${A3S_CODE_ACL_ENV_PREPARED:-0}" != "1" ]; then
  exec bash "$SCRIPT_DIR/with_acl_env.sh" run "$CONFIG_FILE" bash "$SCRIPT_DIR/sdk_real_config_env_integration.sh"
fi

missing=0
for name in A3S_OPENAI_API_KEY A3S_OPENAI_BASE_URL; do
  if [ -z "${!name:-}" ]; then
    echo "missing required environment variable: $name" >&2
    missing=1
  fi
done
if [ "$missing" -ne 0 ]; then
  echo "Configure credentials for the default provider, or inject compatible A3S_OPENAI_* / MINIMAX_* variables, then rerun this script." >&2
  exit 2
fi

export A3S_CONFIG_FILE="$CONFIG_FILE"

echo "[1/4] Type-checking Node SDK examples"
(cd "$WORKSPACE/sdk/node/examples" && npm run typecheck)

echo "[2/4] Running Node SDK real-provider smoke"
(cd "$WORKSPACE/sdk/node/examples" && node basic/test_real_config_env_sdk.mjs)

echo "[3/4] Checking Python SDK import"
"$PYTHON_BIN" - <<'PY'
try:
    import a3s_code
except Exception as exc:
    raise SystemExit(
        "Python SDK import failed. Build/install it first, e.g. "
        "run `python sdk/python/scripts/clean_native_artifacts.py` and "
        "`maturin develop --locked --manifest-path sdk/python/Cargo.toml`, "
        "then rerun this script.\n"
        f"Import error: {exc}"
    )

missing = [
    name
    for name in ("register_dynamic_workflow_runtime", "unregister_dynamic_tool")
    if not hasattr(a3s_code.Session, name)
]
if missing:
    location = getattr(a3s_code, "__file__", "<unknown>")
    raise SystemExit(
        "Python SDK import resolved to a build that is missing current API "
        f"{', '.join(missing)}. Build/install the workspace SDK first, e.g. "
        "run `python sdk/python/scripts/clean_native_artifacts.py` and "
        "`maturin develop --locked --manifest-path sdk/python/Cargo.toml`, "
        "then rerun this script.\n"
        f"Imported from: {location}"
    )
PY

echo "[4/4] Running Python SDK real-provider smoke"
(cd "$WORKSPACE" && "$PYTHON_BIN" sdk/python/tests/real_config_env_sdk.py)
