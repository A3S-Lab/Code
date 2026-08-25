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
unset A3S_CONFIG_FILE

raise_open_file_limit() {
  local soft_limit hard_limit target_limit
  soft_limit="$(ulimit -Sn)"
  hard_limit="$(ulimit -Hn)"
  target_limit=10240

  if [ "$soft_limit" = "unlimited" ]; then
    return
  fi
  case "$soft_limit" in
    ''|*[!0-9]*)
      echo "unable to determine the open-file soft limit: $soft_limit" >&2
      exit 2
      ;;
  esac

  if [ "$hard_limit" != "unlimited" ]; then
    case "$hard_limit" in
      ''|*[!0-9]*)
        echo "unable to determine the open-file hard limit: $hard_limit" >&2
        exit 2
        ;;
    esac
    if [ "$hard_limit" -lt "$target_limit" ]; then
      target_limit="$hard_limit"
    fi
  fi

  if [ "$soft_limit" -lt "$target_limit" ]; then
    if ! ulimit -Sn "$target_limit"; then
      echo "failed to raise the open-file soft limit to $target_limit" >&2
      exit 2
    fi
    echo "raised open-file soft limit from $soft_limit to $target_limit"
  fi
}

cd "$WORKSPACE"
raise_open_file_limit

echo "[1/13] Checking patch hygiene"
git diff --check

echo "[2/13] Checking release version consistency"
scripts/check_release_versions.sh

echo "[3/13] Checking formatting"
cargo fmt --all --check

echo "[4/13] Checking SDK API alignment"
node scripts/generate_event_protocol_artifacts.mjs --check
node scripts/sdk_api_alignment_check.mjs
node sdk/node/scripts/patch-loader.mjs --check

echo "[5/13] Running default Rust test suite"
cargo test --workspace

echo "[6/13] Running feature-gated library tests"
cargo test --workspace --all-features --lib

echo "[7/13] Building and testing Node SDK"
(
  unset A3S_CONFIG_FILE A3S_OPENAI_API_KEY A3S_OPENAI_BASE_URL MINIMAX_API_KEY MINIMAX_BASE_URL
  cd sdk/node && npm run build:debug && npm test && npm run test:helpers
)

echo "[8/13] Type-checking Node examples"
(cd sdk/node/examples && npm run typecheck)

echo "[9/13] Compiling Python SDK shim"
PYTHONPYCACHEPREFIX="${PYTHONPYCACHEPREFIX:-/private/tmp/a3s-code-pycache}" \
  python3 -m compileall sdk/python/python >/dev/null

echo "[10/13] Running Go SDK bridge integration"
cargo build --package a3s-code-go-bridge --bin a3s-code-go-bridge
TARGET_ROOT="${CARGO_TARGET_DIR:-$WORKSPACE/target}"
if [[ "$TARGET_ROOT" != /* ]]; then
  TARGET_ROOT="$WORKSPACE/$TARGET_ROOT"
fi
BRIDGE_BINARY="$TARGET_ROOT/debug/a3s-code-go-bridge"
if [ -f "$BRIDGE_BINARY.exe" ]; then
  BRIDGE_BINARY="$BRIDGE_BINARY.exe"
fi
(
  cd sdk/go
  A3S_CODE_GO_BRIDGE_TEST_BINARY="$BRIDGE_BINARY" \
    go test ./...
)

echo "[11/13] Checking ACL env injection dry run"
A3S_CONFIG_FILE="$CONFIG_FILE" scripts/real_config_env_integration.sh --dry-run

echo "[12/13] Checking real-provider ACL env smoke availability"
CONFIG_HAS_DEFAULT_PROVIDER_CREDS=0
if [ -f "$CONFIG_FILE" ]; then
  CONFIG_HAS_DEFAULT_PROVIDER_CREDS="$(
    python3 - "$CONFIG_FILE" <<'PY'
import os
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text(encoding="utf-8")
default_model = re.search(r'^\s*default_model\s*=\s*"([^"]+)"', text, re.M)
if not default_model or "/" not in default_model.group(1):
    print(0)
    raise SystemExit

provider_name = default_model.group(1).split("/", 1)[0]
provider = re.search(
    rf'providers\s+"{re.escape(provider_name)}"\s*\{{(.*?)\n\}}',
    text,
    re.S,
)
if not provider:
    print(0)
    raise SystemExit

body = provider.group(1)
provider_env = re.sub(r"[^A-Za-z0-9]", "_", provider_name).upper()

def configured_value(names):
    for name in names:
        match = re.search(
            rf'\b{name}\s*=\s*(?:"([^"]*)"|env\("([^"]+)"\))',
            body,
        )
        if match:
            if match.group(1) is not None:
                return match.group(1)
            return os.environ.get(match.group(2))
    return None

has_key = bool(
    os.environ.get(f"A3S_{provider_env}_API_KEY")
    or os.environ.get("A3S_OPENAI_API_KEY")
    or os.environ.get("MINIMAX_API_KEY")
    or configured_value(("apiKey", "api_key"))
)
has_url = bool(
    os.environ.get(f"A3S_{provider_env}_BASE_URL")
    or os.environ.get("A3S_OPENAI_BASE_URL")
    or os.environ.get("MINIMAX_BASE_URL")
    or configured_value(("baseUrl", "base_url"))
)
print(1 if has_key and has_url else 0)
PY
  )"
fi

if [ "${SKIP_REAL_PROVIDER:-0}" = "1" ]; then
  if [ "${REQUIRE_REAL_PROVIDER:-0}" = "1" ]; then
    echo "SKIP_REAL_PROVIDER and REQUIRE_REAL_PROVIDER cannot both be enabled" >&2
    exit 2
  fi
  echo "skipped real-provider smoke by explicit SKIP_REAL_PROVIDER=1" >&2
  echo "[13/13] Skipping SDK real-provider smoke"
elif [ "$CONFIG_HAS_DEFAULT_PROVIDER_CREDS" = "1" ]; then
  A3S_CONFIG_FILE="$CONFIG_FILE" scripts/real_config_env_integration.sh
  echo "[13/13] Running SDK real-provider smoke"
  A3S_CONFIG_FILE="$CONFIG_FILE" scripts/sdk_real_config_env_integration.sh
elif [ "${REQUIRE_REAL_PROVIDER:-0}" = "1" ]; then
  echo "missing credentials for the configured default provider; real-provider smoke is required" >&2
  exit 2
else
  echo "skipped real-provider smoke; configure credentials for the default provider before tagging" >&2
  echo "[13/13] Skipping SDK real-provider smoke"
fi

echo "release preflight completed"
