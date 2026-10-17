#!/usr/bin/env bash
# Quiet cargo: scripts/cargo.sh without colour codes or progress lines,
# for reading output in logs.
#
#   scripts/q.sh test -p gai-store
"$(dirname "${BASH_SOURCE[0]}")/cargo.sh" "$@" < /dev/null 2>&1 \
  | sed -E 's/\x1b\[[0-9;]*m//g' \
  | grep -vE '^\s*(Compiling|Downloaded|Downloading|Checking|Fresh|Blocking)\b'
exit "${PIPESTATUS[0]}"
