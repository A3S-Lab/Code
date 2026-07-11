# A3S Code - Justfile

default:
    @just --list

# ============================================================================
# Build
# ============================================================================

# Build the project
build:
    cargo build --workspace

# Build release
release:
    cargo build --workspace --release

# ============================================================================
# Test (unified command with progress display)
# ============================================================================

# Run all tests with progress display and module breakdown
test:
    #!/usr/bin/env bash
    set -e

    # Colors
    BOLD='\033[1m'
    GREEN='\033[0;32m'
    BLUE='\033[0;34m'
    CYAN='\033[0;36m'
    YELLOW='\033[0;33m'
    RED='\033[0;31m'
    DIM='\033[2m'
    RESET='\033[0m'

    # Counters
    TOTAL_PASSED=0
    TOTAL_FAILED=0
    TOTAL_IGNORED=0

    print_header() {
        echo ""
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
        echo -e "${BOLD}  $1${RESET}"
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    }

    # Extract module test counts from cargo test output
    extract_module_counts() {
        local output="$1"
        echo "$output" | grep -E "^test .+::.+ \.\.\. ok$" | \
            sed 's/^test \([^:]*\)::.*/\1/' | \
            sort | uniq -c | sort -rn | \
            while read count module; do
                printf "      ${DIM}%-20s %3d tests${RESET}\n" "$module" "$count"
            done
    }

    run_crate_tests() {
        local crate_name="$1"
        local display_name="$2"

        echo -ne "${CYAN}▶${RESET} ${BOLD}${display_name}${RESET} "

        if OUTPUT=$(cargo test -p "$crate_name" 2>&1); then
            TEST_EXIT=0
        else
            TEST_EXIT=1
        fi

        RESULT_LINES=$(echo "$OUTPUT" | grep -E "^test result:" || true)
        if [ -n "$RESULT_LINES" ]; then
            PASSED=$(echo "$RESULT_LINES" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+' | awk '{ total += $1 } END { print total + 0 }')
            FAILED=$(echo "$RESULT_LINES" | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+' | awk '{ total += $1 } END { print total + 0 }')
            IGNORED=$(echo "$RESULT_LINES" | grep -oE '[0-9]+ ignored' | grep -oE '[0-9]+' | awk '{ total += $1 } END { print total + 0 }')

            TOTAL_PASSED=$((TOTAL_PASSED + PASSED))
            TOTAL_FAILED=$((TOTAL_FAILED + FAILED))
            TOTAL_IGNORED=$((TOTAL_IGNORED + IGNORED))

            if [ "$FAILED" -gt 0 ]; then
                echo -e "${RED}✗${RESET} ${DIM}$PASSED passed, $FAILED failed${RESET}"
                echo "$OUTPUT" | grep -E "^test .* FAILED$" | sed 's/^/    /'
            else
                echo -e "${GREEN}✓${RESET} ${DIM}$PASSED passed${RESET}"
                if [ "$PASSED" -gt 10 ]; then
                    extract_module_counts "$OUTPUT"
                fi
            fi
        else
            if echo "$OUTPUT" | grep -q "error\[E"; then
                echo -e "${RED}✗${RESET} ${DIM}compile error${RESET}"
                echo "$OUTPUT" | grep -E "^error" | head -3 | sed 's/^/    /'
            elif [ "$TEST_EXIT" -ne 0 ]; then
                echo -e "${RED}✗${RESET} ${DIM}failed${RESET}"
            else
                echo -e "${YELLOW}○${RESET} ${DIM}no tests${RESET}"
            fi
        fi
    }

    print_header "🧪 A3S Code Test Suite"
    echo ""

    run_crate_tests "a3s-code-core" "a3s-code-core"

    # Summary
    echo ""
    echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"

    if [ "$TOTAL_FAILED" -gt 0 ]; then
        echo -e "  ${RED}${BOLD}✗ FAILED${RESET}  ${GREEN}$TOTAL_PASSED passed${RESET}  ${RED}$TOTAL_FAILED failed${RESET}  ${YELLOW}$TOTAL_IGNORED ignored${RESET}"
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
        exit 1
    else
        echo -e "  ${GREEN}${BOLD}✓ PASSED${RESET}  ${GREEN}$TOTAL_PASSED passed${RESET}  ${YELLOW}$TOTAL_IGNORED ignored${RESET}"
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    fi
    echo ""

# Run tests without progress (raw cargo output)
test-raw:
    cargo test --workspace

# Run tests with verbose output
test-v:
    cargo test --workspace -- --nocapture

# ============================================================================
# Test Subsets
# ============================================================================

# Run core tests only
test-core:
    cargo test -p a3s-code-core --lib

# Run queue and HITL tests
test-queue:
    cargo test -p a3s-code-core --lib -- queue::tests hitl::tests

# Run queue tests only
test-queue-only:
    cargo test -p a3s-code-core --lib -- queue::tests

# Run HITL tests only (hitl module)
test-hitl:
    cargo test -p a3s-code-core --lib -- hitl::tests

# Run HITL tests in agent loop (agent module)
test-agent-hitl:
    cargo test -p a3s-code-core --lib -- agent::tests::test_agent_hitl

# Run all HITL-related tests (hitl + agent)
test-hitl-all:
    cargo test -p a3s-code-core --lib -- hitl::tests agent::tests::test_agent_hitl

# Run session tests (includes queue/HITL integration)
test-session:
    cargo test -p a3s-code-core --lib -- session::tests

# Run agent tests
test-agent:
    cargo test -p a3s-code-core --lib -- agent::tests

# Run context provider tests
test-context:
    cargo test -p a3s-code-core --lib -- test_context test_agent_context test_session_context

# ============================================================================
# Coverage (requires: cargo install cargo-llvm-cov, brew install lcov)
# ============================================================================

# Test with coverage - shows real-time test progress + module coverage
test-cov:
    #!/usr/bin/env bash
    set -e

    # Colors
    BOLD='\033[1m'
    GREEN='\033[0;32m'
    BLUE='\033[0;34m'
    CYAN='\033[0;36m'
    YELLOW='\033[0;33m'
    RED='\033[0;31m'
    DIM='\033[2m'
    RESET='\033[0m'

    CLEAR_LINE='\033[2K'

    print_header() {
        echo ""
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
        echo -e "${BOLD}  $1${RESET}"
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    }

    print_header "🧪 A3S Code Test Suite with Coverage"
    echo ""
    echo -e "${CYAN}▶${RESET} ${BOLD}a3s-code-core${RESET}"
    echo ""

    tmp_dir="/tmp/test_cov_code_$$"
    mkdir -p "$tmp_dir"
    touch "$tmp_dir/module_counts"

    {
        cargo llvm-cov --workspace --lib 2>&1
    } | {
        total_passed=0
        total_failed=0

        while IFS= read -r line; do
            if [[ "$line" =~ ^test\ ([a-z_]+)::.*\.\.\.\ (ok|FAILED)$ ]]; then
                module="${BASH_REMATCH[1]}"
                result="${BASH_REMATCH[2]}"

                if [ "$result" = "ok" ]; then
                    total_passed=$((total_passed + 1))
                    count=$(grep "^${module} " "$tmp_dir/module_counts" 2>/dev/null | awk '{print $2}' || echo "0")
                    count=$((count + 1))
                    grep -v "^${module} " "$tmp_dir/module_counts" > "$tmp_dir/module_counts.tmp" 2>/dev/null || true
                    echo "$module $count" >> "$tmp_dir/module_counts.tmp"
                    mv "$tmp_dir/module_counts.tmp" "$tmp_dir/module_counts"
                else
                    total_failed=$((total_failed + 1))
                fi

                echo -ne "\r${CLEAR_LINE}      ${DIM}Running:${RESET} ${module}::... ${GREEN}${total_passed}${RESET} passed"
                [ "$total_failed" -gt 0 ] && echo -ne " ${RED}${total_failed}${RESET} failed"

            elif [[ "$line" =~ ^[[:space:]]*Compiling ]]; then
                echo -ne "\r${CLEAR_LINE}      ${DIM}Compiling...${RESET}"
            elif [[ "$line" =~ ^[[:space:]]*Running ]]; then
                echo -ne "\r${CLEAR_LINE}      ${DIM}Running tests...${RESET}"
            elif [[ "$line" =~ ^[a-z_]+.*\.rs[[:space:]] ]]; then
                echo "$line" >> "$tmp_dir/coverage_lines"
            elif [[ "$line" =~ ^TOTAL ]]; then
                echo "$line" >> "$tmp_dir/total_line"
            fi
        done

        echo "$total_passed" > "$tmp_dir/total_passed"
        echo "$total_failed" > "$tmp_dir/total_failed"
    }

    echo -ne "\r${CLEAR_LINE}"

    total_passed=$(cat "$tmp_dir/total_passed" 2>/dev/null || echo "0")
    total_failed=$(cat "$tmp_dir/total_failed" 2>/dev/null || echo "0")

    if [ "$total_failed" -gt 0 ]; then
        echo -e "      ${RED}✗${RESET} ${total_passed} passed, ${RED}${total_failed} failed${RESET}"
    else
        echo -e "      ${GREEN}✓${RESET} ${total_passed} tests passed"
    fi
    echo ""

    if [ -f "$tmp_dir/coverage_lines" ]; then
        awk '
        {
            file=$1; lines=$8; missed=$9
            n = split(file, parts, "/")
            if (n > 1) {
                module = parts[1]
            } else {
                gsub(/\.rs$/, "", file)
                module = file
            }
            total_lines[module] += lines
            total_missed[module] += missed
        }
        END {
            for (m in total_lines) {
                if (total_lines[m] > 0) {
                    covered = total_lines[m] - total_missed[m]
                    pct = (covered / total_lines[m]) * 100
                    printf "%s %.1f %d\n", m, pct, total_lines[m]
                }
            }
        }' "$tmp_dir/coverage_lines" | sort -t' ' -k2 -rn > "$tmp_dir/cov_agg"

        echo -e "      ${BOLD}Module               Tests   Coverage${RESET}"
        echo -e "      ${DIM}──────────────────────────────────────────────${RESET}"

        while read module pct lines; do
            tests=$(grep "^${module} " "$tmp_dir/module_counts" 2>/dev/null | awk '{print $2}' || echo "0")
            [ -z "$tests" ] && tests=0

            num=${pct%.*}
            if [ "$num" -ge 90 ]; then
                cov_color="${GREEN}${pct}%${RESET}"
            elif [ "$num" -ge 70 ]; then
                cov_color="${YELLOW}${pct}%${RESET}"
            else
                cov_color="${RED}${pct}%${RESET}"
            fi
            echo -e "      $(printf '%-18s' "$module") $(printf '%4d' "$tests")   ${cov_color} ${DIM}($lines lines)${RESET}"
        done < "$tmp_dir/cov_agg"

        if [ -f "$tmp_dir/total_line" ]; then
            total_cov=$(cat "$tmp_dir/total_line" | awk '{print $4}' | tr -d '%')
            total_lines=$(cat "$tmp_dir/total_line" | awk '{print $8}')
            echo -e "      ${DIM}──────────────────────────────────────────────${RESET}"

            num=${total_cov%.*}
            if [ "$num" -ge 90 ]; then
                cov_color="${GREEN}${BOLD}${total_cov}%${RESET}"
            elif [ "$num" -ge 70 ]; then
                cov_color="${YELLOW}${BOLD}${total_cov}%${RESET}"
            else
                cov_color="${RED}${BOLD}${total_cov}%${RESET}"
            fi
            echo -e "      ${BOLD}$(printf '%-18s' "TOTAL") $(printf '%4d' "$total_passed")${RESET}   ${cov_color} ${DIM}($total_lines lines)${RESET}"
        fi
    fi

    rm -rf "$tmp_dir"
    echo ""
    echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo ""

# Coverage with pretty terminal output
cov:
    #!/usr/bin/env bash
    set -e
    COV_FILE="/tmp/a3s-code-coverage.lcov"
    echo "┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓"
    echo "┃                    🧪 Running Tests with Coverage                     ┃"
    echo "┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛"
    cargo llvm-cov --workspace --lib --lcov --output-path "$COV_FILE" 2>&1 | grep -E "^test result"
    echo ""
    echo "┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓"
    echo "┃                         📊 Coverage Report                            ┃"
    echo "┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛"
    lcov --summary "$COV_FILE" 2>&1
    rm -f "$COV_FILE"

# Coverage for specific module
cov-module MOD:
    cargo llvm-cov -p a3s-code-core --lib -- {{MOD}}::

# Coverage with HTML report (opens in browser)
cov-html:
    cargo llvm-cov --workspace --lib --html --open

# Coverage with detailed file-by-file table
cov-table:
    cargo llvm-cov --workspace --lib

# Coverage for CI (generates lcov.info)
cov-ci:
    cargo llvm-cov --workspace --lib --lcov --output-path lcov.info

# ============================================================================
# Code Quality
# ============================================================================

# Format code
fmt:
    cargo fmt --all

# Lint (clippy)
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# CI checks (fmt + lint + test)
ci:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    cargo test --workspace --all-features --lib

# ============================================================================
# Utilities
# ============================================================================

# Clean build artifacts
clean:
    cargo clean

# Check project (fast compile check)
check:
    cargo check --workspace

# Watch and rebuild
watch:
    cargo watch -x 'build --workspace'

# Generate docs
doc:
    cargo doc --workspace --no-deps --open

# ============================================================================
# Publish
# ============================================================================

# Publish a3s-code-core to crates.io
publish:
    #!/usr/bin/env bash
    set -e

    BOLD='\033[1m'
    GREEN='\033[0;32m'
    BLUE='\033[0;34m'
    RED='\033[0;31m'
    DIM='\033[2m'
    RESET='\033[0m'

    print_step() { echo -e "${BLUE}▶${RESET} ${BOLD}$1${RESET}"; }
    print_success() { echo -e "${GREEN}✓${RESET} $1"; }
    print_error() { echo -e "${RED}✗${RESET} $1"; exit 1; }

    echo ""
    echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo -e "${BOLD}  📦 Publishing a3s-code-core to crates.io${RESET}"
    echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo ""

    CORE_VERSION=$(grep '^version' core/Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
    echo -e "  ${DIM}a3s-code-core:${RESET} ${BOLD}${CORE_VERSION}${RESET}"
    echo ""

    print_step "Checking version sync..."
    just check-versions || { print_error "Version mismatch. Run 'just bump-version <version>' to fix."; }

    print_step "Checking formatting..."
    cargo fmt --all -- --check && print_success "Formatting OK" || print_error "Run 'just fmt' first."

    print_step "Running clippy..."
    cargo clippy --workspace --all-targets -- -D warnings && print_success "Clippy OK" || print_error "Fix warnings first."

    print_step "Running tests..."
    cargo test --workspace --lib && print_success "Tests OK" || print_error "Tests failed."

    print_step "Publishing a3s-code-core..."
    cargo publish -p a3s-code-core && print_success "a3s-code-core published" || print_error "Failed to publish core."

    echo ""
    echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo -e "  ${GREEN}${BOLD}✓ Published a3s-code-core v${CORE_VERSION}${RESET}"
    echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo ""

# Publish dry-run (verify without publishing)
publish-dry:
    cargo publish -p a3s-code-core --dry-run

# Show current version of all packages
version:
    #!/usr/bin/env bash
    CANONICAL=$(grep '^version' core/Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
    echo "core/Cargo.toml           ${CANONICAL}"
    echo "sdk/node/Cargo.toml       $(grep '^version' sdk/node/Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
    echo "sdk/node/package.json     $(grep '"version"' sdk/node/package.json | head -1 | sed 's/.*"\([0-9.]*\)".*/\1/')"
    echo "sdk/python/Cargo.toml     $(grep '^version' sdk/python/Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"
    echo "sdk/python/pyproject.toml $(grep '^version' sdk/python/pyproject.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"

# Check all package version files are in sync with core/Cargo.toml
check-versions:
    #!/usr/bin/env bash
    set -e

    GREEN='\033[0;32m'
    RED='\033[0;31m'
    BOLD='\033[1m'
    DIM='\033[2m'
    RESET='\033[0m'

    CANONICAL=$(grep '^version' core/Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')

    echo ""
    echo -e "${BOLD}Version sync check — canonical: ${CANONICAL}${RESET}"
    echo ""

    FAIL=0
    check() {
        local label="$1"
        local actual="$2"
        if [ "$actual" = "$CANONICAL" ]; then
            echo -e "  ${GREEN}✓${RESET}  $(printf '%-35s' "$label") ${DIM}${actual}${RESET}"
        else
            echo -e "  ${RED}✗${RESET}  $(printf '%-35s' "$label") ${RED}${actual}${RESET}  ← expected ${CANONICAL}"
            FAIL=1
        fi
    }

    check "core/Cargo.toml"           "$(grep '^version' core/Cargo.toml           | head -1 | sed 's/.*"\(.*\)".*/\1/')"
    check "sdk/node/Cargo.toml"       "$(grep '^version' sdk/node/Cargo.toml       | head -1 | sed 's/.*"\(.*\)".*/\1/')"
    check "sdk/node/package.json"     "$(grep '"version"' sdk/node/package.json    | head -1 | sed 's/.*"\([0-9.]*\)".*/\1/')"
    check "sdk/python/Cargo.toml"     "$(grep '^version' sdk/python/Cargo.toml     | head -1 | sed 's/.*"\(.*\)".*/\1/')"
    check "sdk/python/pyproject.toml" "$(grep '^version' sdk/python/pyproject.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')"

    echo ""
    if [ "$FAIL" -ne 0 ]; then
        echo -e "  ${RED}${BOLD}Version mismatch detected. Run 'just bump-version <version>' to sync all files.${RESET}"
        echo ""
        exit 1
    else
        echo -e "  ${GREEN}${BOLD}All versions in sync ✓${RESET}"
        echo ""
    fi

# Bump version across all package files atomically
bump-version VERSION:
    #!/usr/bin/env bash
    set -e

    GREEN='\033[0;32m'
    BOLD='\033[1m'
    DIM='\033[2m'
    RESET='\033[0m'

    V="{{VERSION}}"

    echo ""
    echo -e "${BOLD}Bumping all packages to ${V}${RESET}"
    echo ""

    bump_toml() {
        local file="$1"
        sed -i.bak "s/^version = \".*\"/version = \"${V}\"/" "$file" && rm -f "${file}.bak"
        echo -e "  ${GREEN}✓${RESET}  ${DIM}${file}${RESET}"
    }
    bump_json() {
        local file="$1"
        sed -i.bak "s/\"version\": \".*\"/\"version\": \"${V}\"/" "$file" && rm -f "${file}.bak"
        echo -e "  ${GREEN}✓${RESET}  ${DIM}${file}${RESET}"
    }

    bump_toml core/Cargo.toml
    bump_toml sdk/node/Cargo.toml
    bump_json sdk/node/package.json
    bump_toml sdk/python/Cargo.toml
    bump_toml sdk/python/pyproject.toml

    echo ""
    just check-versions
