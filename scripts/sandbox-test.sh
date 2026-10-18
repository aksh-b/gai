#!/usr/bin/env bash
# Run the code-sandbox tests that need a real container engine: the podman
# machine's socket is passed into the dev container, so the tests start
# sibling containers on the same engine.
#
#   scripts/sandbox-test.sh            # all gai-sandbox tests, ignored ones included
#   scripts/sandbox-test.sh runs_node  # one test
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GAI_PODMAN_ARGS="-v /run/podman/podman.sock:/var/run/docker.sock ${GAI_PODMAN_ARGS:-}" \
  "$here/in-container.sh" "cargo test -p gai-sandbox -- --include-ignored ${*:-}"
