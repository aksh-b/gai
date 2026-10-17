#!/usr/bin/env bash
# Run the gai CLI against the same data as scripts/serve.sh (the gai-data
# volume), e.g.:
#
#   scripts/gai.sh keys create --name me --scope admin
#   scripts/gai.sh runs list
set -euo pipefail
here="$(dirname "${BASH_SOURCE[0]}")"
args=""
for a in "$@"; do args+=" $(printf '%q' "$a")"; done
GAI_PODMAN_ARGS="-v gai-data:/data -e GAI_DATA_DIR=/data" \
  "$here/in-container.sh" "cargo build -q -p gai-cli && /target/debug/gai$args"
