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

cd "$WORKSPACE"

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

echo "[7/13] Running Node SDK smoke tests"
(
  unset A3S_CONFIG_FILE A3S_OPENAI_API_KEY A3S_OPENAI_BASE_URL MINIMAX_API_KEY MINIMAX_BASE_URL
  cd sdk/node && npm test && npm run test:helpers
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
(
  cd sdk/go
  A3S_CODE_GO_BRIDGE_TEST_BINARY="$TARGET_ROOT/debug/a3s-code-go-bridge" \
    go test ./...
)

echo "[11/13] Checking ACL env injection dry run"
scripts/real_config_env_integration.sh --dry-run

echo "[12/13] Checking real-provider ACL env smoke availability"
CONFIG_FILE="${A3S_CONFIG_FILE:-$CONFIG_ROOT/.a3s/config.acl}"
CONFIG_HAS_LITERAL_OPENAI_CREDS=0
if [ -f "$CONFIG_FILE" ]; then
  CONFIG_HAS_LITERAL_OPENAI_CREDS="$(
    python3 - "$CONFIG_FILE" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text(encoding="utf-8")
provider = re.search(r'providers\s+"openai"\s*\{(.*?)\n\}', text, re.S)
if not provider:
    print(0)
    raise SystemExit
body = provider.group(1)
has_key = re.search(r'\b(apiKey|api_key)\s*=\s*"[^"]+"', body) is not None
has_url = re.search(r'\b(baseUrl|base_url)\s*=\s*"[^"]+"', body) is not None
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
elif [ -n "${A3S_OPENAI_API_KEY:-${MINIMAX_API_KEY:-}}" ] && [ -n "${A3S_OPENAI_BASE_URL:-${MINIMAX_BASE_URL:-}}" ]; then
  scripts/real_config_env_integration.sh
  echo "[13/13] Running SDK real-provider smoke"
  scripts/sdk_real_config_env_integration.sh
elif [ "$CONFIG_HAS_LITERAL_OPENAI_CREDS" = "1" ]; then
  scripts/real_config_env_integration.sh
  echo "[13/13] Running SDK real-provider smoke"
  scripts/sdk_real_config_env_integration.sh
elif [ "${REQUIRE_REAL_PROVIDER:-0}" = "1" ]; then
  echo "missing A3S_OPENAI_* / MINIMAX_* variables or literal openai credentials in config; real-provider smoke is required" >&2
  exit 2
else
  echo "skipped real-provider smoke; inject A3S_OPENAI_*, MINIMAX_*, or literal openai config credentials before tagging" >&2
  echo "[13/13] Skipping SDK real-provider smoke"
fi

echo "release preflight completed"
