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
TEST_FILTER="${REAL_PROVIDER_TEST_FILTER:-}"

if [ "$MODE" != "real" ] && [ "$MODE" != "--dry-run" ]; then
  echo "usage: $0 [--dry-run]" >&2
  exit 2
fi

if [ ! -f "$CONFIG_FILE" ]; then
  echo "config file not found: $CONFIG_FILE" >&2
  exit 1
fi

if [ "${A3S_CODE_ACL_ENV_PREPARED:-0}" != "1" ]; then
  exec bash "$SCRIPT_DIR/with_acl_env.sh" run "$CONFIG_FILE" bash "$SCRIPT_DIR/real_config_env_integration.sh" "$MODE"
fi

cd "$WORKSPACE"

run_acl_env_tests() {
  local extra_flag="$1"
  local command=(cargo test -p a3s-code-core --test test_real_config_env_integration)
  if [ -n "${TARGET_DIR:-}" ]; then
    command+=(--target-dir "$TARGET_DIR")
  fi
  if [ -n "$TEST_FILTER" ]; then
    command+=("$TEST_FILTER")
  fi
  command+=(--)
  if [ -n "$extra_flag" ]; then
    command+=("$extra_flag")
  fi
  command+=(--nocapture --test-threads=1)
  "${command[@]}"
}

export A3S_CONFIG_FILE="$CONFIG_FILE"
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
  echo "Configure credentials for the default provider, or inject compatible A3S_OPENAI_* / MINIMAX_* variables, then rerun this script." >&2
  exit 2
fi

run_acl_env_tests "--ignored"
