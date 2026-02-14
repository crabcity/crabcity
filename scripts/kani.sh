#!/usr/bin/env bash
#
# Run Kani bounded model checking on crab_city_auth proof harnesses.
#
# Usage:
#   ./scripts/kani.sh [OPTIONS]
#
# Options:
#   --harness NAME     Run a single harness by name (substring match)
#   --list             List available proof harnesses without running them
#   --verbose          Show CBMC output (default: summary only)
#   --jobs N           Parallel verification jobs (default: number of CPUs)
#   --check            Exit 1 on any failure (for CI)
#   --help             Show this help message
#
# Examples:
#   ./scripts/kani.sh                              # Verify all harnesses
#   ./scripts/kani.sh --harness intersect           # Run matching harnesses
#   ./scripts/kani.sh --list                        # Show harness names
#   ./scripts/kani.sh --check --jobs 4              # CI mode, 4 parallel jobs
#

set -euo pipefail

_package="crab_city_auth"
_harness=""
_list=0
_verbose=0
_jobs=""
_check=0
_help=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --harness)  _harness="$2"; shift 2 ;;
        --list)     _list=1; shift ;;
        --verbose)  _verbose=1; shift ;;
        --jobs)     _jobs="$2"; shift 2 ;;
        --check)    _check=1; shift ;;
        --help|-h)  _help=1; shift ;;
        *)          echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [[ $_help -eq 1 ]]; then
    sed -n '2,/^$/p' "$0" | sed 's/^# *//'
    exit 0
fi

# ── Colors (when stdout is a terminal) ────────────────────────────────────────

if [[ -t 1 ]]; then
    _red=$'\033[31m' _grn=$'\033[32m' _ylw=$'\033[33m'
    _bld=$'\033[1m' _dim=$'\033[2m' _rst=$'\033[0m'
else
    _red='' _grn='' _ylw='' _bld='' _dim='' _rst=''
fi

# ── Find repo root ────────────────────────────────────────────────────────────

_root="$(cd "$(dirname "$0")/.." && pwd)"

# ── List mode (no kani needed) ────────────────────────────────────────────────

if [[ $_list -eq 1 ]]; then
    echo "${_bld}Kani proof harnesses in ${_package}:${_rst}"
    echo ""
    grep -rn '#\[kani::proof\]' "$_root/packages/$_package/src/" \
        | while IFS= read -r line; do
            _file=$(echo "$line" | cut -d: -f1 | sed "s|$_root/||")
            _lineno=$(echo "$line" | cut -d: -f2)
            # next non-empty line after #[kani::proof] is the fn signature
            _fn=$(sed -n "$((_lineno+1)),$((_lineno+5))p" "$(echo "$line" | cut -d: -f1)" \
                | grep -m1 'fn ' | sed 's/.*fn \([a-zA-Z0-9_]*\).*/\1/')
            printf "  ${_dim}%-45s${_rst} %s:%d\n" "$_fn" "$_file" "$_lineno"
        done
    exit 0
fi

# ── Preflight ─────────────────────────────────────────────────────────────────

if ! command -v cargo-kani &>/dev/null; then
    echo "${_red}ERROR: cargo-kani not found${_rst}"
    echo ""
    echo "Install Kani:"
    echo "  cargo install --locked kani-verifier"
    echo "  cargo kani setup"
    echo ""
    echo "See: https://model-checking.github.io/kani/install-guide.html"
    exit 1
fi

# ── Build kani command ────────────────────────────────────────────────────────

_cmd=(cargo kani -p "$_package")

if [[ -n "$_harness" ]]; then
    _cmd+=(--harness "$_harness")
fi

if [[ -n "$_jobs" ]]; then
    _cmd+=(--jobs "$_jobs")
fi

if [[ $_verbose -eq 0 ]]; then
    _cmd+=(--output-format terse)
fi

# ── Run ───────────────────────────────────────────────────────────────────────

echo "${_bld}Running Kani verification on ${_package}...${_rst}"
echo "${_dim}${_cmd[*]}${_rst}"
echo ""

_start=$(date +%s)

if "${_cmd[@]}"; then
    _end=$(date +%s)
    _elapsed=$((_end - _start))
    echo ""
    echo "${_grn}${_bld}All harnesses verified${_rst} ${_dim}(${_elapsed}s)${_rst}"
else
    _rc=$?
    _end=$(date +%s)
    _elapsed=$((_end - _start))
    echo ""
    echo "${_red}${_bld}Verification failed${_rst} ${_dim}(${_elapsed}s)${_rst}"
    if [[ $_check -eq 1 ]]; then
        exit "$_rc"
    fi
    exit "$_rc"
fi
