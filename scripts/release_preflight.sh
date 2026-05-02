#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WORKSPACE="$ROOT/crates/code"

cd "$WORKSPACE"

echo "[1/11] Checking patch hygiene"
git diff --check

echo "[2/11] Checking release version consistency"
scripts/check_release_versions.sh

echo "[3/11] Checking formatting"
cargo fmt --all --check

echo "[4/11] Running core library tests"
cargo test -p a3s-code-core --lib

echo "[5/11] Running core integration tests"
cargo test -p a3s-code-core --tests

echo "[6/11] Running AHP feature tests"
cargo test -p a3s-code-core --features ahp --test test_ahp_idle_with_llm

echo "[7/11] Running Node SDK smoke tests"
(cd sdk/node && npm test && npm run test:helpers)

echo "[8/11] Type-checking Node examples"
(cd sdk/node/examples && npm run typecheck)

echo "[9/11] Compiling Python SDK shim"
PYTHONPYCACHEPREFIX="${PYTHONPYCACHEPREFIX:-/private/tmp/a3s-code-pycache}" \
  python3 -m compileall sdk/python/python >/dev/null

echo "[10/11] Checking ACL env injection dry run"
scripts/real_config_env_integration.sh --dry-run

echo "[11/11] Checking real-provider ACL env smoke availability"
CONFIG_FILE="${A3S_CONFIG_FILE:-$ROOT/.a3s/config.acl}"
CONFIG_HAS_LITERAL_OPENAI_CREDS=0
if [ -f "$CONFIG_FILE" ]; then
  CONFIG_HAS_LITERAL_OPENAI_CREDS="$(
    python3 - "$CONFIG_FILE" <<'PY'
import re
import sys
from pathlib import Path

text = Path(sys.argv[1]).read_text()
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

if [ -n "${A3S_OPENAI_API_KEY:-${MINIMAX_API_KEY:-}}" ] && [ -n "${A3S_OPENAI_BASE_URL:-${MINIMAX_BASE_URL:-}}" ]; then
  scripts/real_config_env_integration.sh
elif [ "$CONFIG_HAS_LITERAL_OPENAI_CREDS" = "1" ]; then
  scripts/real_config_env_integration.sh
elif [ "${REQUIRE_REAL_PROVIDER:-0}" = "1" ]; then
  echo "missing A3S_OPENAI_* / MINIMAX_* variables or literal openai credentials in config; real-provider smoke is required" >&2
  exit 2
else
  echo "skipped real-provider smoke; inject A3S_OPENAI_*, MINIMAX_*, or literal openai config credentials before tagging" >&2
fi

echo "release preflight completed"
