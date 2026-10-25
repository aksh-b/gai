#!/usr/bin/env bash
# Run the vector index tests against throwaway Qdrant and pgvector
# containers on the gai-net network, removing them afterwards.
#
#   scripts/vector-test.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
auth="${TMPDIR:-/tmp}/gai-podman-auth"
mkdir -p "$auth" && echo '{}' > "$auth/config.json"
export DOCKER_CONFIG="$auth" REGISTRY_AUTH_FILE="$auth/config.json"
podman network exists gai-net || podman network create gai-net >/dev/null
podman rm -f gai-qdrant gai-pgvector >/dev/null 2>&1 || true
podman run -d --rm --name gai-qdrant --network gai-net docker.io/qdrant/qdrant:v1.15.4 >/dev/null
podman run -d --rm --name gai-pgvector --network gai-net -e POSTGRES_PASSWORD=gai \
  docker.io/pgvector/pgvector:pg17 >/dev/null
trap 'podman rm -f gai-qdrant gai-pgvector >/dev/null 2>&1 || true' EXIT
until podman exec gai-pgvector pg_isready -q; do sleep 1; done
GAI_PODMAN_ARGS="--network gai-net -e GAI_TEST_QDRANT_URL=http://gai-qdrant:6333 -e GAI_TEST_PGVECTOR_URL=postgres://postgres:gai@gai-pgvector:5432/postgres ${GAI_PODMAN_ARGS:-}" \
  "$here/in-container.sh" "cargo test -p gai-memory --test vector_index ${*:-}"
