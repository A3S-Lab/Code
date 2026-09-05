#!/usr/bin/env bash
set -euo pipefail

# Reproducible local production qualification for the framework-owned search
# path. This gate does not require MCP or a live provider request. The default
# path is intentionally focused for fast development feedback; pass --full for
# the release-only complete unit-test sweep.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE="$(cd "$SCRIPT_DIR/.." && pwd)"
FEATURES="${A3S_WORKSPACE_FEATURES:-zvec-rust-fts-bundled}"
FILES="${A3S_WORKSPACE_ACCEPTANCE_FILES:-512}"
QUERY_WORKERS="${A3S_WORKSPACE_ACCEPTANCE_QUERY_WORKERS:-8}"
FULL=0

case "${1:-}" in
  "") ;;
  --full) FULL=1 ;;
  *) echo "usage: $0 [--full]" >&2; exit 2 ;;
esac

cd "$WORKSPACE"

echo "[1/5] formatting and lockfile checks"
cargo fmt --all -- --check
git diff --check

echo "[2/5] focused native retrieval tests"
cargo test --locked -p a3s-code-core --features "$FEATURES" --lib \
  workspace::manifest::tests::automatic_persistent_retrieval_uses_portable_cold_admission -- --exact
cargo test --locked -p a3s-code-core --features "$FEATURES" --lib \
  workspace::retrieval::runtime::tests
cargo test --locked -p a3s-code-core --features "$FEATURES" --lib \
  workspace::retrieval::persistent::tests
cargo test --locked -p a3s-code-core --features "$FEATURES" --lib \
  tools::builtin::bm25::tests

echo "[3/5] focused portable retrieval tests"
cargo test --locked -p a3s-code-core --no-default-features --lib \
  workspace::retrieval::tests

if [ "$FULL" -eq 1 ]; then
  echo "[4/5] full native and portable unit suites"
  cargo test --locked -p a3s-code-core --features "$FEATURES" --lib
  cargo test --locked -p a3s-code-core --no-default-features --lib
else
  echo "[4/5] full suites skipped (use --full for release admission)"
fi

echo "[5/5] strict native lint and real workspace qualification"
cargo clippy --locked -p a3s-code-core --all-targets --features "$FEATURES" -- -D warnings
A3S_WORKSPACE_ACCEPTANCE_FILES="$FILES" \
A3S_WORKSPACE_ACCEPTANCE_QUERY_WORKERS="$QUERY_WORKERS" \
cargo run --locked --release -p a3s-code-core \
  --example workspace_persistent_index_production --features "$FEATURES"

echo "workspace search production qualification: pass"
