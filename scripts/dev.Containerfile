# Development and CI image for gai: the Rust toolchain plus the tools that
# scripts/check.sh needs, and Chromium for the gai-browser tests. Built
# once with scripts/dev-image.sh.
FROM docker.io/library/rust:1-slim-bookworm

RUN rustup component add rustfmt clippy \
 && apt-get update \
 && apt-get install -y --no-install-recommends sqlite3 git ca-certificates pkg-config curl chromium fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/*

# OCR for scanned documents (gai-memory's tests use them when present).
RUN apt-get update \
 && apt-get install -y --no-install-recommends tesseract-ocr poppler-utils \
 && rm -rf /var/lib/apt/lists/*
