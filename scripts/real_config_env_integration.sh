#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WORKSPACE="$ROOT/crates/code"
CONFIG_FILE="${A3S_CONFIG_FILE:-$ROOT/.a3s/config.acl}"
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
ENV_EXPORT_FILE=""
cleanup_env_config() {
  if [ -n "$ENV_CONFIG_FILE" ] && [ -f "$ENV_CONFIG_FILE" ]; then
    rm -f "$ENV_CONFIG_FILE"
  fi
  if [ -n "$ENV_EXPORT_FILE" ] && [ -f "$ENV_EXPORT_FILE" ]; then
    rm -f "$ENV_EXPORT_FILE"
  fi
}
trap cleanup_env_config EXIT

prepare_env_config() {
  local generated
  generated="$(python3 - "$CONFIG_FILE" <<'PY'
import os
import re
import shlex
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

env_fd, env_path = tempfile.mkstemp(prefix="a3s-code-config-env-", suffix=".sh")
with os.fdopen(env_fd, "w") as handle:
    if api_key:
        handle.write("export A3S_OPENAI_API_KEY=" + shlex.quote(api_key) + "\n")
    if base_url:
        handle.write("export A3S_OPENAI_BASE_URL=" + shlex.quote(base_url) + "\n")
os.chmod(env_path, stat.S_IRUSR | stat.S_IWUSR)

paths = []
if api_key:
    paths.append("has_api_key=1")
if base_url:
    paths.append("has_base_url=1")
paths.append("env_file=" + env_path)
paths.append("config_file=" + path)
print("\n".join(paths))
PY
)"

  local entry
  while IFS= read -r entry; do
    case "$entry" in
      env_file=*)
        ENV_EXPORT_FILE="${entry#env_file=}"
        ;;
      config_file=*)
        ENV_CONFIG_FILE="${entry#config_file=}"
        ;;
    esac
  done < <(printf '%s\n' "$generated")

  if [ -n "$ENV_EXPORT_FILE" ] && [ -f "$ENV_EXPORT_FILE" ]; then
    # shellcheck disable=SC1090
    . "$ENV_EXPORT_FILE"
  fi
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
