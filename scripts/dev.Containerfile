# Development and CI image for gai: the Rust toolchain plus the tools that
# scripts/check.sh needs. Built once with scripts/dev-image.sh.
FROM docker.io/library/rust:1-slim-bookworm

RUN rustup component add rustfmt clippy \
 && apt-get update \
 && apt-get install -y --no-install-recommends sqlite3 git ca-certificates pkg-config curl \
 && rm -rf /var/lib/apt/lists/*
