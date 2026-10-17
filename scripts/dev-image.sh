#!/usr/bin/env bash
# Builds the dev container image (localhost/gai-dev) used by the other scripts.
set -euo pipefail
dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && (pwd -W 2>/dev/null || pwd))"
auth="${TMPDIR:-/tmp}/gai-podman-auth"
mkdir -p "$auth" && echo '{}' > "$auth/config.json"
MSYS_NO_PATHCONV=1 DOCKER_CONFIG="$auth" REGISTRY_AUTH_FILE="$auth/config.json" \
  podman build --pull=never -t localhost/gai-dev:latest -f "$dir/dev.Containerfile" "$dir"
