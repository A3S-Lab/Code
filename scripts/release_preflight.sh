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
# Debug Go-bridge / tokio worker stacks overflow the default on macOS; CI
# exports the same floor for cargo test --workspace.
RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}"
export RUST_MIN_STACK

# a3s-vec is pure Rust; no native FTS sidecar staging is required.
TARGET_ROOT="${CARGO_TARGET_DIR:-$WORKSPACE/target}"
if [[ "$TARGET_ROOT" != /* ]]; then
  TARGET_ROOT="$WORKSPACE/$TARGET_ROOT"
fi

echo "[1/15] Checking patch hygiene"
git diff --check

echo "[2/15] Checking release version consistency"
scripts/check_release_versions.sh

echo "[3/15] Checking formatting"
cargo fmt --all --check

echo "[4/15] Checking strict Rust lints"
rustup run 1.97.1 cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

echo "[5/15] Checking SDK API alignment"
node scripts/generate_event_protocol_artifacts.mjs --check
node scripts/generate_evaluation_protocol_artifacts.mjs --check
node scripts/check_evaluation_protocol_artifacts.mjs
python3 scripts/check_scoped_capability_architecture.py
python3 scripts/check_capability_verification.py
node scripts/sdk_api_alignment_check.mjs
node sdk/node/scripts/patch-loader.mjs --check

echo "[6/15] Checking rustdoc warnings"
RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --all-features --no-deps

echo "[7/15] Running default Rust test suite"
cargo test --workspace

echo "[8/15] Running feature-gated library tests"
cargo test --workspace --all-features --lib

echo "[9/15] Building and testing Node SDK"
(
  unset A3S_CONFIG_FILE A3S_OPENAI_API_KEY A3S_OPENAI_BASE_URL MINIMAX_API_KEY MINIMAX_BASE_URL
  cd sdk/node && npm run build:debug && npm test && npm run test:helpers
)

echo "[10/15] Type-checking Node examples"
(cd sdk/node/examples && npm run typecheck)

echo "[11/15] Compiling Python SDK shim"
PYTHONPYCACHEPREFIX="${PYTHONPYCACHEPREFIX:-/private/tmp/a3s-code-pycache}" \
  python3 -m compileall sdk/python/python >/dev/null

echo "[12/15] Running Go SDK bridge integration"
cargo build --package a3s-code-go-bridge --bin a3s-code-go-bridge --features advanced-harness,server
BRIDGE_BINARY="$TARGET_ROOT/debug/a3s-code-go-bridge"
if [ -f "$BRIDGE_BINARY.exe" ]; then
  BRIDGE_BINARY="$BRIDGE_BINARY.exe"
fi
(
  cd sdk/go
  # Debug builds of the Go bridge can overflow the default tokio worker stack
  # on macOS; keep local preflight aligned with a larger Rust thread stack.
  A3S_CODE_GO_BRIDGE_TEST_BINARY="$BRIDGE_BINARY" \
  RUST_MIN_STACK="${RUST_MIN_STACK:-16777216}" \
    go test -race -tags=release_contract ./...
)

echo "[13/15] Checking ACL env injection dry run"
A3S_CONFIG_FILE="$CONFIG_FILE" scripts/real_config_env_integration.sh --dry-run

echo "[14/15] Checking real-provider ACL env smoke availability"
CONFIG_HAS_DEFAULT_PROVIDER_CREDS=0
if [ -f "$CONFIG_FILE" ]; then
  CONFIG_HAS_DEFAULT_PROVIDER_CREDS="$(bash scripts/with_acl_env.sh check "$CONFIG_FILE")"
fi

if [ "${SKIP_REAL_PROVIDER:-0}" = "1" ]; then
  if [ "${REQUIRE_REAL_PROVIDER:-0}" = "1" ]; then
    echo "SKIP_REAL_PROVIDER and REQUIRE_REAL_PROVIDER cannot both be enabled" >&2
    exit 2
  fi
  echo "skipped real-provider smoke by explicit SKIP_REAL_PROVIDER=1" >&2
  echo "[15/15] Skipping SDK real-provider smoke"
elif [ "$CONFIG_HAS_DEFAULT_PROVIDER_CREDS" = "1" ]; then
  A3S_CONFIG_FILE="$CONFIG_FILE" scripts/real_config_env_integration.sh
  echo "[15/15] Running SDK real-provider smoke"
  A3S_CONFIG_FILE="$CONFIG_FILE" scripts/sdk_real_config_env_integration.sh
elif [ "${REQUIRE_REAL_PROVIDER:-0}" = "1" ]; then
  echo "missing credentials for the configured default provider; real-provider smoke is required" >&2
  exit 2
else
  echo "skipped real-provider smoke; configure credentials for the default provider before tagging" >&2
  echo "[15/15] Skipping SDK real-provider smoke"
fi

echo "release preflight completed"
