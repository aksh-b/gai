#!/usr/bin/env bash
# Run gai-data's tests against throwaway Postgres and MySQL servers on the
# gai-net network (removed again afterwards). SQLite, BigQuery and
# Snowflake (mocked) tests run without them.
#
#   scripts/data-test.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
auth="${TMPDIR:-/tmp}/gai-podman-auth"
mkdir -p "$auth" && echo '{}' > "$auth/config.json"
export DOCKER_CONFIG="$auth" REGISTRY_AUTH_FILE="$auth/config.json"
podman network exists gai-net || podman network create gai-net >/dev/null
podman rm -f gai-data-pg gai-data-my >/dev/null 2>&1 || true
podman run -d --rm --name gai-data-pg --network gai-net -e POSTGRES_PASSWORD=gai \
  docker.io/library/postgres:17-alpine >/dev/null
podman run -d --rm --name gai-data-my --network gai-net -e MYSQL_ROOT_PASSWORD=gai -e MYSQL_DATABASE=app \
  docker.io/library/mysql:8.4 >/dev/null
trap 'podman rm -f gai-data-pg gai-data-my >/dev/null 2>&1 || true' EXIT
until podman exec gai-data-pg pg_isready -q; do sleep 1; done
until podman exec gai-data-my mysqladmin ping -h 127.0.0.1 -pgai --silent >/dev/null 2>&1; do sleep 2; done
GAI_PODMAN_ARGS="--network gai-net -e GAI_TEST_PG=postgres://postgres@gai-data-pg:5432/postgres -e GAI_TEST_MYSQL=mysql://root@gai-data-my:3306/app -e GAI_TEST_DB_PASSWORD=gai ${GAI_PODMAN_ARGS:-}" \
  "$here/in-container.sh" "cargo test -p gai-data ${*:-}"
