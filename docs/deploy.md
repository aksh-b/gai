# Deploying gai

gai is one binary and one SQLite file. Run it anywhere Rust runs, or as a
container.

## Docker

```bash
docker build -t gai .                       # add --build-arg BROWSER=1 for the browser tools
docker run -d --name gai \
  -p 127.0.0.1:8484:8484 \
  -v gai-data:/data \
  -e ANTHROPIC_API_KEY \
  gai serve --local
```

The image is Debian slim with the dashboard built in. It runs as an
unprivileged user, keeps everything under `/data` (the database,
workspaces, files), and listens on `0.0.0.0:8484` inside the container.

- `--local` says the published port is only reachable from this machine
  (as with `-p 127.0.0.1:...`), so auth stays open until the first key
  exists and the dashboard signs itself in. Leave it out when the port is
  public.
- To use your own config, mount it and point at it:
  `-v ./gai.toml:/etc/gai.toml:ro -e GAI_CONFIG=/etc/gai.toml`. Any value
  can also be set with `GAI_*` variables.
- To reach Ollama on the host, use
  `-e GAI_PROVIDERS__OLLAMA__BASE_URL=http://host.docker.internal:11434/v1`
  (on Linux, add `--add-host=host.docker.internal:host-gateway`).
- The code sandbox starts containers of its own, so it needs the engine's
  socket: `-v /var/run/docker.sock:/var/run/docker.sock`, plus
  `--group-add $(stat -c %g /var/run/docker.sock)` so the gai user may use
  it. Anyone who can make gai run code then reaches that engine; keep
  `shell` and `code_exec` behind approval, or set `[sandbox] backend = "off"`.

## Auth and keys

`[server] auth` (or `--auth`) is one of:

- `auto` (default): open while no API key exists and the server is local;
  once a key exists, every request needs one.
- `required`: every request needs a key.
- `off`: no auth at all. Only behind a proxy that does it for you.

Create keys with the CLI on the server (or in the dashboard, Settings →
API keys):

```bash
gai keys create --name me --scope admin                  # prints the key once
gai keys create --name ci --scope run --budget 20        # run + read, spend cap (USD)
docker exec gai gai keys create --name me --scope admin  # in the container
```

Scopes: `read` (look), `run` (start runs, plus read) and `admin`
(everything, including agents, settings, MCP servers, bundles and keys).
Send the key as `Authorization: Bearer <key>`. Keys are stored hashed. Each
key has a rate limit (`[server] rate_limit_per_minute`, 600) and an
optional spending budget.

A remote dashboard asks for a key the first time it opens.

## Behind a proxy

Put gai behind a reverse proxy for TLS. Server-sent events stream run
output, so turn off response buffering for `/v1/` (in nginx,
`proxy_buffering off;`). If the proxy authenticates users itself, gai can
run with `auth = "off"` on a private network.

## Data and backups

Everything lives in the data folder (`GAI_DATA_DIR`, by default
`~/.local/share/gai` on Linux, `/data` in the image): `gai.db` (agents,
runs, memory, knowledge, files) and the per-session workspaces. Back up
`gai.db` with `sqlite3 gai.db ".backup backup.db"` while gai runs, or copy
it while gai is stopped. To move agents rather than the whole database, use
[bundles](bundles.md).

Runs survive restarts: on start, gai resumes unfinished runs from their
last completed step and picks up queued background runs.

## Postgres

`gai-store` has a `postgres` feature with a `PostgresStore` for agents,
settings and MCP servers, tested against the same conformance checks as
SQLite. The rest of the store is not ported yet, so a server still runs on
SQLite. See [STATUS.md](../STATUS.md).
