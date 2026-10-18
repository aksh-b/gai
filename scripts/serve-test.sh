#!/usr/bin/env bash
# A second gai server for trying changes without touching the main one:
# port 8485, its own data (volume gai-shake), the same Ollama and gai.toml.
# Auth stays open while it has no keys. Stop it with: podman stop gai-shake
set -euo pipefail
here="$(dirname "${BASH_SOURCE[0]}")"
podman rm -f gai-shake >/dev/null 2>&1 || true
GAI_PODMAN_ARGS="-p 127.0.0.1:8485:8484 -v gai-shake:/data -e GAI_DATA_DIR=/data --network gai-net \
  -v /run/podman/podman.sock:/var/run/docker.sock \
  -e GAI_PROVIDERS__OLLAMA__BASE_URL=http://gai-ollama:11434/v1 \
  -e GAI_MEDIA__IMAGE=mock/image -e GAI_MEDIA__SPEECH=mock/voice -e GAI_MEDIA__TRANSCRIPTION=mock/ears \
  --name gai-shake" \
  "$here/in-container.sh" 'cargo build -q -p gai-cli && /target/debug/gai --log "warn,gai_agent=info,gai_server=info" serve --bind 0.0.0.0:8484 --local --ui web/dist'
