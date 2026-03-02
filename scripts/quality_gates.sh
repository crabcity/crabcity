#!/usr/bin/env bash
#
# Run quality gates for the repository.
#
# Usage:
#   scripts/quality_gates.sh [gate ...]
#
# Gates: format, clippy, test, coverage
# No args runs all gates. Named args run only those gates.
#
# Examples:
#   scripts/quality_gates.sh              # all gates
#   scripts/quality_gates.sh test         # just tests
#   scripts/quality_gates.sh format test  # two gates
#

set -uo pipefail

# ── Colors (when stdout is a terminal) ────────────────────────────────────────

if [[ -t 1 ]]; then
    _grn=$'\033[32m' _red=$'\033[31m' _bld=$'\033[1m' _dim=$'\033[2m' _rst=$'\033[0m'
else
    _grn='' _red='' _bld='' _dim='' _rst=''
fi

# ── Temp dir for captured output ──────────────────────────────────────────────

_tmpdir=$(mktemp -d)
trap 'rm -rf "$_tmpdir"' EXIT

# ── Gate definitions ──────────────────────────────────────────────────────────

_all_gates=(format clippy test coverage)

gate_format() {
    bazel run //tools/format -- --mode=check 2>&1
}

gate_clippy() {
    local output
    output=$(bazel build //packages/... //tools/... --config=clippy --keep_going 2>&1 || true)

    # Extract clippy errors (same approach as tools/lint/lint.sh)
    local errors
    errors=$(echo "$output" | grep -A50 "^error: field\|^error: unused\|^error: this\|^error\[" \
        | grep -v "^error: aborting\|^ERROR:\|^--$" || true)

    if [[ -n "$errors" ]]; then
        echo "$errors"
        return 1
    fi
}

gate_test() {
    bazel test //... --test_output=errors 2>&1
}

gate_coverage() {
    scripts/coverage.sh --check 75 2>&1
}

# ── Runner ────────────────────────────────────────────────────────────────────

run_gate() {
    local name=$1
    local logfile="$_tmpdir/$name.log"

    printf "%s: " "$name"

    local start=$SECONDS
    if "gate_$name" > "$logfile" 2>&1; then
        local elapsed=$(( SECONDS - start ))
        echo "${_grn}PASS${_rst} (${elapsed}s)"
        return 0
    else
        local elapsed=$(( SECONDS - start ))
        echo "${_red}FAIL${_rst} (${elapsed}s)"
        # Show last 50 lines of output, indented
        tail -50 "$logfile" | sed 's/^/    /'
        return 1
    fi
}

# ── Parse args ────────────────────────────────────────────────────────────────

gates=()
for arg in "$@"; do
    # Validate gate name
    found=0
    for g in "${_all_gates[@]}"; do
        if [[ "$arg" == "$g" ]]; then
            found=1
            break
        fi
    done
    if [[ $found -eq 0 ]]; then
        echo "Unknown gate: $arg"
        echo "Valid gates: ${_all_gates[*]}"
        exit 2
    fi
    gates+=("$arg")
done

# Default: all gates
if [[ ${#gates[@]} -eq 0 ]]; then
    gates=("${_all_gates[@]}")
fi

# ── Run gates ─────────────────────────────────────────────────────────────────

passed=0
total=${#gates[@]}

echo "${_bld}Running ${total} quality gate(s)${_rst}"
echo ""

for gate in "${gates[@]}"; do
    if run_gate "$gate"; then
        ((passed++))
    fi
done

echo ""

if [[ $passed -eq $total ]]; then
    echo "${_grn}${passed}/${total} passed${_rst}"
    exit 0
else
    echo "${_red}${passed}/${total} passed${_rst}"
    exit 1
fi
