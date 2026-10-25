#!/usr/bin/env bash
# Run the store tests against a throwaway Postgres: starts postgres:17 as
# gai-pg on the gai-net network, runs gai-store's tests with the postgres
# feature, and removes the container again.
#
#   scripts/postgres-test.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
auth="${TMPDIR:-/tmp}/gai-podman-auth"
mkdir -p "$auth" && echo '{}' > "$auth/config.json"
export DOCKER_CONFIG="$auth" REGISTRY_AUTH_FILE="$auth/config.json"
podman network exists gai-net || podman network create gai-net >/dev/null
podman rm -f gai-pg >/dev/null 2>&1 || true
podman run -d --rm --name gai-pg --network gai-net -e POSTGRES_PASSWORD=gai \
  docker.io/library/postgres:17-alpine >/dev/null
trap 'podman rm -f gai-pg >/dev/null 2>&1 || true' EXIT
until podman exec gai-pg pg_isready -q; do sleep 1; done
GAI_PODMAN_ARGS="--network gai-net -e GAI_TEST_POSTGRES_URL=postgres://postgres:gai@gai-pg:5432/postgres ${GAI_PODMAN_ARGS:-}" \
  "$here/in-container.sh" "cargo test -p gai-store --features postgres ${*:-}"
