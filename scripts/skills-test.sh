#!/usr/bin/env bash
# Run the Python checks of the bundled skills in the code sandbox image
# (scripts/sandbox-image.sh builds it), without network, as code_exec would.
#
#   scripts/skills-test.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && (pwd -W 2>/dev/null || pwd))"
auth="${TMPDIR:-/tmp}/gai-podman-auth"
mkdir -p "$auth" && echo '{}' > "$auth/config.json"
export DOCKER_CONFIG="$auth" REGISTRY_AUTH_FILE="$auth/config.json"
for dir in "$here"/../examples/skills/*/; do
  name="$(basename "$dir")"
  [ -n "$(ls "$dir"/test_*.py 2>/dev/null)" ] || continue
  echo "== $name"
  src="$(cd "$dir" && (pwd -W 2>/dev/null || pwd))"
  MSYS_NO_PATHCONV=1 podman run --rm --pull=never --network none -v "$src:/skill:ro" -w /tmp \
    -e PYTHONPATH=/skill -e PYTHONDONTWRITEBYTECODE=1 localhost/gai-sandbox-python:latest \
    sh -c 'for t in /skill/test_*.py; do python "$t"; done'
done
