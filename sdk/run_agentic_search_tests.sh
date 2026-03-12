#!/bin/bash
# Run agentic_search tests with Kimi K2.5 model
#
# Usage:
#   ./run_agentic_search_tests.sh python
#   ./run_agentic_search_tests.sh typescript
#   ./run_agentic_search_tests.sh all

set -e

# Check environment variables
if [ -z "$KIMI_API_KEY" ]; then
    echo "❌ Error: KIMI_API_KEY environment variable not set"
    echo "   export KIMI_API_KEY='your-api-key'"
    exit 1
fi

if [ -z "$KIMI_BASE_URL" ]; then
    echo "❌ Error: KIMI_BASE_URL environment variable not set"
    echo "   export KIMI_BASE_URL='http://your-endpoint/v1'"
    exit 1
fi

run_python() {
    echo "Running Python tests..."
    cd sdk/python/examples
    python test_agentic_search.py
    cd ../../..
}

run_typescript() {
    echo "Running TypeScript tests..."
    cd sdk/node/examples
    npx tsx test-agentic-search.ts
    cd ../../..
}

case "${1:-all}" in
    python)
        run_python
        ;;
    typescript|ts)
        run_typescript
        ;;
    all)
        run_python
        echo ""
        run_typescript
        ;;
    *)
        echo "Usage: $0 {python|typescript|all}"
        exit 1
        ;;
esac

echo ""
echo "✅ All tests completed"
