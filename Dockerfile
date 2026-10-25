# gai as one container: the server with the dashboard built in.
#
#   docker build -t gai .
#   docker run -p 127.0.0.1:8484:8484 -v gai-data:/data gai serve --local
#
# Build with `--build-arg BROWSER=1` to include Chromium for the browser
# tools (about 300 MB more). See README.md for keys, models and the code
# sandbox.

# ---- dashboard ----
FROM docker.io/library/node:22-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# ---- binary ----
FROM docker.io/library/rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
COPY --from=web /web/dist web/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p gai-cli \
 && cp target/release/gai /usr/local/bin/gai

# ---- runtime ----
FROM docker.io/library/debian:bookworm-slim
ARG BROWSER=0
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates tini \
 && if [ "$BROWSER" != "0" ]; then \
      apt-get install -y --no-install-recommends chromium fonts-dejavu-core; \
    fi \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home-dir /data --create-home gai
COPY --from=build /usr/local/bin/gai /usr/local/bin/gai
USER gai
WORKDIR /data
ENV GAI_DATA_DIR=/data \
    GAI_BIND=0.0.0.0:8484
VOLUME /data
EXPOSE 8484
ENTRYPOINT ["tini", "--", "gai"]
CMD ["serve"]
