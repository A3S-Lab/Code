#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
WORKSPACE="$ROOT/crates/code"
CONFIG_FILE="${A3S_CONFIG_FILE:-$ROOT/.a3s/config.acl}"
TARGET_DIR="${TARGET_DIR:-$WORKSPACE/target/coverage-kimi}"
PROF_DIR="${PROF_DIR:-$TARGET_DIR/profraw}"
PROFDATA="${PROFDATA:-$TARGET_DIR/a3s-code-core.profdata}"
LLVM_PROFDATA_BIN="${LLVM_PROFDATA_BIN:-$(xcrun --find llvm-profdata)}"
LLVM_COV_BIN="${LLVM_COV_BIN:-$(xcrun --find llvm-cov)}"
REPORT_ONLY="${REPORT_ONLY:-0}"

mkdir -p "$PROF_DIR"
if [ "$REPORT_ONLY" != "1" ]; then
  rm -f "$PROF_DIR"/*.profraw "$PROFDATA"
fi

export CARGO_INCREMENTAL=0
export LLVM_PROFILE_FILE="$PROF_DIR/%p-%m.profraw"
export RUSTFLAGS="${RUSTFLAGS:-} -C instrument-coverage -C codegen-units=1 -C debuginfo=0"
export A3S_CONFIG_FILE="$CONFIG_FILE"

if [ "$REPORT_ONLY" != "1" ]; then
  echo "[1/4] Running a3s-code-core lib tests"
  cargo test -p a3s-code-core --lib --manifest-path "$WORKSPACE/Cargo.toml" --target-dir "$TARGET_DIR"

  echo "[2/4] Running real provider ACL env integration smoke test"
  "$ROOT/crates/code/scripts/real_config_env_integration.sh"
else
  echo "[1/4] Skipping test execution (REPORT_ONLY=1)"
  echo "[2/4] Reusing existing coverage artifacts"
fi

echo "[3/4] Merging raw profiles"
"$LLVM_PROFDATA_BIN" merge -sparse "$PROF_DIR"/*.profraw -o "$PROFDATA"

echo "[4/4] Computing coverage"
OBJECTS=()
while IFS= read -r obj; do
  OBJECTS+=("$obj")
done < <(
  find "$TARGET_DIR/debug" -type f \
    \( -path "*/deps/*" -o -path "*/examples/*" \) \
    -perm -111 \
    ! -name "*.d" \
    ! -name "*.dylib" \
    ! -name "*.rlib" \
    ! -name "*.rmeta" \
    | sort
)

if [ "${#OBJECTS[@]}" -eq 0 ]; then
  echo "no instrumented objects found" >&2
  exit 1
fi

REPORT_ARGS=("${OBJECTS[0]}")
for obj in "${OBJECTS[@]:1}"; do
  REPORT_ARGS+=(-object "$obj")
done

"$LLVM_COV_BIN" report \
  "${REPORT_ARGS[@]}" \
  --instr-profile "$PROFDATA" \
  --ignore-filename-regex='(/\.cargo/registry/|/rustc/)' \
  --use-color
