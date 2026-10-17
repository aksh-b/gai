#!/usr/bin/env bash
# Runs a shell command inside the gai dev container.
#
# Rust cannot link natively on the Windows development machine (no MSVC
# linker), so all cargo work happens in Linux. The cargo registry and target
# directory live in named volumes so repeat builds stay fast over the bind
# mount.
#
#   scripts/in-container.sh 'cargo build'
#   GAI_PODMAN_ARGS='-p 8080:8080' scripts/in-container.sh 'cargo run -p gai-cli -- serve'
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && (pwd -W 2>/dev/null || pwd))"

# An empty registry auth file stops podman from invoking credential helpers
# (gcloud) configured in ~/.docker/config.json.
auth="${TMPDIR:-/tmp}/gai-podman-auth"
mkdir -p "$auth"
echo '{}' > "$auth/config.json"

tty_flag=()
if [ -t 0 ] && [ -t 1 ]; then tty_flag=(-it)
# Piped input (e.g. an MCP client talking to `gai mcp serve`) must reach the container.
elif [ ! -t 0 ]; then tty_flag=(-i); fi

image="localhost/gai-dev:latest"
if ! DOCKER_CONFIG="$auth" podman image exists "$image"; then
  "$(dirname "${BASH_SOURCE[0]}")/dev-image.sh" >&2
fi

MSYS_NO_PATHCONV=1 \
DOCKER_CONFIG="$auth" \
REGISTRY_AUTH_FILE="$auth/config.json" \
podman run --rm --pull=never "${tty_flag[@]}" \
  -v "${root}:/app" -w /app \
  -v gai-cargo-registry:/usr/local/cargo/registry \
  -v gai-target:/target \
  -e CARGO_TARGET_DIR=/target \
  -e CARGO_TERM_COLOR=always \
  -e ANTHROPIC_API_KEY -e OPENAI_API_KEY -e GEMINI_API_KEY \
  ${GAI_PODMAN_ARGS:-} \
  "$image" \
  bash -c "$*"
