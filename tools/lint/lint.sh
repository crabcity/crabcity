#!/usr/bin/env bash
set -uo pipefail

cd "$BUILD_WORKSPACE_DIRECTORY"

EXIT_CODE=0

# Targets to lint (exclude packages with missing dependencies)
TARGETS="${//packages/... //tools/...}"

echo ""
echo "=== Rust linter (clippy) ==="
CLIPPY_OUTPUT=$(bazel build $TARGETS --config=clippy --keep_going 2>&1 || true)

# Extract clippy errors (lines starting with "error:" that aren't bazel errors)
CLIPPY_ERRORS=$(echo "$CLIPPY_OUTPUT" | grep -A50 "^error: field\|^error: unused\|^error: this\|^error\[" | grep -v "^error: aborting\|^ERROR:\|^--$" || true)
if [ -n "$CLIPPY_ERRORS" ]; then
    echo "$CLIPPY_ERRORS"
    EXIT_CODE=1
else
    echo "No issues found."
fi

echo ""
echo "=== Starlark linter (buildifier) ==="
FORMAT_OUTPUT=$(bazel run //tools/format -- --mode=check 2>&1 || true)
# Look for files that would be reformatted
if echo "$FORMAT_OUTPUT" | grep -q "would reformat"; then
    echo "$FORMAT_OUTPUT" | grep "would reformat" || true
    EXIT_CODE=1
else
    echo "No issues found."
fi

echo ""
echo "=== Summary ==="
if [ $EXIT_CODE -eq 0 ]; then
    echo "All linters passed."
else
    echo "Some linters reported issues."
fi

exit $EXIT_CODE
