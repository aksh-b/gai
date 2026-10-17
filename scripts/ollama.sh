#!/usr/bin/env bash
# Run Ollama inside the podman machine, next to the gai server, and pull
# models into it. gai reaches it over the gai-net network, so nothing on
# Windows (Ollama settings, firewall) is involved. CPU only. Models get a
# 12k-token context (OLLAMA_CONTEXT_LENGTH): Ollama's own default is small
# and it cuts longer prompts off silently. To change it, remove the
# container (podman rm -f gai-ollama; the models are kept) and run again.
#
#   scripts/ollama.sh                    # start, and pull gemma3:1b
#   scripts/ollama.sh qwen3:4b llama3.2  # start, and pull these models
#   scripts/ollama.sh stop
set -euo pipefail
auth="${TMPDIR:-/tmp}/gai-podman-auth"
mkdir -p "$auth" && echo '{}' > "$auth/config.json"
export DOCKER_CONFIG="$auth" REGISTRY_AUTH_FILE="$auth/config.json" MSYS_NO_PATHCONV=1

if [ "${1:-}" = "stop" ]; then
  podman stop gai-ollama >/dev/null && echo "stopped gai-ollama"
  exit 0
fi

podman network exists gai-net || podman network create gai-net >/dev/null
if ! podman container exists gai-ollama; then
  podman run -d --name gai-ollama --network gai-net \
    -v gai-ollama:/root/.ollama \
    -e OLLAMA_KEEP_ALIVE=30m \
    -e OLLAMA_CONTEXT_LENGTH="${OLLAMA_CONTEXT_LENGTH:-12288}" \
    docker.io/ollama/ollama:latest >/dev/null
elif [ "$(podman inspect -f '{{.State.Running}}' gai-ollama)" != "true" ]; then
  podman start gai-ollama >/dev/null
fi
for i in $(seq 1 30); do podman exec gai-ollama ollama list >/dev/null 2>&1 && break; sleep 1; done

models=("$@")
[ ${#models[@]} -eq 0 ] && models=(gemma3:1b)
for m in "${models[@]}"; do
  podman exec gai-ollama ollama pull "$m"
done
podman exec gai-ollama ollama list
echo "gai-ollama is running; restart scripts/serve.sh to use it."
