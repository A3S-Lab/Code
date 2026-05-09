#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE="$(cd "$SCRIPT_DIR/.." && pwd)"
CONFIG_ROOT="$WORKSPACE"
for candidate in "$WORKSPACE/../.." "$WORKSPACE" "$WORKSPACE/../../.."; do
  candidate="$(cd "$candidate" 2>/dev/null && pwd || true)"
  if [ -n "$candidate" ] && [ -f "$candidate/.a3s/config.acl" ]; then
    CONFIG_ROOT="$candidate"
    break
  fi
done
CONFIG_FILE="${A3S_CONFIG_FILE:-$CONFIG_ROOT/.a3s/config.acl}"
MODE="${1:-real}"

if [ "$MODE" != "real" ] && [ "$MODE" != "--dry-run" ]; then
  echo "usage: $0 [--dry-run]" >&2
  exit 2
fi

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

prepare_env_config() {
  local generated
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

if api_key:
    os.environ["A3S_OPENAI_API_KEY"] = api_key
if base_url:
    os.environ["A3S_OPENAI_BASE_URL"] = base_url

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
    # If the provider did not have credentials yet, insert env-based ones.
    rewritten_body = '\n  apiKey = env("A3S_OPENAI_API_KEY")\n  baseUrl = env("A3S_OPENAI_BASE_URL")' + body

rewritten = text[:provider.start(2)] + rewritten_body + text[provider.end(2):]

fd, path = tempfile.mkstemp(prefix="a3s-code-config-env-", suffix=".acl")
with os.fdopen(fd, "w") as handle:
    handle.write(rewritten)
os.chmod(path, stat.S_IRUSR | stat.S_IWUSR)

paths = []
if api_key:
    paths.append("api_key=" + api_key)
if base_url:
    paths.append("base_url=" + base_url)
paths.append("config_file=" + path)
print("\n".join(paths))
PY
)"

  local entry
  while IFS= read -r entry; do
    case "$entry" in
      api_key=*)
        export A3S_OPENAI_API_KEY="${entry#api_key=}"
        ;;
      base_url=*)
        export A3S_OPENAI_BASE_URL="${entry#base_url=}"
        ;;
      config_file=*)
        ENV_CONFIG_FILE="${entry#config_file=}"
        ;;
    esac
  done < <(printf '%s\n' "$generated")

}

prepare_env_config

cd "$WORKSPACE"

run_acl_env_tests() {
  local extra_flag="$1"
  if [ -n "${TARGET_DIR:-}" ]; then
    cargo test \
      -p a3s-code-core \
      --test test_real_config_env_integration \
      --target-dir "$TARGET_DIR" \
      -- \
      $extra_flag \
      --nocapture \
      --test-threads=1
  else
    cargo test \
      -p a3s-code-core \
      --test test_real_config_env_integration \
      -- \
      $extra_flag \
      --nocapture \
      --test-threads=1
  fi
}

export A3S_CONFIG_FILE="${ENV_CONFIG_FILE:-$CONFIG_FILE}"
export RUST_TEST_THREADS=1

if [ "$MODE" = "--dry-run" ]; then
  run_acl_env_tests ""
  exit 0
fi

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

run_acl_env_tests "--ignored"
