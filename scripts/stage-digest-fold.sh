#!/usr/bin/env bash
# Stage only the Flow step identity digest-fold publish surface.
# Does not commit. From crates/code: ./scripts/stage-digest-fold.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

files=(
  CHANGELOG.md
  ROADMAP.md
  core/src/dynamic_workflow.rs
  core/src/dynamic_workflow/tests.rs
  scripts/stage-digest-fold.sh
)

for f in "${files[@]}"; do
  if [[ ! -f "$f" ]]; then
    echo "error: missing $f" >&2
    exit 1
  fi
done

echo "=== digest-fold unit tests ==="
cargo test -p a3s-code-core --features dynamic-workflow --lib \
  dynamic_flow_step_identity_ --quiet

git add -- "${files[@]}"
git status --short -- "${files[@]}"
echo
echo "Staged digest-fold surface only. Review, then commit when authorized."
echo "Suggested subject:"
echo "  fix(core): fold large Flow step inputs for DeepResearch multi-source"
echo
echo "After push, in crates/cli:"
echo "  ./scripts/bump-core-digest-fold-pin.sh <published-rev>"
echo "  ./scripts/verify-capability-regression.sh --require-published"
echo "  # then commit CLI pin when authorized"
