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

if [ -z "${A3S_OPENAI_API_KEY:-}" ] && [ -n "${MINIMAX_API_KEY:-}" ]; then
  export A3S_OPENAI_API_KEY="$MINIMAX_API_KEY"
fi

if [ -z "${A3S_OPENAI_BASE_URL:-}" ] && [ -n "${MINIMAX_BASE_URL:-}" ]; then
  export A3S_OPENAI_BASE_URL="$MINIMAX_BASE_URL"
fi

ENV_CONFIG_FILE=""
cleanup_env_config() {
  if [ -n "$ENV_CONFIG_FILE" ] && [ -f "$ENV_CONFIG_FILE" ]; then
    rm -f "$ENV_CONFIG_FILE"
  fi
}
trap cleanup_env_config EXIT

generated="$(python3 - "$CONFIG_FILE" <<'PY'
import os
import re
import stat
import sys
import tempfile
from pathlib import Path

source = Path(sys.argv[1])
text = source.read_text()
provider = re.search(r'(providers\s+"openai"\s*\{)(.*?)(\n\})', text, re.S)
if not provider:
    print("openai provider not found in config", file=sys.stderr)
    sys.exit(1)

body = provider.group(2)

def literal_value(names):
    for name in names:
        match = re.search(rf'\b{name}\s*=\s*"([^"]*)"', body)
        if match:
            return match.group(1)
    return None

api_key = os.environ.get("A3S_OPENAI_API_KEY") or literal_value(("apiKey", "api_key"))
base_url = os.environ.get("A3S_OPENAI_BASE_URL") or literal_value(("baseUrl", "base_url"))

rewritten_body = re.sub(
    r'\b(apiKey|api_key)\s*=\s*(?:"[^"]*"|env\("[^"]+"\))',
    'apiKey = env("A3S_OPENAI_API_KEY")',
    body,
    count=1,
)
rewritten_body = re.sub(
    r'\b(baseUrl|base_url)\s*=\s*(?:"[^"]*"|env\("[^"]+"\))',
    'baseUrl = env("A3S_OPENAI_BASE_URL")',
    rewritten_body,
    count=1,
)
if rewritten_body == body:
    rewritten_body = '\n  apiKey = env("A3S_OPENAI_API_KEY")\n  baseUrl = env("A3S_OPENAI_BASE_URL")' + body

rewritten = text[:provider.start(2)] + rewritten_body + text[provider.end(2):]

fd, config_path = tempfile.mkstemp(prefix="a3s-code-sdk-config-env-", suffix=".acl")
with os.fdopen(fd, "w") as handle:
    handle.write(rewritten)
os.chmod(config_path, stat.S_IRUSR | stat.S_IWUSR)

if api_key:
    print("api_key=" + api_key)
if base_url:
    print("base_url=" + base_url)
print("config_file=" + config_path)
PY
)"

while IFS= read -r entry; do
  case "$entry" in
    api_key=*) export A3S_OPENAI_API_KEY="${entry#api_key=}" ;;
    base_url=*) export A3S_OPENAI_BASE_URL="${entry#base_url=}" ;;
    config_file=*) ENV_CONFIG_FILE="${entry#config_file=}" ;;
  esac
done < <(printf '%s\n' "$generated")

missing=0
for name in A3S_OPENAI_API_KEY A3S_OPENAI_BASE_URL; do
  if [ -z "${!name:-}" ]; then
    echo "missing required environment variable: $name" >&2
    missing=1
  fi
done
if [ "$missing" -ne 0 ]; then
  echo "Inject A3S_OPENAI_* or MINIMAX_* variables, then rerun this script." >&2
  exit 2
fi

export A3S_CONFIG_FILE="${ENV_CONFIG_FILE:-$CONFIG_FILE}"

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
        "`cd sdk/python && maturin develop`, then rerun this script.\n"
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
        "`cd sdk/python && maturin develop`, then rerun this script.\n"
        f"Imported from: {location}"
    )
PY

echo "[4/4] Running Python SDK real-provider smoke"
(cd "$WORKSPACE" && "$PYTHON_BIN" sdk/python/tests/real_config_env_sdk.py)
