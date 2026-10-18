#!/usr/bin/env bash
# Build and serve gai (API + dashboard) from the dev container on
# http://localhost:8484. Data lives in the gai-data volume. The port is
# published on 127.0.0.1 only, hence --local: auth stays open until the
# first API key is created.
#
#   scripts/serve.sh                 # builds web/ first if needed
#   ANTHROPIC_API_KEY=... scripts/serve.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ ! -f "$here/../web/dist/index.html" ]; then
  (cd "$here/../web" && npm install && npm run build)
fi
default_model="${GAI_DEFAULT_MODEL:-}"
# Without a key or a default_model in gai.toml, fall back to the mock model.
if [ -z "$default_model" ] && [ -z "${ANTHROPIC_API_KEY:-}" ] \
  && ! grep -q '^default_model' "$here/../gai.toml" 2>/dev/null; then
  default_model="mock/echo"
fi
# Ollama: prefer the one scripts/ollama.sh runs next to gai (reached over
# the gai-net network, no Windows changes needed). Otherwise try model
# servers on Windows (Ollama, LM Studio) at the podman VM's gateway; those
# must listen beyond 127.0.0.1 and be allowed through the Windows firewall.
host_ip="$(podman machine ssh "ip route" 2>/dev/null | awk '/^default/ {print $3; exit}' | tr -d '\r')"
extra=""
if [ "$(podman inspect -f '{{.State.Running}}' gai-ollama 2>/dev/null)" = "true" ]; then
  extra="--network gai-net -e GAI_PROVIDERS__OLLAMA__BASE_URL=${GAI_PROVIDERS__OLLAMA__BASE_URL:-http://gai-ollama:11434/v1}"
elif [ -n "$host_ip" ]; then
  extra="-e GAI_PROVIDERS__OLLAMA__BASE_URL=${GAI_PROVIDERS__OLLAMA__BASE_URL:-http://$host_ip:11434/v1}"
fi
if [ -n "$host_ip" ]; then
  extra="$extra -e GAI_PROVIDERS__LMSTUDIO__BASE_URL=${GAI_PROVIDERS__LMSTUDIO__BASE_URL:-http://$host_ip:1234/v1}"
fi
# Without real media models configured, use the mock ones so the media
# tools, the playground mic and the /v1/audio and /v1/images endpoints work.
if ! grep -q '^\[media\]' "$here/../gai.toml" 2>/dev/null; then
  extra="$extra -e GAI_MEDIA__IMAGE=${GAI_MEDIA__IMAGE:-mock/image} -e GAI_MEDIA__SPEECH=${GAI_MEDIA__SPEECH:-mock/voice} -e GAI_MEDIA__TRANSCRIPTION=${GAI_MEDIA__TRANSCRIPTION:-mock/ears}"
fi
GAI_PODMAN_ARGS="-p 127.0.0.1:8484:8484 -v gai-data:/data -e GAI_DATA_DIR=/data ${default_model:+-e GAI_DEFAULT_MODEL=$default_model} $extra" \
  "$here/in-container.sh" 'cargo build -q -p gai-cli && /target/debug/gai --log "warn,gai_tools=info,gai_agent=info,gai_server=info" serve --bind 0.0.0.0:8484 --local --ui web/dist'
