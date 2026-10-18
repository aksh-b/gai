#!/usr/bin/env bash
# Builds localhost/gai-sandbox-python (see sandbox.Containerfile).
set -euo pipefail
dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && (pwd -W 2>/dev/null || pwd))"
auth="${TMPDIR:-/tmp}/gai-podman-auth"
mkdir -p "$auth" && echo '{}' > "$auth/config.json"
MSYS_NO_PATHCONV=1 DOCKER_CONFIG="$auth" REGISTRY_AUTH_FILE="$auth/config.json" \
  podman build -t localhost/gai-sandbox-python:latest -f "$dir/sandbox.Containerfile" "$dir"
