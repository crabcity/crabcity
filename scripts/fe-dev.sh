#!/usr/bin/env bash
set -euo pipefail

if [ "${1:-}" ]; then
  export CRAB_CITY_DATA_DIR="$1"
fi

exec bazel run //packages/crab_city_ui:dev
