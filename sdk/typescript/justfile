# A3S Code TypeScript SDK - Justfile

default:
    @just --list

# ============================================================================
# Setup
# ============================================================================

# Install dependencies
install:
    npm install

# ============================================================================
# Build
# ============================================================================

# Build TypeScript
build:
    npm run build

# Clean build artifacts
clean:
    rm -rf dist lib coverage .nyc_output
    rm -f *.js *.d.ts *.js.map
    find . -name "*.tgz" -delete 2>/dev/null || true

# Type check without emitting
check:
    npx tsc --noEmit

# ============================================================================
# Test (unified command with progress display)
# ============================================================================

# Run all tests with progress display
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

    print_header() {
        echo ""
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
        echo -e "${BOLD}  $1${RESET}"
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    }

    print_header "🧪 A3S Code TypeScript SDK Test Suite"
    echo ""
    echo -ne "${CYAN}▶${RESET} ${BOLD}@a3s-lab/code${RESET} "

    # Run tests and capture output
    if OUTPUT=$(npm test 2>&1); then
        TEST_EXIT=0
    else
        TEST_EXIT=1
    fi

    # Extract test results from vitest output
    PASSED=$(echo "$OUTPUT" | grep -oE '[0-9]+ passed' | grep -oE '[0-9]+' || echo "0")
    FAILED=$(echo "$OUTPUT" | grep -oE '[0-9]+ failed' | grep -oE '[0-9]+' || echo "0")

    if [ "$FAILED" -gt 0 ] || [ "$TEST_EXIT" -ne 0 ]; then
        echo -e "${RED}✗${RESET} ${DIM}$PASSED passed, $FAILED failed${RESET}"
        echo "$OUTPUT" | grep -E "FAIL|Error|AssertionError" | head -5 | sed 's/^/    /'
    else
        echo -e "${GREEN}✓${RESET} ${DIM}$PASSED passed${RESET}"
    fi

    # Summary
    echo ""
    echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"

    if [ "$FAILED" -gt 0 ] || [ "$TEST_EXIT" -ne 0 ]; then
        echo -e "  ${RED}${BOLD}✗ FAILED${RESET}  ${GREEN}$PASSED passed${RESET}  ${RED}$FAILED failed${RESET}"
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
        exit 1
    else
        echo -e "  ${GREEN}${BOLD}✓ PASSED${RESET}  ${GREEN}$PASSED passed${RESET}"
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    fi
    echo ""

# Run tests without progress (raw output)
test-raw:
    npm test

# Run tests in watch mode
test-watch:
    npm run test:watch

# ============================================================================
# Coverage
# ============================================================================

# Run tests with coverage report
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

    print_header() {
        echo ""
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
        echo -e "${BOLD}  $1${RESET}"
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    }

    print_header "🧪 A3S Code TypeScript SDK Test Suite with Coverage"
    echo ""

    # Run tests with coverage
    npx vitest run --coverage

    echo ""
    echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo ""

# Coverage with HTML report (opens in browser)
cov-html:
    npx vitest run --coverage --coverage.reporter=html && open coverage/index.html

# Coverage for CI (generates lcov.info)
cov-ci:
    npx vitest run --coverage --coverage.reporter=lcov

# ============================================================================
# Code Quality
# ============================================================================

# Format code with prettier
fmt:
    npx prettier --write "ts/**/*.ts"

# Lint code with eslint
lint:
    npx eslint "ts/**/*.ts" || echo "ESLint not configured"

# CI checks (check + lint + test)
ci: check test

# ============================================================================
# Publish
# ============================================================================

# Publish to npm (with all checks)
publish:
    #!/usr/bin/env bash
    set -e

    # Colors
    BOLD='\033[1m'
    GREEN='\033[0;32m'
    BLUE='\033[0;34m'
    RED='\033[0;31m'
    DIM='\033[2m'
    RESET='\033[0m'

    print_step() {
        echo -e "${BLUE}▶${RESET} ${BOLD}$1${RESET}"
    }

    print_success() {
        echo -e "${GREEN}✓${RESET} $1"
    }

    print_error() {
        echo -e "${RED}✗${RESET} $1"
        exit 1
    }

    echo ""
    echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo -e "${BOLD}  📦 Publishing @a3s-lab/code to npm${RESET}"
    echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo ""

    # Show current version
    VERSION=$(node -p "require('./package.json').version")
    echo -e "  ${DIM}Version:${RESET} ${BOLD}${VERSION}${RESET}"
    echo ""

    # Step 1: Type check
    print_step "Type checking..."
    if npx tsc --noEmit; then
        print_success "Type check OK"
    else
        print_error "Type check failed."
    fi

    # Step 2: Test
    print_step "Running tests..."
    if npm test; then
        print_success "Tests OK"
    else
        print_error "Tests failed."
    fi

    # Step 3: Build
    print_step "Building..."
    if npm run build; then
        print_success "Build OK"
    else
        print_error "Build failed."
    fi

    # Step 4: Publish
    print_step "Publishing to npm..."
    if npm publish --access public; then
        echo ""
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
        echo -e "  ${GREEN}${BOLD}✓ Successfully published @a3s-lab/code v${VERSION}${RESET}"
        echo -e "${BOLD}${BLUE}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    else
        print_error "Publish failed."
    fi
    echo ""

# Publish dry-run (verify without publishing)
publish-dry:
    #!/usr/bin/env bash
    set -e
    echo ""
    echo "┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓"
    echo "┃                    📦 Publish Dry Run (@a3s-lab/code)                  ┃"
    echo "┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛"
    echo ""
    VERSION=$(node -p "require('./package.json').version")
    echo "Version: ${VERSION}"
    echo ""
    npm pack --dry-run
    echo ""
    echo "✓ Dry run successful. Ready to publish with 'just publish'"
    echo ""

# Show current version
version:
    @node -p "require('./package.json').version"

# ============================================================================
# Utilities
# ============================================================================

# Build and test
all: build test
