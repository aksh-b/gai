#!/usr/bin/env bash
# Runs cargo inside the dev container.
#
#   scripts/cargo.sh build
#   scripts/cargo.sh test --workspace
set -euo pipefail
here="$(dirname "${BASH_SOURCE[0]}")"
args=""
for a in "$@"; do args+=" $(printf '%q' "$a")"; done
exec "$here/in-container.sh" "cargo$args"
