#!/usr/bin/env bash
# Format check, clippy with warnings denied, and the full test suite.
set -euo pipefail
exec "$(dirname "${BASH_SOURCE[0]}")/in-container.sh" \
  'cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace'
