#!/usr/bin/env bash
set -euo pipefail

DATA_DIR="${1:-local/state/dev}"

bazel build //packages/crab_city:crab_embedded
exec bazel-bin/packages/crab_city/crab_embedded --data-dir "$DATA_DIR" server
