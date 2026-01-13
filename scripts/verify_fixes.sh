#!/bin/bash
set -e

echo "==================================="
echo "  skills.rs - Verification Script"
echo "==================================="
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

FAILED=0

# Function to print test result
print_result() {
    if [ $1 -eq 0 ]; then
        echo -e "${GREEN}✅ PASS${NC}: $2"
    else
        echo -e "${RED}❌ FAIL${NC}: $2"
        FAILED=1
    fi
}

print_warning() {
    echo -e "${YELLOW}⚠️  WARNING${NC}: $1"
}

echo "1. Checking for binary name collision..."
if cargo build --workspace 2>&1 | grep -q "output filename collision"; then
    print_result 1 "Binary collision still present"
    echo "   Fix: Delete skills/src/main.rs"
else
    print_result 0 "No binary collision"
fi

echo ""
echo "2. Running cargo check..."
if cargo check --workspace --all-targets > /dev/null 2>&1; then
    print_result 0 "Cargo check passed"
else
    print_result 1 "Cargo check failed"
    echo "   Run: cargo check --workspace --all-targets"
fi

echo ""
echo "3. Running tests..."
TEST_OUTPUT=$(cargo test --workspace 2>&1)
if echo "$TEST_OUTPUT" | grep -q "test result: ok"; then
    TEST_COUNT=$(echo "$TEST_OUTPUT" | grep "test result: ok" | head -1 | sed 's/.*ok. \([0-9]*\) passed.*/\1/')
    print_result 0 "All tests pass ($TEST_COUNT tests)"
else
    print_result 1 "Tests failed"
    echo "   Run: cargo test --workspace"
fi

echo ""
echo "4. Checking for dead code warnings..."
DEAD_CODE_COUNT=$(cargo build --workspace 2>&1 | grep "warning.*is never" | wc -l)
if [ "$DEAD_CODE_COUNT" -eq 0 ]; then
    print_result 0 "No dead code warnings"
else
    print_warning "$DEAD_CODE_COUNT dead code warnings present"
    echo "   These may be intentional stubs. Check CODE_REVIEW.md"
fi

echo ""
echo "5. Running clippy..."
CLIPPY_OUTPUT=$(cargo clippy --workspace --all-targets 2>&1)
CLIPPY_WARNINGS=$(echo "$CLIPPY_OUTPUT" | grep "warning:" | wc -l)
if [ "$CLIPPY_WARNINGS" -eq 0 ]; then
    print_result 0 "No clippy warnings"
else
    print_warning "$CLIPPY_WARNINGS clippy warnings present"
    echo "   Run: cargo clippy --workspace --all-targets"
fi

echo ""
echo "6. Running rustfmt check..."
if cargo fmt --all -- --check > /dev/null 2>&1; then
    print_result 0 "Code is properly formatted"
else
    print_warning "Code needs formatting"
    echo "   Run: cargo fmt --all"
fi

echo ""
echo "7. Checking for src/main.rs (should not exist)..."
if [ -f "src/main.rs" ]; then
    print_result 1 "src/main.rs exists (causes binary collision)"
    echo "   Fix: rm src/main.rs"
else
    print_result 0 "src/main.rs does not exist"
fi

echo ""
echo "8. Checking documentation..."
if [ -f "CODE_REVIEW.md" ] && [ -f "QUICK_FIXES.md" ] && [ -f "REVIEW_SUMMARY.md" ]; then
    print_result 0 "Review documentation present"
else
    print_warning "Missing review documentation"
fi

echo ""
echo "==================================="
echo "  Summary"
echo "==================================="
echo ""

if [ $FAILED -eq 0 ]; then
    echo -e "${GREEN}✅ All critical checks passed!${NC}"
    echo ""
    echo "Next steps:"
    echo "  1. Review CODE_REVIEW.md for detailed findings"
    echo "  2. Apply quick fixes from QUICK_FIXES.md"
    echo "  3. Implement stubbed components (see REVIEW_SUMMARY.md)"
    exit 0
else
    echo -e "${RED}❌ Some checks failed${NC}"
    echo ""
    echo "Please review the failures above and:"
    echo "  1. Read QUICK_FIXES.md for immediate fixes"
    echo "  2. Read CODE_REVIEW.md for detailed analysis"
    exit 1
fi
